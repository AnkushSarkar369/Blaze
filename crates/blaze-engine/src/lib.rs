//! UI-independent media operations for Blaze.
//!
//! FFmpeg remains an external executable. This crate owns tool discovery,
//! probing, keyframe detection, waveform analysis, audio preparation and the
//! primitives used to stream numbered MJPEG preview frames.

pub mod peaks;
pub mod silence;
pub mod export;
pub mod player;
mod frames;

use serde::Serialize;
use serde_json::Value;
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Debug, Clone, Serialize)]
pub struct MediaInfo {
    pub duration: f64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_channels: Option<u32>,
    pub audio_sample_rate: Option<u32>,
    pub has_audio: bool,
}

/// Locates and runs FFmpeg/FFprobe without depending on an application
/// framework. Application-specific resource directories are supplied by the
/// caller; the conventional executable and per-user locations are retained.
#[derive(Debug, Clone, Default)]
pub struct MediaEngine {
    extra_tool_dirs: Vec<PathBuf>,
}

impl MediaEngine {
    /// Creates an engine with optional tool directories between the bundled
    /// executable directory and the per-user ~/bin fallback.
    ///
    /// The caller should pass resource and development BIN directories in
    /// their desired precedence order.
    pub fn new(extra_tool_dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            extra_tool_dirs: extra_tool_dirs.into_iter().collect(),
        }
    }

    fn binary_filename(tool: &str) -> String {
        if cfg!(windows) {
            format!("{tool}.exe")
        } else {
            tool.to_string()
        }
    }

    fn resolve_tool(&self, tool: &str) -> Result<PathBuf, String> {
        let filename = Self::binary_filename(tool);
        let mut candidates = Vec::new();

        if let Ok(executable) = env::current_exe() {
            if let Some(directory) = executable.parent() {
                candidates.push(directory.join("Bin").join(&filename));
                candidates.push(directory.join("BIN").join(&filename));
            }
        }

        for directory in &self.extra_tool_dirs {
            candidates.push(directory.join(&filename));
        }

        let home = env::var_os("HOME")
            .or_else(|| env::var_os("USERPROFILE"))
            .map(PathBuf::from);

        if let Some(home) = home {
            candidates.push(home.join("bin").join(&filename));
        }

        candidates
            .into_iter()
            .find(|candidate| candidate.is_file())
            .ok_or_else(|| {
                format!(
                    "Could not find {filename}. Put the media tools in ~/bin for development, or in the application's Bin folder."
                )
            })
    }

    /// Builds a command for one of the bundled media tools, including the
    /// adjacent shared-library search path used by the existing Linux and
    /// macOS bundles.
    pub fn command(&self, tool: &str) -> Result<Command, String> {
        let executable = self.resolve_tool(tool)?;
        let mut command = Command::new(&executable);

        #[cfg(target_os = "linux")]
        {
            let directory = executable.parent().unwrap_or_else(|| Path::new("."));
            let mut library_paths = vec![directory.to_path_buf(), directory.join("lib")];
            if let Some(existing) = env::var_os("LD_LIBRARY_PATH") {
                library_paths.extend(env::split_paths(&existing));
            }
            if let Ok(joined) = env::join_paths(library_paths) {
                command.env("LD_LIBRARY_PATH", joined);
            }
        }

        #[cfg(target_os = "macos")]
        {
            let directory = executable.parent().unwrap_or_else(|| Path::new("."));
            let mut library_paths = vec![directory.to_path_buf(), directory.join("lib")];
            if let Some(existing) = env::var_os("DYLD_LIBRARY_PATH") {
                library_paths.extend(env::split_paths(&existing));
            }
            if let Ok(joined) = env::join_paths(library_paths) {
                command.env("DYLD_LIBRARY_PATH", joined);
            }
        }

        #[cfg(windows)]
        {
            let directory = executable.parent().unwrap_or_else(|| Path::new("."));
            let mut paths = vec![directory.to_path_buf()];
            if let Some(existing) = env::var_os("PATH") {
                paths.extend(env::split_paths(&existing));
            }
            if let Ok(joined) = env::join_paths(paths) {
                command.env("PATH", joined);
            }
        }

        Ok(command)
    }

    fn run_tool(
        &self,
        tool: &str,
        args: &[&str],
    ) -> Result<std::process::Output, String> {
        let output = self
            .command(tool)?
            .args(args)
            .output()
            .map_err(|error| format!("Could not start {tool}: {error}"))?;

        if !output.status.success() {
            return Err(format!(
                "{tool} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }

        Ok(output)
    }

    pub fn probe_media(&self, path: &str) -> Result<MediaInfo, String> {
        let output = self.run_tool(
            "ffprobe",
            &[
                "-v",
                "error",
                "-show_entries",
                "format=duration:stream=codec_type,codec_name,width,height,avg_frame_rate,r_frame_rate,duration,channels,sample_rate",
                "-of",
                "json",
                path,
            ],
        )?;

        let document: Value = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Could not parse FFprobe output: {error}"))?;

        let streams = document["streams"]
            .as_array()
            .ok_or_else(|| "FFprobe did not return any media streams.".to_string())?;

        let video = streams.iter().find(|stream| stream["codec_type"] == "video");
        let audio = streams.iter().find(|stream| stream["codec_type"] == "audio");

        let duration = document["format"]["duration"]
            .as_str()
            .and_then(|value| value.parse::<f64>().ok())
            .or_else(|| {
                video
                    .and_then(|stream| stream["duration"].as_str())
                    .and_then(|value| value.parse::<f64>().ok())
            })
            .unwrap_or(0.0);

        if video.is_none() || duration <= 0.0 {
            return Err("The selected file does not contain a readable video stream.".to_string());
        }

        let video = video.unwrap();

        Ok(MediaInfo {
            duration,
            width: video["width"].as_u64().map(|value| value as u32),
            height: video["height"].as_u64().map(|value| value as u32),
            fps: parse_rate(video.get("avg_frame_rate")).or_else(|| parse_rate(video.get("r_frame_rate"))),
            video_codec: video["codec_name"].as_str().map(str::to_string),
            audio_codec: audio
                .and_then(|stream| stream["codec_name"].as_str())
                .map(str::to_string),
            audio_channels: audio
                .and_then(|stream| stream["channels"].as_u64())
                .map(|value| value as u32),
            audio_sample_rate: audio
                .and_then(|stream| stream["sample_rate"].as_str()
                    .and_then(|value| value.parse::<u32>().ok())
                    .or_else(|| stream["sample_rate"].as_u64().map(|value| value as u32))),
            has_audio: audio.is_some(),
        })
    }

    pub fn detect_keyframes(&self, path: &str) -> Result<Vec<f64>, String> {
        let output = self.run_tool(
            "ffprobe",
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-skip_frame",
                "nokey",
                "-show_entries",
                "frame=best_effort_timestamp_time",
                "-of",
                "csv=p=0",
                path,
            ],
        )?;

        let timestamps = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter_map(|line| line.split(',').next())
            .filter_map(|value| value.trim().parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .collect();

        Ok(timestamps)
    }

    /// Generates waveform peaks while reporting decoded-audio progress from 0.0 to 1.0.
    pub fn generate_waveform_with_progress(
        &self,
        path: &str,
        channels: usize,
        duration_seconds: f64,
        mut on_progress: impl FnMut(f32),
    ) -> Result<Vec<u8>, String> {
        // Preserve the decoded channel count. PeakBuilder merges every channel
        // into one envelope without downmixing or phase cancellation.
        let channels = channels.clamp(1, 32);
        let channel_arg = channels.to_string();
        let rate_arg = peaks::SAMPLE_RATE.to_string();
        let expected_bytes = if duration_seconds.is_finite() && duration_seconds > 0.0 {
            (duration_seconds * peaks::SAMPLE_RATE as f64 * channels as f64 * 2.0)
                .max(1.0) as u64
        } else {
            0
        };

        let mut child = self
            .command("ffmpeg")?
            .args([
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-i",
                path,
                "-map",
                "0:a:0",
                "-vn",
                "-ac",
                channel_arg.as_str(),
                "-ar",
                rate_arg.as_str(),
                "-f",
                "s16le",
                "pipe:1",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("Could not start FFmpeg: {error}"))?;

        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Could not read FFmpeg audio output.".to_string())?;
        let mut builder = peaks::PeakBuilder::new(channels);
        let mut buffer = vec![0_u8; 16 * 1024];
        let mut decoded_bytes = 0_u64;
        let mut last_reported = 0_u32;

        loop {
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    builder.push_bytes(&buffer[..count]);
                    decoded_bytes = decoded_bytes.saturating_add(count as u64);

                    if expected_bytes > 0 {
                        let fraction = (decoded_bytes as f64 / expected_bytes as f64)
                            .clamp(0.0, 0.99) as f32;
                        let percent = (fraction * 100.0) as u32;
                        if percent > last_reported {
                            last_reported = percent;
                            on_progress(fraction);
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("Could not read decoded audio: {error}"));
                }
            }
        }

        let status = child
            .wait()
            .map_err(|error| format!("Could not finish audio analysis: {error}"))?;

        if !status.success() {
            return Err("FFmpeg could not decode an audio stream from this file.".to_string());
        }

        let peaks = builder.finish();
        if peaks.blocks() == 0 {
            return Err("The audio stream contains no samples.".to_string());
        }

        on_progress(1.0);
        Ok(peaks.to_bytes())
    }

    /// Decodes the first audio stream to a 48 kHz stereo WAV for playback.
    /// Existing playback WAVs in the supplied scratch directory are removed
    /// before the new decode, matching the original import lifecycle.
    pub fn prepare_audio(
        &self,
        dir: &Path,
        number: u32,
        path: &str,
    ) -> Result<String, String> {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with("playback-") && name.ends_with(".wav") {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }

        let output = dir.join(format!("playback-{number}.wav"));
        let output_arg = output
            .to_str()
            .ok_or_else(|| "The temporary folder path is not valid UTF-8.".to_string())?;

        let result = self.run_tool(
            "ffmpeg",
            &[
                "-hide_banner",
                "-nostdin",
                "-loglevel",
                "error",
                "-y",
                "-i",
                path,
                "-map",
                "0:a:0",
                "-vn",
                "-ac",
                "2",
                "-ar",
                "48000",
                "-c:a",
                "pcm_s16le",
                output_arg,
            ],
        );

        match result {
            Ok(_) => Ok(output_arg.to_string()),
            Err(error) => {
                let _ = std::fs::remove_file(&output);
                Err(error)
            }
        }
    }

    /// Builds the existing MJPEG preview command. The caller controls
    /// stdin/stdout/stderr and process lifetime.
    pub fn preview_command(
        &self,
        path: &str,
        start: f64,
        fps: f64,
        max_frames: u32,
    ) -> Result<Command, String> {
        let mut command = self.command("ffmpeg")?;
        command.args(player::video_args(path, start, fps, max_frames));
        Ok(command)
    }
}

fn parse_rate(value: Option<&Value>) -> Option<f64> {
    let value = value?.as_str()?;
    let mut parts = value.split('/');
    let numerator = parts.next()?.parse::<f64>().ok()?;
    let denominator = parts.next()?.parse::<f64>().ok()?;
    if denominator == 0.0 {
        None
    } else {
        Some(numerator / denominator)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_parser_handles_normal_and_invalid_rates() {
        assert_eq!(parse_rate(Some(&Value::String("30000/1001".into()))), Some(30000.0 / 1001.0));
        assert_eq!(parse_rate(Some(&Value::String("25/1".into()))), Some(25.0));
        assert_eq!(parse_rate(Some(&Value::String("25/0".into()))), None);
        assert_eq!(parse_rate(Some(&Value::String("not-a-rate".into()))), None);
        assert_eq!(parse_rate(None), None);
    }
}
