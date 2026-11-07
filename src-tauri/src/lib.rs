use tauri_plugin_shell::ShellExt;

#[tauri::command]
async fn get_media_tool_versions(app: tauri::AppHandle) -> Result<String, String> {
    let ffmpeg = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|error| format!("Could not locate bundled FFmpeg: {error}"))?
        .args(["-version"])
        .output()
        .await
        .map_err(|error| format!("Could not execute bundled FFmpeg: {error}"))?;

    if !ffmpeg.status.success() {
        return Err(format!(
            "Bundled FFmpeg failed: {}",
            String::from_utf8_lossy(&ffmpeg.stderr)
        ));
    }

    let ffprobe = app
        .shell()
        .sidecar("ffprobe")
        .map_err(|error| format!("Could not locate bundled FFprobe: {error}"))?
        .args(["-version"])
        .output()
        .await
        .map_err(|error| format!("Could not execute bundled FFprobe: {error}"))?;

    if !ffprobe.status.success() {
        return Err(format!(
            "Bundled FFprobe failed: {}",
            String::from_utf8_lossy(&ffprobe.stderr)
        ));
    }

    Ok(format!(
        "FFmpeg:\n{}\nFFprobe:\n{}",
        String::from_utf8_lossy(&ffmpeg.stdout),
        String::from_utf8_lossy(&ffprobe.stdout)
    ))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .invoke_handler(tauri::generate_handler![get_media_tool_versions])
        .run(tauri::generate_context!())
        .expect("error while running Blaze");
}
