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
    env::split_paths(&env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}

#[test]
fn real_ffmpeg_probe_keyframes_waveform_audio_and_mjpeg_stream() {
    let Some(ffmpeg) = find_executable("ffmpeg") else {
        eprintln!("Skipping real-FFmpeg integration test: ffmpeg is not on PATH");
        return;
    };
    let Some(ffprobe) = find_executable("ffprobe") else {
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

    let waveform = engine
        .generate_waveform(media_arg, 2)
        .expect("generate waveform");
    assert_eq!(read_u32(&waveform, 0), 0x4257_5631);
    assert_eq!(read_u32(&waveform, 4), 1);
    assert_eq!(read_u32(&waveform, 8), peaks::BLOCKS_PER_SECOND);
    assert!(read_u32(&waveform, 12) > 1_000);

    let playback_path = engine
        .prepare_audio(&temp.0, 1, media_arg)
        .expect("prepare playback audio");
    let playback = fs::read(&playback_path).expect("read prepared WAV");
    assert!(playback.starts_with(b"RIFF"));
    assert!(playback.len() > 44);

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
