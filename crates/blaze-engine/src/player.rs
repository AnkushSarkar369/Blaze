//! Preview-player streaming core: FFmpeg argument building, credit-based flow
//! control and the pump that turns FFmpeg's MJPEG pipe into numbered frames.
//!
//! std-only (plus `frames.rs`) so it can be tested against a real FFmpeg without
//! the Tauri toolchain.
//!
//! Wire format of every message sent to the webview (little-endian):
//!
//! ```text
//! u32 index  then the JPEG bytes          -> a frame
//! u32 0xFFFFFFFF                          -> end of stream
//! u32 0xFFFFFFFE then UTF-8 message       -> error
//! ```
//!
//! Frame `index` N is shown at `start + N / fps` seconds. FFmpeg's `fps` filter
//! produces a constant frame rate (duplicating or dropping as needed), so the
//! index alone is a timestamp.

use crate::frames::FrameSplitter;
use std::{
    ffi::OsString,
    io::Read,
    path::Path,
    sync::{Condvar, Mutex},
};

pub const END_MARKER: u32 = 0xFFFF_FFFF;
pub const ERROR_MARKER: u32 = 0xFFFF_FFFE;

/// Frames FFmpeg may run ahead of what the webview has consumed.
pub const INITIAL_CREDITS: i64 = 16;

pub const MAX_PREVIEW_WIDTH: u32 = 1280;
pub const MAX_PREVIEW_HEIGHT: u32 = 720;

pub fn encode_frame(index: u32, jpeg: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(4 + jpeg.len());
    message.extend_from_slice(&index.to_le_bytes());
    message.extend_from_slice(jpeg);
    message
}

pub fn encode_marker(marker: u32, text: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(4 + text.len());
    message.extend_from_slice(&marker.to_le_bytes());
    message.extend_from_slice(text.as_bytes());
    message
}

