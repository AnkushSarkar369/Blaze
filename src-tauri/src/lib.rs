mod frames;
mod peaks;
mod player;

use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashMap,
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex,
    },
};
use tauri::{
    ipc::{Channel, InvokeResponseBody},
    path::BaseDirectory,
    Manager, State,
};

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

/// Scratch space for decoded playback audio. Wiped on every launch.
struct MediaTemp {
    dir: PathBuf,
    counter: AtomicU32,
}

fn prepare_audio_sync(
    app: &tauri::AppHandle,
    dir: &Path,
    number: u32,
    path: &str,
) -> Result<String, String> {
    // Drop playback audio from earlier imports.
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

    let result = run_tool(
        app,
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

/// Decodes the first audio stream to a 48 kHz stereo WAV that the webview's
/// audio element can play and seek in. Returns the WAV's path.
#[tauri::command]
async fn prepare_audio(
    app: tauri::AppHandle,
    temp: State<'_, MediaTemp>,
    path: String,
) -> Result<String, String> {
    let dir = temp.dir.clone();
    let number = temp.counter.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || prepare_audio_sync(&app, &dir, number, &path))
        .await
        .map_err(|error| format!("Audio preparation task failed: {error}"))?
}

struct StreamSession {
    credits: player::Credits,
    child: Mutex<Option<Child>>,
}

#[derive(Default)]
struct Streams {
    next: AtomicU32,
    active: Arc<Mutex<HashMap<u32, Arc<StreamSession>>>>,
}

/// Starts streaming numbered JPEG preview frames from `start` seconds (see
/// `player.rs` for the message format). Returns a session id for `player_ack`
/// and `player_stop`. `max_frames == 0` streams to the end of the file.
#[tauri::command]
async fn player_start(
    app: tauri::AppHandle,
    streams: State<'_, Streams>,
    path: String,
    start: f64,
    fps: f64,
    max_frames: u32,
    on_frame: Channel,
) -> Result<u32, String> {
    let mut command = tool_command(&app, "ffmpeg")?;
    command
        .args(player::video_args(&path, start, fps, max_frames))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not start FFmpeg: {error}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "Could not read FFmpeg video output.".to_string())?;
    let stderr = child.stderr.take();

    let id = streams.next.fetch_add(1, Ordering::SeqCst) + 1;
    let session = Arc::new(StreamSession {
        credits: player::Credits::new(player::INITIAL_CREDITS),
        child: Mutex::new(Some(child)),
    });
    streams.active.lock().unwrap().insert(id, session.clone());
    let active = streams.active.clone();

    std::thread::spawn(move || {
        // Keep the start of FFmpeg's error output, but always drain the pipe so
        // FFmpeg can never block on a full stderr buffer.
        let error_log = stderr.map(|mut pipe| {
            std::thread::spawn(move || {
                let mut kept = Vec::new();
                let mut chunk = [0_u8; 1024];
                while let Ok(count) = pipe.read(&mut chunk) {
                    if count == 0 {
                        break;
                    }
                    if kept.len() < 4096 {
                        kept.extend_from_slice(&chunk[..count]);
                    }
                }
                String::from_utf8_lossy(&kept).trim().to_string()
            })
        });

        let (frames, outcome) = player::pump(stdout, &session.credits, |index, jpeg| {
            on_frame
                .send(InvokeResponseBody::Raw(player::encode_frame(index, &jpeg)))
                .is_ok()
        });

        if outcome == player::Outcome::Stopped || session.credits.is_cancelled() {
            if let Some(child) = session.child.lock().unwrap().as_mut() {
                let _ = child.kill();
            }
        }

        let child = session.child.lock().unwrap().take();
        let succeeded = child
            .map(|mut child| child.wait().map(|status| status.success()).unwrap_or(false))
            .unwrap_or(false);
        let log = error_log
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();

        if !session.credits.is_cancelled() {
            let message = if frames == 0 && !succeeded {
                let text = if log.is_empty() {
                    "FFmpeg could not decode this video.".to_string()
                } else {
                    log
                };
                player::encode_marker(player::ERROR_MARKER, &text)
            } else {
                player::encode_marker(player::END_MARKER, "")
            };
            let _ = on_frame.send(InvokeResponseBody::Raw(message));
        }

        active.lock().unwrap().remove(&id);
    });

    Ok(id)
}

/// The webview has consumed `count` frames; let FFmpeg run `count` frames further.
#[tauri::command]
async fn player_ack(streams: State<'_, Streams>, session: u32, count: u32) -> Result<(), String> {
    let found = streams.active.lock().unwrap().get(&session).cloned();
    if let Some(session) = found {
        session.credits.release(count as i64);
    }
    Ok(())
}

#[tauri::command]
async fn player_stop(streams: State<'_, Streams>, session: u32) -> Result<(), String> {
    let found = streams.active.lock().unwrap().get(&session).cloned();
    if let Some(session) = found {
        session.credits.cancel();
        if let Some(child) = session.child.lock().unwrap().as_mut() {
            let _ = child.kill();
        }
    }
    Ok(())
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

/// Creates the data folders and returns the data directory that was used.
fn ensure_runtime_dirs(app: &tauri::AppHandle) -> Result<PathBuf, String> {
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            let data = directory.join("DATA");
            if initialize_data_directory(&data).is_ok() {
                return Ok(data);
            }
        }
    }

    let cache = app
        .path()
        .app_cache_dir()
        .map_err(|error| format!("Could not locate the application cache directory: {error}"))?;

    initialize_data_directory(&cache)?;
    Ok(cache)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data = ensure_runtime_dirs(app.handle()).map_err(std::io::Error::other)?;
            app.manage(MediaTemp {
                dir: data.join("Temp"),
                counter: AtomicU32::new(0),
            });
            app.manage(Streams::default());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            probe_media,
            detect_keyframes,
            generate_waveform,
            prepare_audio,
            player_start,
            player_ack,
            player_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running Blaze");
}
