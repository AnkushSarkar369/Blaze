use blaze_engine::{player, MediaEngine, MediaInfo};
use std::{
    collections::HashMap,
    env,
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Stdio},
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

/// Construct the UI-independent engine with the same resource/development
/// lookup precedence used before extraction.
fn media_engine(app: &tauri::AppHandle) -> MediaEngine {
    let mut tool_dirs = Vec::new();

    if let Ok(resources) = app.path().resolve("BIN", BaseDirectory::Resource) {
        tool_dirs.push(resources);
    }

    // In development, media tools are prepared in the repository-level BIN/.
    if let Some(manifest_dir) = option_env!("CARGO_MANIFEST_DIR") {
        tool_dirs.push(PathBuf::from(manifest_dir).join("..").join("BIN"));
    }

    MediaEngine::new(tool_dirs)
}

#[tauri::command]
async fn probe_media(app: tauri::AppHandle, path: String) -> Result<MediaInfo, String> {
    let engine = media_engine(&app);
    tauri::async_runtime::spawn_blocking(move || engine.probe_media(&path))
        .await
        .map_err(|error| format!("Media probe task failed: {error}"))?
}

#[tauri::command]
async fn detect_keyframes(app: tauri::AppHandle, path: String) -> Result<Vec<f64>, String> {
    let engine = media_engine(&app);
    tauri::async_runtime::spawn_blocking(move || engine.detect_keyframes(&path))
        .await
        .map_err(|error| format!("Keyframe scan task failed: {error}"))?
}

/// Returns the raw peak summary (see blaze-engine/src/peaks.rs for the layout).
/// The frontend receives it as an ArrayBuffer rather than JSON-encoding numbers.
#[tauri::command]
async fn generate_waveform(
    app: tauri::AppHandle,
    path: String,
    channels: usize,
) -> Result<tauri::ipc::Response, String> {
    let engine = media_engine(&app);
    tauri::async_runtime::spawn_blocking(move || engine.generate_waveform(&path, channels))
        .await
        .map_err(|error| format!("Waveform task failed: {error}"))?
        .map(tauri::ipc::Response::new)
}

/// Scratch space for decoded playback audio. Wiped on every launch.
struct MediaTemp {
    dir: PathBuf,
    counter: AtomicU32,
}

#[tauri::command]
async fn prepare_audio(
    app: tauri::AppHandle,
    temp: State<'_, MediaTemp>,
    path: String,
) -> Result<String, String> {
    let engine = media_engine(&app);
    let dir = temp.dir.clone();
    let number = temp.counter.fetch_add(1, Ordering::SeqCst) + 1;
    tauri::async_runtime::spawn_blocking(move || engine.prepare_audio(&dir, number, &path))
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

/// Starts streaming numbered JPEG preview frames from start seconds. Returns
/// a session id for player_ack/player_stop. max_frames == 0 streams to EOF.
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
    let mut command = media_engine(&app).preview_command(&path, start, fps, max_frames)?;
    command
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

/// The frontend has consumed count frames; let FFmpeg run count frames further.
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