/// Arguments for an FFmpeg run that streams preview frames from `start`.
/// `max_frames == 0` means "until the end of the file".
pub fn video_args(path: &Path, start: f64, fps: f64, max_frames: u32) -> Vec<OsString> {
    let fps = if fps.is_finite() { fps.clamp(1.0, 60.0) } else { 30.0 };
    let start = if start.is_finite() { start.max(0.0) } else { 0.0 };
    let filter = format!(
        "fps=fps={fps:.6}:start_time=0,\
         scale=w='min({MAX_PREVIEW_WIDTH},iw)':h='min({MAX_PREVIEW_HEIGHT},ih)':\
         force_original_aspect_ratio=decrease:force_divisible_by=2,\
         format=yuvj420p"
    );

    let mut args: Vec<OsString> = [
        "-hide_banner",
        "-nostdin",
        "-loglevel",
        "error",
        "-ss",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    args.push(format!("{start:.3}").into());
    args.push("-i".into());
    args.push(path.as_os_str().to_os_string());
    args.extend(
        ["-map", "0:v:0", "-an", "-sn", "-dn", "-vf"]
            .iter()
            .map(OsString::from),
    );
    args.push(filter);
    if max_frames > 0 {
        args.push("-frames:v".into());
        args.push(max_frames.to_string().into());
    }
    args.extend(
        ["-c:v", "mjpeg", "-q:v", "4", "-f", "image2pipe", "pipe:1"]
            .iter()
            .map(OsString::from),
    );
    args
}

struct CreditState {
    available: i64,
    cancelled: bool,
}

/// Counting semaphore with cancellation. The webview returns one credit per
/// frame it has consumed, so FFmpeg is throttled by backpressure on its pipe
/// instead of decoding the whole file as fast as it can.
pub struct Credits {
    state: Mutex<CreditState>,
    wake: Condvar,
}

impl Credits {
    pub fn new(initial: i64) -> Self {
        Self {
            state: Mutex::new(CreditState { available: initial, cancelled: false }),
            wake: Condvar::new(),
        }
    }

    /// Blocks until a credit is available. Returns false once cancelled.
    pub fn acquire(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        while state.available <= 0 && !state.cancelled {
            state = self.wake.wait(state).unwrap();
        }
        if state.cancelled {
            return false;
        }
        state.available -= 1;
        true
    }

    pub fn release(&self, count: i64) {
        let mut state = self.state.lock().unwrap();
        state.available += count.max(0);
        self.wake.notify_all();
    }

    pub fn cancel(&self) {
        let mut state = self.state.lock().unwrap();
        state.cancelled = true;
        self.wake.notify_all();
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.lock().unwrap().cancelled
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The source reached end of file.
    Finished,
    /// Cancelled, or the receiver went away.
    Stopped,
}

/// Reads MJPEG from `source`, splits it into frames and passes each, numbered
/// from 0, to `emit` once a credit is available. `emit` returns false when the
/// receiver is gone. Returns how many frames were emitted.
pub fn pump<R: Read>(
    mut source: R,
    credits: &Credits,
    mut emit: impl FnMut(u32, Vec<u8>) -> bool,
) -> (u32, Outcome) {
    let mut splitter = FrameSplitter::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    let mut index: u32 = 0;

    loop {
        let count = match source.read(&mut buffer) {
            Ok(0) => return (index, Outcome::Finished),
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            // A broken pipe means FFmpeg was killed; treat it like end of data.
            Err(_) => return (index, Outcome::Finished),
        };

        for frame in splitter.push(&buffer[..count]) {
            if !credits.acquire() {
                return (index, Outcome::Stopped);
            }
            if !emit(index, frame) {
                return (index, Outcome::Stopped);
            }
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    fn fake_jpeg(seed: u8) -> Vec<u8> {
        vec![
            0xFF, 0xD8, 0xFF, 0xDA, 0x00, 0x04, 0x11, 0x22, seed, 0x10, 0xFF, 0x00, 0x20, 0xFF,
            0xD9,
        ]
    }

    #[test]
    fn pump_numbers_frames_from_zero() {
        let stream: Vec<u8> = (0..5).flat_map(fake_jpeg).collect();
        let credits = Credits::new(100);
        let mut seen = Vec::new();
        let (count, outcome) = pump(Cursor::new(stream), &credits, |index, frame| {
            seen.push((index, frame));
            true
        });
        assert_eq!(count, 5);
        assert_eq!(outcome, Outcome::Finished);
        for (position, (index, frame)) in seen.iter().enumerate() {
            assert_eq!(*index, position as u32);
            assert_eq!(*frame, fake_jpeg(position as u8));
        }
    }

    #[test]
    fn pump_stops_when_receiver_is_gone() {
        let stream: Vec<u8> = (0..5).flat_map(fake_jpeg).collect();
        let credits = Credits::new(100);
        let mut calls = 0;
        let (count, outcome) = pump(Cursor::new(stream), &credits, |_, _| {
            calls += 1;
            calls < 3
        });
        assert_eq!(outcome, Outcome::Stopped);
        assert_eq!(count, 2);
    }

    #[test]
    fn pump_blocks_without_credits_and_resumes_on_release() {
        let stream: Vec<u8> = (0..6).flat_map(fake_jpeg).collect();
        let credits = Arc::new(Credits::new(2));
        let emitted = Arc::new(Mutex::new(Vec::new()));

        let worker = {
            let credits = credits.clone();
            let emitted = emitted.clone();
            thread::spawn(move || {
                pump(Cursor::new(stream), &credits, |index, _| {
                    emitted.lock().unwrap().push(index);
                    true
                })
            })
        };

        thread::sleep(Duration::from_millis(150));
        assert_eq!(*emitted.lock().unwrap(), vec![0, 1], "must stall at 2 credits");

        credits.release(4);
        let (count, outcome) = worker.join().unwrap();
        assert_eq!((count, outcome), (6, Outcome::Finished));
        assert_eq!(emitted.lock().unwrap().len(), 6);
    }

    #[test]
    fn cancel_wakes_a_blocked_pump() {
        let stream: Vec<u8> = (0..4).flat_map(fake_jpeg).collect();
        let credits = Arc::new(Credits::new(1));
        let worker = {
            let credits = credits.clone();
            thread::spawn(move || pump(Cursor::new(stream), &credits, |_, _| true))
        };
        thread::sleep(Duration::from_millis(100));
        credits.cancel();
        let (count, outcome) = worker.join().unwrap();
        assert_eq!((count, outcome), (1, Outcome::Stopped));
        assert!(credits.is_cancelled());
    }

    #[test]
    fn args_are_well_formed() {
        let args = video_args(Path::new("/tmp/a b.mp4"), 12.3456, 29.97, 1);
        let position = |needle: &str| args.iter().position(|arg| arg == needle).unwrap();
        assert_eq!(args[position("-ss") + 1], "12.346");
        assert_eq!(args[position("-i") + 1], "/tmp/a b.mp4");
        assert_eq!(args[position("-frames:v") + 1], "1");
        assert!(args[position("-vf") + 1].to_string_lossy().starts_with("fps=fps=29.970000:start_time=0,"));
        assert_eq!(args.last().unwrap(), "pipe:1");

        let unlimited = video_args(Path::new("x.mp4"), -4.0, f64::NAN, 0);
        assert!(!unlimited.iter().any(|arg| arg == "-frames:v"));
        assert_eq!(unlimited[unlimited.iter().position(|a| a == "-ss").unwrap() + 1], "0.000");
        assert!(unlimited.iter().any(|arg| arg.to_string_lossy().starts_with("fps=fps=30.000000")));
    }

    #[test]
    fn encoded_messages_carry_their_header() {
        let frame = encode_frame(7, &[1, 2, 3]);
        assert_eq!(&frame[..4], &7_u32.to_le_bytes());
        assert_eq!(&frame[4..], &[1, 2, 3]);
        let marker = encode_marker(ERROR_MARKER, "boom");
        assert_eq!(&marker[..4], &ERROR_MARKER.to_le_bytes());
        assert_eq!(&marker[4..], b"boom");
    }
}
