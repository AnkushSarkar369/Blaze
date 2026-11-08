//! Splits a concatenated MJPEG byte stream (FFmpeg's `image2pipe`) into frames.
//!
//! std-only so it can be unit-tested without the Tauri toolchain
//! (`rustc --test src/frames.rs`). Frames are found by walking JPEG marker
//! segments rather than searching for `FF D9`, so a stray `FF D9` inside a
//! segment payload cannot cut a frame short.

enum Scan {
    /// A whole frame occupies `buf[..end]`.
    Complete(usize),
    /// More bytes are needed.
    Incomplete,
    /// The data does not start with a valid frame.
    Invalid,
}

fn scan_frame(buf: &[u8]) -> Scan {
    if buf.len() < 2 {
        return Scan::Incomplete;
    }
    if buf[0] != 0xFF || buf[1] != 0xD8 {
        return Scan::Invalid;
    }

    let mut pos = 2;
    loop {
        if pos + 1 >= buf.len() {
            return Scan::Incomplete;
        }
        if buf[pos] != 0xFF {
            return Scan::Invalid;
        }
        // Any number of 0xFF fill bytes may precede a marker code.
        let mut marker = buf[pos + 1];
        while marker == 0xFF {
            pos += 1;
            if pos + 1 >= buf.len() {
                return Scan::Incomplete;
            }
            marker = buf[pos + 1];
        }

        match marker {
            0xD9 => return Scan::Complete(pos + 2),
            0x00 => return Scan::Invalid,
            // SOI, TEM and RSTn carry no length field.
            0xD8 | 0x01 | 0xD0..=0xD7 => pos += 2,
            _ => {
                if pos + 4 > buf.len() {
                    return Scan::Incomplete;
                }
                let length = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]) as usize;
                if length < 2 {
                    return Scan::Invalid;
                }
                let segment_end = pos + 2 + length;
                if segment_end > buf.len() {
                    return Scan::Incomplete;
                }
                pos = segment_end;

                if marker == 0xDA {
                    // Entropy-coded data runs until the next real marker.
                    // FF 00 is a stuffed data byte and FF D0..D7 are restarts.
                    loop {
                        if pos + 1 >= buf.len() {
                            return Scan::Incomplete;
                        }
                        if buf[pos] != 0xFF {
                            pos += 1;
                            continue;
                        }
                        match buf[pos + 1] {
                            0x00 | 0xD0..=0xD7 => pos += 2,
                            0xFF => pos += 1,
                            _ => break,
                        }
                    }
                }
            }
        }
    }
}

#[derive(Default)]
pub struct FrameSplitter {
    buf: Vec<u8>,
}

impl FrameSplitter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed more bytes; returns every frame completed by them, in order.
    pub fn push(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        self.buf.extend_from_slice(data);
        let mut frames = Vec::new();

        loop {
            // Align to the next start-of-image marker, dropping any garbage.
            match self.buf.windows(2).position(|pair| pair == [0xFF, 0xD8]) {
                Some(0) => {}
                Some(offset) => {
                    self.buf.drain(..offset);
                }
                None => {
                    // Keep a trailing 0xFF: it may be the first half of a marker.
                    let keep = usize::from(self.buf.last() == Some(&0xFF));
                    let drop = self.buf.len() - keep;
                    self.buf.drain(..drop);
                    break;
                }
            }

            match scan_frame(&self.buf) {
                Scan::Complete(end) => {
                    let rest = self.buf.split_off(end);
                    frames.push(std::mem::replace(&mut self.buf, rest));
                }
                Scan::Incomplete => break,
                // Skip this SOI and resynchronise on the next one.
                Scan::Invalid => {
                    self.buf.drain(..2);
                }
            }
        }

        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A structurally valid JPEG: SOI, APP0, DQT, SOS, entropy data containing a
    /// stuffed FF 00, a restart marker and a *payload* FF D9 pair, then EOI.
    fn fake_jpeg(seed: u8) -> Vec<u8> {
        let mut jpeg = vec![0xFF, 0xD8];
        // APP0 with a payload that contains FF D9 (must not end the frame).
        jpeg.extend_from_slice(&[0xFF, 0xE0, 0x00, 0x08, 0xFF, 0xD9, seed, 0x00, 0x01, 0x02]);
        // DQT
        jpeg.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x05, 0x01, 0x02, 0x03]);
        // SOS header (length 4 -> 2 payload bytes)
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x04, 0x11, 0x22]);
        // Entropy data
        jpeg.extend_from_slice(&[seed, 0x10, 0xFF, 0x00, 0x20, 0xFF, 0xD0, 0x30, 0x40]);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    fn collect(chunks: impl Iterator<Item = Vec<u8>>) -> Vec<Vec<u8>> {
        let mut splitter = FrameSplitter::new();
        let mut frames = Vec::new();
        for chunk in chunks {
            frames.extend(splitter.push(&chunk));
        }
        frames
    }

    #[test]
    fn splits_back_to_back_frames() {
        let (a, b, c) = (fake_jpeg(1), fake_jpeg(2), fake_jpeg(3));
        let stream: Vec<u8> = [a.clone(), b.clone(), c.clone()].concat();
        let frames = collect(std::iter::once(stream));
        assert_eq!(frames, vec![a, b, c]);
    }

    #[test]
    fn any_chunking_yields_the_same_frames() {
        let expected = vec![fake_jpeg(10), fake_jpeg(11), fake_jpeg(12)];
        let stream: Vec<u8> = expected.concat();
        for size in 1..=stream.len() {
            let frames = collect(stream.chunks(size).map(|chunk| chunk.to_vec()));
            assert_eq!(frames, expected, "chunk size {size}");
        }
    }

    #[test]
    fn payload_ff_d9_does_not_end_a_frame() {
        let frame = fake_jpeg(7);
        let frames = collect(std::iter::once(frame.clone()));
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], frame);
    }

    #[test]
    fn leading_garbage_is_skipped() {
        let frame = fake_jpeg(4);
        let mut stream = vec![0x00, 0x12, 0xFF, 0x00, 0x55];
        stream.extend_from_slice(&frame);
        let frames = collect(std::iter::once(stream));
        assert_eq!(frames, vec![frame]);
    }

    #[test]
    fn truncated_frame_is_not_emitted_until_complete() {
        let frame = fake_jpeg(9);
        let mut splitter = FrameSplitter::new();
        assert!(splitter.push(&frame[..frame.len() - 1]).is_empty());
        assert_eq!(splitter.push(&frame[frame.len() - 1..]), vec![frame]);
    }

    #[test]
    fn marker_split_across_pushes_is_handled() {
        // The SOI marker itself is split between two pushes.
        let frame = fake_jpeg(5);
        let mut splitter = FrameSplitter::new();
        assert!(splitter.push(&[0x00, 0x00, 0xFF]).is_empty());
        assert_eq!(splitter.push(&frame[1..]), vec![frame]);
    }
}
