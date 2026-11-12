use blaze_engine::{
    player::{pump, Credits, Outcome},
    peaks, MediaEngine,
};
use std::{
    env,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = env::temp_dir().join(format!(
            "blaze-engine-ffmpeg-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create integration test directory");
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    // Windows executables carry the .exe suffix; PATH lookup does not add
    // PATHEXT automatically when checking paths with Path::is_file().
    let filename = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(&filename))
        .find(|candidate| candidate.is_file())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn real_ffmpeg_probe_keyframes_waveform_audio_and_mjpeg_stream() {
    let require_tools = env::var_os("BLAZE_REQUIRE_FFMPEG_INTEGRATION").is_some();
    let Some(ffmpeg) = find_executable("ffmpeg") else {
        if require_tools {
            panic!("real-FFmpeg integration is required, but ffmpeg was not found on PATH");
        }
        eprintln!("Skipping real-FFmpeg integration test: ffmpeg is not on PATH");
        return;
    };
    let Some(ffprobe) = find_executable("ffprobe") else {
        if require_tools {
            panic!("real-FFmpeg integration is required, but ffprobe was not found on PATH");
        }
        eprintln!("Skipping real-FFmpeg integration test: ffprobe is not on PATH");
        return;
    };
    let ffmpeg_dir = ffmpeg.parent().expect("ffmpeg has a parent directory");
    let ffprobe_dir = ffprobe.parent().expect("ffprobe has a parent directory");

    let temp = TestDir::new();
    let media = temp.0.join("sample.mkv");
    let media_arg = media.to_str().expect("temporary media path is UTF-8");

    // Use FFmpeg's built-in lavfi sources and MPEG-4/PCM codecs to avoid
    // relying on optional H.264 encoders in the test runner.
    let output = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=160x90:rate=25:duration=2",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:sample_rate=48000:duration=2",
            "-c:v",
            "mpeg4",
            "-g",
            "10",
            "-q:v",
            "5",
            "-c:a",
            "pcm_s16le",
            "-shortest",
            media_arg,
        ])
        .output()
        .expect("run FFmpeg to create integration media");
    assert!(
        output.status.success(),
        "could not create integration media: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let engine = MediaEngine::new(vec![
        ffmpeg_dir.to_path_buf(),
        ffprobe_dir.to_path_buf(),
    ]);

    let info = engine.probe_media(media_arg).expect("probe media");
    assert!((info.duration - 2.0).abs() < 0.15, "duration: {}", info.duration);
    assert_eq!(info.width, Some(160));
    assert_eq!(info.height, Some(90));
    assert!((info.fps.unwrap_or_default() - 25.0).abs() < 0.01);
    assert!(info.has_audio);
    assert_eq!(info.audio_codec.as_deref(), Some("pcm_s16le"));

    let keyframes = engine.detect_keyframes(media_arg).expect("detect keyframes");
    assert!(keyframes.len() >= 4, "keyframes: {keyframes:?}");
    assert!(keyframes[0].abs() < 0.1);

    let mut waveform_progress = Vec::new();
    let waveform = engine
        .generate_waveform_with_progress(media_arg, 2, info.duration, |progress| {
            waveform_progress.push(progress);
        })
        .expect("generate waveform");
    assert!(!waveform_progress.is_empty());
    assert_eq!(waveform_progress.last().copied(), Some(1.0));
    assert!(waveform_progress.windows(2).all(|pair| pair[0] <= pair[1]));
    assert_eq!(read_u32(&waveform, 0), 0x4257_5631);
    assert_eq!(read_u32(&waveform, 4), 1);
    assert_eq!(read_u32(&waveform, 8), peaks::BLOCKS_PER_SECOND);
    assert!(read_u32(&waveform, 12) > 300);

    let playback_path = engine
        .prepare_audio(&temp.0, 1, media_arg)
        .expect("prepare playback audio");
    let playback = fs::read(&playback_path).expect("read prepared WAV");
    assert!(playback.starts_with(b"RIFF"));
    assert!(playback.len() > 44);

    // Exercise the real stream-copy export with its workspace in the output
    // directory. Seed an existing destination so the test also verifies the
    // final rename replaces a previously confirmed output file.
    let export_dir = temp.0.join("exports");
    fs::create_dir_all(&export_dir).expect("create export directory");
    let export_path = export_dir.join("cut.mkv");
    fs::write(&export_path, b"previous export").expect("seed existing export");
    let report = blaze_engine::export::export_blaze_video(
        &engine,
        &media,
        &export_path,
        info.duration,
        &[(0.4, 0.8)],
    )
    .expect("export cut video");
    assert!(report.actual_duration > 0.0);
    assert!(report.actual_duration < info.duration);
    assert!(fs::metadata(&export_path).expect("read exported file").len() > 1_000);
    assert!(
        fs::read_dir(&export_dir)
            .expect("list export directory")
            .all(|entry| !entry.unwrap().file_name().to_string_lossy().contains("_blaze_export_")),
        "temporary export workspace should be removed"
    );

    let mut command = engine
        .preview_command(media_arg, 0.0, 25.0, 3)
        .expect("build preview command");
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = command.spawn().expect("start preview FFmpeg");
    let stdout = child.stdout.take().expect("capture MJPEG stdout");
    let credits = Credits::new(16);
    let mut jpeg_frames = Vec::new();
    let (count, outcome) = pump(stdout, &credits, |_, jpeg| {
        jpeg_frames.push(jpeg);
        true
    });
    let status = child.wait().expect("wait for preview FFmpeg");

    assert_eq!(outcome, Outcome::Finished);
    assert_eq!(count, 3);
    assert!(status.success());
    assert_eq!(jpeg_frames.len(), 3);
    for jpeg in jpeg_frames {
        assert!(jpeg.starts_with(&[0xFF, 0xD8]));
        assert!(jpeg.ends_with(&[0xFF, 0xD9]));
    }

    // The method accepts a path object rather than assuming UTF-8 for the
    // scratch directory itself.
    assert!(Path::new(&playback_path).exists());
}
