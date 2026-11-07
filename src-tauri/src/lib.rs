mod peaks;

use serde::Serialize;
use serde_json::Value;
use std::{
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
use tauri::{path::BaseDirectory, Manager};

#[derive(Serialize)]
struct MediaInfo {
    duration: f64,
    width: Option<u32>,
    height: Option<u32>,
    fps: Option<f64>,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    audio_channels: Option<u32>,
    has_audio: bool,
}

fn binary_filename(tool: &str) -> String {
    if cfg!(windows) {
        format!("{tool}.exe")
    } else {
        tool.to_string()
    }
}

fn resolve_tool(app: &tauri::AppHandle, tool: &str) -> Result<PathBuf, String> {
    let filename = binary_filename(tool);
    let mut candidates = Vec::new();

    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("BIN").join(&filename));
        }
    }

    if let Ok(resources) = app.path().resolve("BIN", BaseDirectory::Resource) {
        candidates.push(resources.join(&filename));
    }

    // In development, media tools are prepared in the repository-level BIN/.
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        candidates.push(
            PathBuf::from(manifest_dir).join("..").join("BIN").join(&filename),
        );
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
                "Could not find {filename}. Put the media tools in ~/bin for development, or in the application's BIN folder."
            )
        })
}

fn tool_command(app: &tauri::AppHandle, tool: &str) -> Result<Command, String> {
    let executable = resolve_tool(app, tool)?;
    let mut command = Command::new(&executable);

    #[cfg(target_os = "linux")]
    {
        let directory = executable.parent().unwrap_or_else(|| std::path::Path::new("."));
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
        let directory = executable.parent().unwrap_or_else(|| std::path::Path::new("."));
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
        let directory = executable.parent().unwrap_or_else(|| std::path::Path::new("."));
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

fn run_tool(app: &tauri::AppHandle, tool: &str, args: &[&str]) -> Result<std::process::Output, String> {
    let output = tool_command(app, tool)?
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

fn probe_media_sync(app: &tauri::AppHandle, path: &str) -> Result<MediaInfo, String> {
    let output = run_tool(
        app,
        "ffprobe",
        &[
            "-v",
            "error",
            "-show_entries",
            "format=duration:stream=codec_type,codec_name,width,height,avg_frame_rate,duration,channels",
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
        fps: parse_rate(video.get("avg_frame_rate")),
        video_codec: video["codec_name"].as_str().map(str::to_string),
        audio_codec: audio.and_then(|stream| stream["codec_name"].as_str()).map(str::to_string),
        audio_channels: audio
            .and_then(|stream| stream["channels"].as_u64())
            .map(|value| value as u32),
        has_audio: audio.is_some(),
    })
}

#[tauri::command]
async fn probe_media(app: tauri::AppHandle, path: String) -> Result<MediaInfo, String> {
    tauri::async_runtime::spawn_blocking(move || probe_media_sync(&app, &path))
        .await
        .map_err(|error| format!("Media probe task failed: {error}"))?
}

fn detect_keyframes_sync(app: &tauri::AppHandle, path: &str) -> Result<Vec<f64>, String> {
    let output = run_tool(
        app,
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

#[tauri::command]
async fn detect_keyframes(app: tauri::AppHandle, path: String) -> Result<Vec<f64>, String> {
    tauri::async_runtime::spawn_blocking(move || detect_keyframes_sync(&app, &path))
        .await
        .map_err(|error| format!("Keyframe scan task failed: {error}"))?
}

fn generate_waveform_sync(
    app: &tauri::AppHandle,
    path: &str,
    channels: usize,
) -> Result<Vec<u8>, String> {
    // Mono stays mono; anything wider is mixed down to a stereo pair.
    let channels = channels.clamp(1, 2);
    let channel_arg = channels.to_string();
    let rate_arg = peaks::SAMPLE_RATE.to_string();

    let mut child = tool_command(app, "ffmpeg")?
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
    let mut buffer = vec![0_u8; 256 * 1024];

    loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => builder.push_bytes(&buffer[..count]),
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
        .map_err(|error| format!("Could not finish FFmpeg audio analysis: {error}"))?;

    if !status.success() {
        return Err("FFmpeg could not decode an audio stream from this file.".to_string());
    }

    let peaks = builder.finish();
    if peaks.blocks() == 0 {
        return Err("The audio stream contains no samples.".to_string());
    }

    Ok(peaks.to_bytes())
}

/// Returns the raw peak summary (see `peaks.rs` for the layout). The frontend
/// receives it as an `ArrayBuffer`, which avoids JSON-encoding millions of numbers.
#[tauri::command]
async fn generate_waveform(
    app: tauri::AppHandle,
    path: String,
    channels: usize,
) -> Result<tauri::ipc::Response, String> {
    tauri::async_runtime::spawn_blocking(move || generate_waveform_sync(&app, &path, channels))
        .await
        .map_err(|error| format!("Waveform task failed: {error}"))?
        .map(tauri::ipc::Response::new)
}

fn initialize_data_directory(data: &Path) -> Result<(), String> {
    let temp = data.join("Temp");
    let cache = data.join("Cache");
    std::fs::create_dir_all(&temp)
        .and_then(|_| std::fs::create_dir_all(&cache))
        .map_err(|error| format!("Could not create media data folders: {error}"))?;

    // Anything left in Temp belongs to an interrupted session. No jobs are
    // active yet during startup, so it is safe to remove these stale files.
    if let Ok(entries) = std::fs::read_dir(&temp) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                let _ = std::fs::remove_dir_all(path);
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
    }

    Ok(())
}

fn ensure_runtime_dirs(app: &tauri::AppHandle) -> Result<(), String> {
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            if initialize_data_directory(&directory.join("DATA")).is_ok() {
                return Ok(());
            }
        }
    }

    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Could not locate the application cache directory: {error}"))?;

    initialize_data_directory(&cache)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            ensure_runtime_dirs(app.handle()).map_err(std::io::Error::other)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            probe_media,
            detect_keyframes,
            generate_waveform
        ])
        .run(tauri::generate_context!())
        .expect("error while running Blaze");
}
