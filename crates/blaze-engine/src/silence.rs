//! Adaptive, amplitude-based silence detection.
use std::io::Read;

const SAMPLE_RATE: u32 = 24_000;
const WINDOW_MS: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetectionLevel {
    Comfortable,
    Optimal,
    Default,
    Tight,
    Aggressive,
}

impl DetectionLevel {
    pub fn threshold_below_baseline_db(self) -> f64 {
        match self {
            Self::Comfortable => 32.0,
            Self::Optimal => 26.0,
            Self::Default => 22.0,
            Self::Tight => 18.0,
            Self::Aggressive => 14.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaddingPreset {
    Comfortable,
    Normal,
    Tight,
}

impl PaddingPreset {
    pub fn milliseconds(self) -> u32 {
        match self {
            Self::Comfortable => 120,
            Self::Normal => 60,
            Self::Tight => 20,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SilenceDetectionConfig {
    pub level: DetectionLevel,
    pub padding_before_ms: u32,
    pub padding_after_ms: u32,
    pub minimum_duration_ms: u32,
    /// Optional advanced override for the adaptive threshold offset.
    pub threshold_below_baseline_db: Option<f64>,
    /// Optional advanced peak ceiling in dBFS. None disables peak gating.
    pub peak_ceiling_dbfs: Option<f64>,
}

impl Default for SilenceDetectionConfig {
    fn default() -> Self {
        Self {
            level: DetectionLevel::Default,
            padding_before_ms: PaddingPreset::Normal.milliseconds(),
            padding_after_ms: PaddingPreset::Normal.milliseconds(),
            minimum_duration_ms: 300,
            threshold_below_baseline_db: None,
            peak_ceiling_dbfs: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SilenceInterval {
    pub start: f64,
    pub end: f64,
}

#[derive(Clone, Copy)]
struct Window {
    rms_dbfs: f64,
    peak_dbfs: f64,
}

fn dbfs(value: f64) -> f64 {
    if value <= 0.0 { -120.0 } else { (value / 32768.0).log10() * 20.0 }
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    if sorted.is_empty() { return -120.0; }
    let index = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}

/// Analyse mono signed 16-bit PCM and return silence candidates in seconds.
/// Short windows are merged into contiguous intervals before duration filtering.
pub fn detect_silence_pcm(
    samples: &[i16],
    config: SilenceDetectionConfig,
) -> Vec<SilenceInterval> {
    if samples.is_empty() { return Vec::new(); }
    let window_samples = (SAMPLE_RATE as usize * WINDOW_MS) / 1000;
    let mut windows = Vec::with_capacity(samples.len() / window_samples + 1);
    for chunk in samples.chunks(window_samples) {
        let squares: f64 = chunk.iter().map(|sample| (*sample as f64).powi(2)).sum();
        let peak = chunk.iter().map(|sample| (*sample as f64).abs()).fold(0.0, f64::max);
        windows.push(Window {
            rms_dbfs: dbfs((squares / chunk.len() as f64).sqrt()),
            peak_dbfs: dbfs(peak),
        });
    }

    let mut levels: Vec<f64> = windows.iter().map(|window| window.rms_dbfs).collect();
    levels.sort_by(f64::total_cmp);
    // The upper-middle distribution estimates normal programme loudness while
    // remaining robust to long silent sections.
    let baseline = percentile(&levels, 0.85);
    let offset = config.threshold_below_baseline_db
        .unwrap_or_else(|| config.level.threshold_below_baseline_db())
        .clamp(3.0, 60.0);
    let threshold = (baseline - offset).max(-90.0);
    let peak_ceiling = config.peak_ceiling_dbfs;

    let mut intervals = Vec::new();
    let mut run_start: Option<usize> = None;
    for (index, window) in windows.iter().enumerate() {
        let quiet = window.rms_dbfs <= threshold
            && peak_ceiling.is_none_or(|ceiling| window.peak_dbfs <= ceiling);
        if quiet {
            run_start.get_or_insert(index);
        } else if let Some(start) = run_start.take() {
            intervals.push((start, index));
        }
    }
    if let Some(start) = run_start {
        intervals.push((start, windows.len()));
    }

    let total_duration = samples.len() as f64 / SAMPLE_RATE as f64;
    let min_duration = config.minimum_duration_ms as f64 / 1000.0;
    let before = config.padding_before_ms as f64 / 1000.0;
    let after = config.padding_after_ms as f64 / 1000.0;

    intervals.into_iter().filter_map(|(start, end)| {
        let raw_start = start as f64 * WINDOW_MS as f64 / 1000.0;
        let raw_end = (end * WINDOW_MS) as f64 / 1000.0;
        if raw_end - raw_start < min_duration { return None; }
        // Padding preserves audio around a detected quiet interval. The cut
        // therefore begins earlier and ends later than the raw silence.
        let padded_start = (raw_start - before).max(0.0);
        let padded_end = (raw_end + after).min(total_duration);
        (padded_end > padded_start).then_some(SilenceInterval {
            start: padded_start,
            end: padded_end,
        })
    }).collect()
}

/// Decode the first audio stream to mono 24 kHz PCM and run silence analysis.
pub fn detect_silence(
    engine: &crate::MediaEngine,
    path: &str,
    config: SilenceDetectionConfig,
) -> Result<Vec<SilenceInterval>, String> {
    let rate = SAMPLE_RATE.to_string();
    let mut child = engine.command("ffmpeg")?
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-i", path,
            "-map", "0:a:0", "-vn", "-ac", "1", "-ar", rate.as_str(),
            "-f", "s16le", "pipe:1"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn().map_err(|error| format!("Could not start FFmpeg: {error}"))?;
    let mut stdout = child.stdout.take().ok_or_else(|| "Could not read decoded audio.".to_string())?;
    let mut bytes = Vec::new();
    stdout.read_to_end(&mut bytes).map_err(|error| {
        let _ = child.kill();
        format!("Could not read decoded audio: {error}")
    })?;
    let status = child.wait().map_err(|error| format!("Could not finish audio analysis: {error}"))?;
    if !status.success() { return Err("FFmpeg could not decode the audio stream.".to_string()); }
    let samples: Vec<i16> = bytes.chunks_exact(2)
        .map(|pair| i16::from_le_bytes([pair[0], pair[1]])).collect();
    Ok(detect_silence_pcm(&samples, config))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(value: i16, duration_ms: usize) -> Vec<i16> {
        vec![value; SAMPLE_RATE as usize * duration_ms / 1000]
    }

    #[test]
    fn detects_quiet_interval_between_loud_sections() {
        let mut samples = constant(12000, 1000);
        samples.extend(constant(0, 800));
        samples.extend(constant(12000, 1000));
        let mut config = SilenceDetectionConfig::default();
        config.padding_before_ms = 0;
        config.padding_after_ms = 0;
        let found = detect_silence_pcm(&samples, config);
        assert_eq!(found.len(), 1);
        assert!((found[0].start - 1.0).abs() < 0.02);
        assert!((found[0].end - 1.8).abs() < 0.02);
    }

    #[test]
    fn minimum_duration_filters_short_gaps() {
        let mut samples = constant(12000, 1000);
        samples.extend(constant(0, 200));
        samples.extend(constant(12000, 1000));
        let found = detect_silence_pcm(&samples, SilenceDetectionConfig::default());
        assert!(found.is_empty());
    }

    #[test]
    fn padding_is_applied_to_both_sides() {
        let mut samples = constant(12000, 1000);
        samples.extend(constant(0, 1000));
        samples.extend(constant(12000, 1000));
        let mut config = SilenceDetectionConfig::default();
        config.padding_before_ms = 100;
        config.padding_after_ms = 150;
        let found = detect_silence_pcm(&samples, config);
        assert_eq!(found.len(), 1);
        assert!((found[0].start - 0.9).abs() < 0.02);
        assert!((found[0].end - 2.15).abs() < 0.02);
    }
}
