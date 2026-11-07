//! Waveform peak summaries: min / max / RMS for fixed-size blocks of samples.
//!
//! This module only uses `std`, so it can be unit-tested without the Tauri
//! toolchain (`rustc --test src/peaks.rs`).
//!
//! Wire format returned to the frontend (all little-endian):
//!
//! ```text
//! u32 magic ("BWV1")  u32 channels  u32 blocks_per_second  u32 blocks
//! then, for every channel in order:
//!     i16 min[blocks]   i16 max[blocks]   u16 rms[blocks]
//! ```
//!
//! Every plane starts on a 2-byte boundary, so the frontend can view each one
//! directly as a typed array without copying.

pub const SAMPLE_RATE: u32 = 24_000;
/// 24 samples at 24 kHz = 1 ms per block, i.e. 1000 blocks per second. That is
/// finer than one screen pixel even at the maximum timeline zoom.
pub const BLOCK_SAMPLES: usize = 24;
pub const BLOCKS_PER_SECOND: u32 = SAMPLE_RATE / BLOCK_SAMPLES as u32;

const MAGIC: u32 = 0x4257_5631;

struct Channel {
    min: Vec<i16>,
    max: Vec<i16>,
    rms: Vec<u16>,
    current_min: i16,
    current_max: i16,
    current_squares: u64,
}

impl Channel {
    fn new() -> Self {
        Self {
            min: Vec::new(),
            max: Vec::new(),
            rms: Vec::new(),
            current_min: i16::MAX,
            current_max: i16::MIN,
            current_squares: 0,
        }
    }

    fn add(&mut self, sample: i16) {
        self.current_min = self.current_min.min(sample);
        self.current_max = self.current_max.max(sample);
        self.current_squares += (sample as i64 * sample as i64) as u64;
    }

    fn flush(&mut self, count: usize) {
        self.min.push(self.current_min);
        self.max.push(self.current_max);
        let mean_square = self.current_squares as f64 / count as f64;
        self.rms.push(mean_square.sqrt().round().min(u16::MAX as f64) as u16);
        self.current_min = i16::MAX;
        self.current_max = i16::MIN;
        self.current_squares = 0;
    }
}

pub struct PeakBuilder {
    channels: Vec<Channel>,
    samples_in_block: usize,
    leftover: Vec<u8>,
}

pub struct Peaks {
    channels: Vec<Channel>,
}

impl PeakBuilder {
    pub fn new(channels: usize) -> Self {
        assert!(channels > 0, "at least one channel is required");
        Self {
            channels: (0..channels).map(|_| Channel::new()).collect(),
            samples_in_block: 0,
            leftover: Vec::new(),
        }
    }

    /// Feed interleaved signed 16-bit little-endian PCM. Chunks may end in the
    /// middle of a sample or frame; the remainder is carried to the next call.
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let frame_bytes = self.channels.len() * 2;
        self.leftover.extend_from_slice(bytes);
        let usable = self.leftover.len() / frame_bytes * frame_bytes;
        if usable == 0 {
            return;
        }

        let data = std::mem::take(&mut self.leftover);
        for frame in data[..usable].chunks_exact(frame_bytes) {
            for (channel, sample) in self.channels.iter_mut().zip(frame.chunks_exact(2)) {
                channel.add(i16::from_le_bytes([sample[0], sample[1]]));
            }
            self.samples_in_block += 1;
            if self.samples_in_block == BLOCK_SAMPLES {
                for channel in &mut self.channels {
                    channel.flush(BLOCK_SAMPLES);
                }
                self.samples_in_block = 0;
            }
        }
        self.leftover = data[usable..].to_vec();
    }

    pub fn finish(mut self) -> Peaks {
        if self.samples_in_block > 0 {
            let count = self.samples_in_block;
            for channel in &mut self.channels {
                channel.flush(count);
            }
        }
        Peaks { channels: self.channels }
    }
}

impl Peaks {
    pub fn blocks(&self) -> usize {
        self.channels[0].min.len()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let blocks = self.blocks();
        let mut out = Vec::with_capacity(16 + self.channels.len() * blocks * 6);
        out.extend_from_slice(&MAGIC.to_le_bytes());
        out.extend_from_slice(&(self.channels.len() as u32).to_le_bytes());
        out.extend_from_slice(&BLOCKS_PER_SECOND.to_le_bytes());
        out.extend_from_slice(&(blocks as u32).to_le_bytes());
        for channel in &self.channels {
            for value in &channel.min {
                out.extend_from_slice(&value.to_le_bytes());
            }
            for value in &channel.max {
                out.extend_from_slice(&value.to_le_bytes());
            }
            for value in &channel.rms {
                out.extend_from_slice(&value.to_le_bytes());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pcm(samples: &[i16]) -> Vec<u8> {
        samples.iter().flat_map(|sample| sample.to_le_bytes()).collect()
    }

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    fn read_i16(bytes: &[u8], offset: usize) -> i16 {
        i16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    fn read_u16(bytes: &[u8], offset: usize) -> u16 {
        u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
    }

    #[test]
    fn square_wave_has_expected_peaks_and_rms() {
        // One second of a +/-16000 square wave, alternating every sample.
        let samples: Vec<i16> = (0..SAMPLE_RATE as usize)
            .map(|index| if index % 2 == 0 { 16000 } else { -16000 })
            .collect();
        let mut builder = PeakBuilder::new(1);
        builder.push_bytes(&pcm(&samples));
        let peaks = builder.finish();
        assert_eq!(peaks.blocks(), BLOCKS_PER_SECOND as usize);
        let bytes = peaks.to_bytes();
        let blocks = peaks.blocks();
        for block in 0..blocks {
            assert_eq!(read_i16(&bytes, 16 + block * 2), -16000);
            assert_eq!(read_i16(&bytes, 16 + blocks * 2 + block * 2), 16000);
            assert_eq!(read_u16(&bytes, 16 + blocks * 4 + block * 2), 16000);
        }
    }

    #[test]
    fn header_and_size_are_consistent() {
        let mut builder = PeakBuilder::new(2);
        builder.push_bytes(&pcm(&[0; 2 * 100]));
        let peaks = builder.finish();
        let bytes = peaks.to_bytes();
        assert_eq!(read_u32(&bytes, 0), MAGIC);
        assert_eq!(read_u32(&bytes, 4), 2);
        assert_eq!(read_u32(&bytes, 8), BLOCKS_PER_SECOND);
        let blocks = read_u32(&bytes, 12) as usize;
        // 100 samples = 4 full blocks + a partial block of 4.
        assert_eq!(blocks, 5);
        assert_eq!(bytes.len(), 16 + 2 * blocks * 6);
    }

    #[test]
    fn arbitrary_chunking_gives_identical_output() {
        let samples: Vec<i16> = (0..5000)
            .map(|index| ((index as f64 * 0.05).sin() * 20000.0) as i16)
            .collect();
        let raw = pcm(&samples);

        let mut whole = PeakBuilder::new(1);
        whole.push_bytes(&raw);
        let expected = whole.finish().to_bytes();

        for chunk_size in [1_usize, 3, 7, 64, 1001] {
            let mut builder = PeakBuilder::new(1);
            for chunk in raw.chunks(chunk_size) {
                builder.push_bytes(chunk);
            }
            assert_eq!(builder.finish().to_bytes(), expected, "chunk size {chunk_size}");
        }
    }

    #[test]
    fn stereo_channels_are_kept_separate() {
        // Left channel loud, right channel silent.
        let mut samples = Vec::new();
        for index in 0..240 {
            samples.push(if index % 2 == 0 { 30000 } else { -30000 });
            samples.push(0);
        }
        let mut builder = PeakBuilder::new(2);
        builder.push_bytes(&pcm(&samples));
        let peaks = builder.finish();
        let bytes = peaks.to_bytes();
        let blocks = peaks.blocks();
        let plane = blocks * 6;
        assert_eq!(read_i16(&bytes, 16 + blocks * 2), 30000); // L max
        assert_eq!(read_i16(&bytes, 16 + plane), 0); // R min
        assert_eq!(read_i16(&bytes, 16 + plane + blocks * 2), 0); // R max
        assert_eq!(read_u16(&bytes, 16 + plane + blocks * 4), 0); // R rms
    }

    #[test]
    fn full_scale_negative_sample_does_not_overflow() {
        let mut builder = PeakBuilder::new(1);
        builder.push_bytes(&pcm(&[i16::MIN; BLOCK_SAMPLES]));
        let peaks = builder.finish();
        let bytes = peaks.to_bytes();
        assert_eq!(read_i16(&bytes, 16), i16::MIN);
        assert_eq!(read_u16(&bytes, 16 + 4), 32768);
    }
}
