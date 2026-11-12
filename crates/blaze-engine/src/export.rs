//! Stream-copy video export and NLE timeline interchange formats.
//! Cut regions are DELETE ranges; exporters build the complementary KEEP ranges.

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::MetadataExt};

use crate::{output_tracked, MediaEngine, MediaInfo};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InterchangeFormat { DavinciResolve, Edl, Fcpxml, OpenTimelineIo, PremiereXml }

#[derive(Clone, Debug)]
pub struct ExportReport {
    pub output_path: PathBuf,
    pub expected_duration: f64,
    pub actual_duration: f64,
    pub warning: Option<String>,
}

#[derive(Clone, Copy, Debug)]
struct FrameRate { numerator: u32, denominator: u32, nominal: u32, ntsc: bool }

#[derive(Clone, Copy, Debug)]
struct Segment { source_in: i64, source_out: i64, record_in: i64 }

impl Segment { fn duration(self) -> i64 { self.source_out - self.source_in } }

struct Workspace(PathBuf);
impl Drop for Workspace {
    fn drop(&mut self) { let _ = fs::remove_dir_all(&self.0); }
}

fn frame_rate(fps: f64) -> Result<FrameRate, String> {
    if !fps.is_finite() || fps <= 0.0 {
        return Err("The source frame rate could not be determined.".into());
    }
    let common = [
        (24000, 1001, 24, true), (30000, 1001, 30, true),
        (60000, 1001, 60, true), (120000, 1001, 120, true),
        (24, 1, 24, false), (25, 1, 25, false), (30, 1, 30, false),
        (48, 1, 48, false), (50, 1, 50, false), (60, 1, 60, false),
        (100, 1, 100, false), (120, 1, 120, false),
    ];
    if let Some((num, den, nominal, ntsc)) = common.iter().copied()
        .min_by(|a, b| {
            let af = a.0 as f64 / a.1 as f64;
            let bf = b.0 as f64 / b.1 as f64;
            (af - fps).abs().total_cmp(&(bf - fps).abs())
        })
        .filter(|(num, den, _, _)| (fps - (*num as f64 / *den as f64)).abs() < 0.02)
    {
        return Ok(FrameRate { numerator: num, denominator: den, nominal, ntsc });
    }
    Err(format!("The source frame rate ({fps:.6} fps) is not a supported constant frame rate for frame-accurate export."))
}

fn seconds_to_frames(seconds: f64, rate: FrameRate) -> i64 {
    (seconds.max(0.0) * rate.numerator as f64 / rate.denominator as f64).round() as i64
}

fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 { let r = a % b; a = b; b = r; }
    a.max(1)
}

fn frame_time(frames: i64, rate: FrameRate) -> String {
    let numerator = (frames.max(0) as u64).saturating_mul(rate.denominator as u64);
    let denominator = rate.numerator as u64;
    let divisor = gcd(numerator, denominator);
    format!("{}/{}s", numerator / divisor, denominator / divisor)
}

fn timecode(frames: i64, rate: FrameRate) -> String {
    let frames = frames.max(0) as u64;
    let fps = rate.nominal as u64;
    let total_seconds = frames / fps;
    format!("{:02}:{:02}:{:02}:{:02}",
        total_seconds / 3600, (total_seconds / 60) % 60, total_seconds % 60, frames % fps)
}

fn xml_escape(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
        .replace('"', "&quot;").replace('\'', "&apos;")
}

/// Convert a filesystem path into a percent-encoded file URL for NLE interchange
/// and libmpv. On Unix, encode the original path bytes rather than a lossy UTF-8
/// conversion so non-UTF-8 filenames remain addressable.
pub fn file_url(path: &Path) -> String {
    let absolute = if path.is_absolute() { path.to_path_buf() }
        else { std::env::current_dir().unwrap_or_default().join(path) };

    #[cfg(unix)]
    {
        file_url_from_bytes(absolute.as_os_str().as_bytes())
    }
    #[cfg(not(unix))]
    {
        file_url_from_str(&absolute.to_string_lossy(), cfg!(windows))
    }
}

#[cfg(unix)]
fn file_url_from_bytes(raw: &[u8]) -> String {
    let mut encoded = String::with_capacity(raw.len());
    for byte in raw {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(byte) {
            encoded.push(*byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// Builds a file:// URL from an absolute path string. On Windows, backslashes
/// become forward slashes and extended-length prefixes (\\?\) are removed, so
/// `C:\My Videos\a.mp4` becomes `file:///C:/My%20Videos/a.mp4` and a UNC path
/// `\\server\share\a.mp4` becomes `file://server/share/a.mp4`. On other
/// platforms a backslash is an ordinary file-name character and is encoded.
fn file_url_from_str(raw: &str, windows: bool) -> String {
    let normalised: String = if windows {
        let trimmed = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
            format!(r"\\{rest}")
        } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
            rest.to_string()
        } else {
            raw.to_string()
        };
        trimmed.replace('\\', "/")
    } else {
        raw.to_string()
    };

    let mut encoded = String::with_capacity(normalised.len());
    for byte in normalised.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/-_.~:".contains(byte) {
            encoded.push(*byte as char);
        } else { encoded.push_str(&format!("%{byte:02X}")); }
    }
    if windows && encoded.starts_with("//") {
        // UNC share: the first path component is the host name.
        format!("file:{encoded}")
    } else if encoded.starts_with('/') {
        format!("file://{encoded}")
    } else {
        format!("file:///{encoded}")
    }
}

/// True when both paths refer to the same file, including different spellings
/// (relative vs absolute, symlinks, and letter case or `\\?\` prefixes on Windows).
/// The output file may not exist yet, so its folder is resolved instead.
fn is_same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }

    if let (Ok(a_meta), Ok(b_meta)) = (fs::metadata(a), fs::metadata(b)) {
        #[cfg(unix)]
        {
            if a_meta.dev() == b_meta.dev() && a_meta.ino() == b_meta.ino() {
                return true;
            }
        }
    }

    fn resolve(path: &Path) -> Option<PathBuf> {
        if let Ok(resolved) = fs::canonicalize(path) { return Some(resolved); }
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        Some(fs::canonicalize(parent).ok()?.join(path.file_name()?))
    }
    match (resolve(a), resolve(b)) {
        (Some(a), Some(b)) => if cfg!(windows) {
            a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
        } else { a == b },
        _ => false,
    }
}

fn reel_name(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("BLAZE");
    let clean: String = stem.chars().filter(|c| c.is_ascii_alphanumeric())
        .take(8).map(|c| c.to_ascii_uppercase()).collect();
    if clean.is_empty() { "BLAZE".into() } else { clean }
}

fn merged_deletions(ranges: &[(f64, f64)], duration: f64) -> Vec<(f64, f64)> {
    let mut cuts: Vec<(f64, f64)> = ranges.iter().filter_map(|(start, end)| {
        if !start.is_finite() || !end.is_finite() { return None; }
        let start = start.clamp(0.0, duration);
        let end = end.clamp(0.0, duration);
        (end > start).then_some((start, end))
    }).collect();
    cuts.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for (start, end) in cuts {
        if let Some(last) = merged.last_mut() {
            if start <= last.1 { last.1 = last.1.max(end); continue; }
        }
        merged.push((start, end));
    }
    merged
}

fn keep_segments(ranges: &[(f64, f64)], duration: f64, rate: FrameRate) -> Vec<Segment> {
    let total_frames = seconds_to_frames(duration, rate);
    let cuts = merged_deletions(ranges, duration);
    let mut cursor = 0.0_f64;
    let mut record_cursor = 0_i64;
    let mut segments = Vec::new();
    let mut push_segment = |start: f64, end: f64| {
        let source_in = seconds_to_frames(start, rate).clamp(0, total_frames);
        let source_out = seconds_to_frames(end, rate).clamp(0, total_frames);
        if source_out > source_in {
            segments.push(Segment { source_in, source_out, record_in: record_cursor });
            record_cursor += source_out - source_in;
        }
    };
    for (start, end) in cuts {
        if start > cursor { push_segment(cursor, start); }
        cursor = cursor.max(end);
    }
    if cursor < duration { push_segment(cursor, duration); }
    segments
}

fn make_workspace(parent: &Path, stem: &str) -> Result<Workspace, String> {
    for _ in 0..100 {
        let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(".{stem}_blaze_export_{}_{}", std::process::id(), id));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(Workspace(candidate)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create export workspace: {error}")),
        }
    }
    Err("Could not allocate a unique export workspace.".into())
}

fn run(command: &mut std::process::Command, description: &str) -> Result<(), String> {
    let output = output_tracked(command)
        .map_err(|error| format!("Could not start FFmpeg while {description}: {error}"))?;
    if !output.status.success() {
        return Err(format!("FFmpeg failed while {description}: {}",
            String::from_utf8_lossy(&output.stderr).trim()));
    }
    Ok(())
}

fn probe_duration(engine: &MediaEngine, path: &Path) -> Result<f64, String> {
    let mut command = engine.command("ffprobe")?;
    command
        .args(["-v", "error", "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(path);
    let output = output_tracked(&mut command)
        .map_err(|error| format!("Could not start FFprobe: {error}"))?;
    if !output.status.success() {
        return Err(format!("FFprobe could not verify the exported video: {}",
            String::from_utf8_lossy(&output.stderr).trim()));
    }
    String::from_utf8_lossy(&output.stdout).trim().parse::<f64>()
        .map_err(|_| "FFprobe returned an invalid output duration.".into())
}

/// Stream-copies KEEP ranges from the original source, then concatenates them.
pub fn export_blaze_video(
    engine: &MediaEngine, source: &Path, output: &Path,
    duration: f64, delete_ranges: &[(f64, f64)],
) -> Result<ExportReport, String> {
    if !source.is_file() { return Err("The source video no longer exists.".into()); }
    if !duration.is_finite() || duration <= 0.0 { return Err("The source video has an invalid duration.".into()); }
    if delete_ranges.is_empty() { return Err("There are no cut regions to export.".into()); }
    if is_same_file(source, output) { return Err("The output file cannot replace the source video.".into()); }
    let source_ext = source.extension().and_then(|s| s.to_str()).unwrap_or("mkv");
    let output_ext_check = output.extension().and_then(|s| s.to_str()).unwrap_or(source_ext);
    if !source_ext.eq_ignore_ascii_case(output_ext_check) {
        return Err(format!("Blaze Export preserves the source container. Save the output with the .{source_ext} extension."));
    }

    let rate = frame_rate_for_stream_copy(engine, source)?;
    let segments = keep_segments(delete_ranges, duration, rate);
    if segments.is_empty() {
        return Err("The cut regions remove the entire video; there is nothing to export.".into());
    }

    // Temporary clips live in the output folder (which the user chose and must
    // be able to write to), so the source folder can be read-only and the final
    // file is moved into place with a same-folder rename.
    let output_parent = output.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(output_parent)
        .map_err(|error| format!("Could not access the output folder: {error}"))?;
    let stem = output.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    let workspace = make_workspace(output_parent, stem)?;
    let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("mkv");
    let mut clips = Vec::with_capacity(segments.len());

    for (index, segment) in segments.iter().enumerate() {
        let clip = workspace.0.join(format!("clip_{:04}.{extension}", index + 1));
        let start = segment.source_in as f64 * rate.denominator as f64 / rate.numerator as f64;
        let length = segment.duration() as f64 * rate.denominator as f64 / rate.numerator as f64;
        let start_arg = format!("{start:.9}");
        let length_arg = format!("{length:.9}");
        let mut command = engine.command("ffmpeg")?;
        command
            .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y", "-ss"])
            .arg(&start_arg)
            .arg("-i")
            .arg(source)
            .arg("-t")
            .arg(&length_arg)
            .args(["-map", "0", "-c", "copy", "-avoid_negative_ts", "make_zero"])
            .arg(&clip);
        run(&mut command, &format!("extracting kept segment {}", index + 1))?;
        if !clip.is_file() || fs::metadata(&clip).map(|m| m.len()).unwrap_or(0) == 0 {
            return Err(format!("FFmpeg produced an empty temporary clip for segment {}.", index + 1));
        }
        clips.push(clip);
    }

    let manifest = workspace.0.join("concat.txt");
    let mut manifest_text = String::new();
    for clip in &clips {
        // The concat manifest lives beside these generated ASCII filenames.
        // Relative references avoid encoding the user's output-folder path in
        // FFmpeg's UTF-8 manifest syntax.
        let name = clip.file_name().expect("generated clip has a filename").to_string_lossy()
            .replace('\'', "'\\''");
        manifest_text.push_str(&format!("file '{name}'\n"));
    }
    fs::write(&manifest, manifest_text)
        .map_err(|error| format!("Could not write the temporary concat manifest: {error}"))?;

    let joined = workspace.0.join(format!("joined.{extension}"));
    let mut command = engine.command("ffmpeg")?;
    command
        .args(["-hide_banner", "-nostdin", "-loglevel", "error", "-y",
            "-f", "concat", "-safe", "0", "-i"])
        .arg(&manifest)
        .args(["-map", "0", "-c", "copy"])
        .arg(&joined);
    run(&mut command, "concatenating the kept segments")?;

    let expected_duration = segments.iter().map(|segment|
        segment.duration() as f64 * rate.denominator as f64 / rate.numerator as f64
    ).sum::<f64>();
    let actual_duration = probe_duration(engine, &joined)?;

    if let Err(error) = fs::rename(&joined, output) {
        return Err(format!("Could not move the completed video into place: {error}"));
    }

    let warning = if (actual_duration - expected_duration).abs() > 2.0 {
        Some(format!("The exported duration ({actual_duration:.2}s) differs from expected ({expected_duration:.2}s). Review the result."))
    } else { None };
    Ok(ExportReport { output_path: output.to_path_buf(), expected_duration, actual_duration, warning })
}

fn frame_rate_for_stream_copy(engine: &MediaEngine, source: &Path) -> Result<FrameRate, String> {
    let mut command = engine.command("ffprobe")?;
    command
        .args(["-v", "error", "-select_streams", "v:0",
            "-show_entries", "stream=avg_frame_rate,r_frame_rate",
            "-of", "default=noprint_wrappers=1:nokey=1"])
        .arg(source);
    let output = output_tracked(&mut command)
        .map_err(|error| format!("Could not read the source frame rate: {error}"))?;
    if !output.status.success() { return Err("FFprobe could not read the source frame rate.".into()); }
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.trim().split('/');
        let numerator = parts.next().and_then(|v| v.parse::<f64>().ok());
        let denominator = parts.next().and_then(|v| v.parse::<f64>().ok());
        if let (Some(n), Some(d)) = (numerator, denominator) {
            if d > 0.0 && n > 0.0 { return frame_rate(n / d); }
        }
    }
    Err("The source frame rate could not be determined for stream-copy export.".into())
}

fn atomic_write(output: &Path, contents: &str) -> Result<(), String> {
    let parent = output.parent().filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not access the timeline output folder: {error}"))?;

    let file_name = output.file_name().unwrap_or_else(|| std::ffi::OsStr::new("timeline"));
    for _ in 0..100 {
        let id = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = file_name.to_os_string();
        temporary_name.push(format!(".blaze_tmp_{}_{}", std::process::id(), id));
        let temporary = parent.join(temporary_name);

        let mut file = match OpenOptions::new().write(true).create_new(true).open(&temporary) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create a temporary timeline file: {error}")),
        };

        let write_result = (|| -> std::io::Result<()> {
            file.write_all(contents.as_bytes())?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, output)?;
            #[cfg(unix)]
            if let Ok(directory) = OpenOptions::new().read(true).open(parent) {
                let _ = directory.sync_all();
            }
            Ok(())
        })();

        if let Err(error) = write_result {
            let _ = fs::remove_file(&temporary);
            return Err(format!("Could not write the timeline file: {error}"));
        }
        return Ok(());
    }

    Err("Could not allocate a temporary timeline file.".into())
}

/// Exports the single-source, single-track edit as a timeline interchange file.
pub fn export_interchange(
    format: InterchangeFormat, source: &Path, info: &MediaInfo,
    delete_ranges: &[(f64, f64)], output: &Path,
) -> Result<(), String> {
    if !source.is_file() { return Err("The source video no longer exists.".into()); }
    if is_same_file(source, output) {
        return Err("The timeline file cannot replace the source video.".into());
    }
    let rate = frame_rate(info.fps.ok_or_else(|| "The source frame rate could not be determined.".to_string())?)?;
    let segments = keep_segments(delete_ranges, info.duration, rate);
    if segments.is_empty() { return Err("The edit removes the entire source; no timeline can be exported.".into()); }
    let contents = match format {
        InterchangeFormat::Edl => build_edl(source, info, rate, &segments),
        InterchangeFormat::Fcpxml => build_fcpxml(source, info, rate, &segments)?,
        InterchangeFormat::OpenTimelineIo => build_otio(source, info, rate, &segments)?,
        InterchangeFormat::DavinciResolve | InterchangeFormat::PremiereXml =>
            build_fcp7_xml(source, info, rate, &segments)?,
    };
    atomic_write(output, &contents)
}

fn build_edl(source: &Path, info: &MediaInfo, rate: FrameRate, segments: &[Segment]) -> String {
    let mut out = String::new();
    let reel = reel_name(source);
    let name = source.file_name().and_then(|s| s.to_str()).unwrap_or("Source");
    let audio_channels = info.audio_channels.unwrap_or(2);
    let channel_code = if !info.has_audio {
        "V"
    } else {
        match audio_channels {
            0 => "V",
            1 => "B",
            2 => "AA/V",
            3 => "VA1A2A3",
            _ => "VA1A2A3A4",
        }
    };

    out.push_str(&format!("TITLE: {}\nFCM: NON-DROP FRAME\n\n",
        source.file_stem().and_then(|s| s.to_str()).unwrap_or("Blaze Edit")));
    for (index, segment) in segments.iter().enumerate() {
        let n = index + 1;
        let timecode_base = rate.nominal as i64 * 3600;
        // Source media is addressed from frame zero; the record timeline starts at 01:00:00:00.
        let source_in = segment.source_in;
        let source_out = segment.source_out;
        let record_in = timecode_base + segment.record_in;
        let record_out = record_in + segment.duration();
        out.push_str(&format!("{n:03}  {reel:<8} {channel_code:<8} C        {} {} {} {}\n",
            timecode(source_in, rate), timecode(source_out, rate),
            timecode(record_in, rate), timecode(record_out, rate)));
        out.push_str(&format!("* FROM CLIP NAME: {name}\n"));
        out.push_str(&format!("* SOURCE FILE: {}\n\n", source.display()));
    }
    if audio_channels > 4 {
        out.push_str("* WARNING: CMX 3600 represents at most four audio channels; additional channels are not listed.\n");
    }
    out
}

fn build_fcp7_xml(
    source: &Path, info: &MediaInfo, rate: FrameRate, segments: &[Segment],
) -> Result<String, String> {
    let width = info.width.ok_or_else(|| "Source video dimensions are unavailable for XML export.".to_string())?;
    let height = info.height.ok_or_else(|| "Source video dimensions are unavailable for XML export.".to_string())?;
    let name = source.file_name().and_then(|s| s.to_str()).unwrap_or("Source");
    let name_xml = xml_escape(name);
    let path_url = xml_escape(&file_url(source));
    let clip_duration: i64 = segments.iter().map(|s| s.duration()).sum();
    let ntsc = if rate.ntsc { "TRUE" } else { "FALSE" };
    let source_duration = seconds_to_frames(info.duration, rate);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE xmeml>\n<xmeml version=\"5\"><project><name>Blaze Edit</name><children><sequence id=\"sequence-1\">\n");
    out.push_str(&format!("<name>Blaze Edit</name><duration>{clip_duration}</duration><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate>\n",
        rate.nominal));
    out.push_str(&format!("<timecode><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate><string>01:00:00:00</string><frame>{}</frame><displayformat>NDF</displayformat></timecode>\n",
        rate.nominal, rate.nominal as i64 * 3600));
    out.push_str(&format!("<media><video><format><samplecharacteristics><width>{width}</width><height>{height}</height><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate><fielddominance>none</fielddominance><pixelaspectratio>square</pixelaspectratio></samplecharacteristics></format><track>\n",
        rate.nominal));
    for (i, segment) in segments.iter().enumerate() {
        out.push_str(&format!("<clipitem id=\"video-{}\"><name>{name_xml}</name><duration>{}</duration><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate><start>{}</start><end>{}</end><in>{}</in><out>{}</out>",
            i + 1, segment.duration(), rate.nominal, segment.record_in,
            segment.record_in + segment.duration(), segment.source_in, segment.source_out));
        if i == 0 {
            out.push_str(&format!("<file id=\"file-1\"><name>{name_xml}</name><pathurl>{path_url}</pathurl><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate><duration>{source_duration}</duration><media><video><samplecharacteristics><width>{width}</width><height>{height}</height></samplecharacteristics></video>",
                rate.nominal));
            if info.has_audio {
                let channels = info.audio_channels.unwrap_or(2);
                let sample_rate = info.audio_sample_rate.unwrap_or(48000);
                out.push_str(&format!("<audio><channelcount>{channels}</channelcount><samplecharacteristics><samplerate>{sample_rate}</samplerate></samplecharacteristics></audio>"));
            }
            out.push_str("</media></file>\n");
        } else {
            out.push_str("<file id=\"file-1\"/>\n");
        }
        if info.has_audio {
            out.push_str(&format!("<link><linkclipref>audio-{}</linkclipref><mediatype>audio</mediatype><trackindex>1</trackindex><clipindex>{}</clipindex></link>\n",
                i + 1, i + 1));
        }
        out.push_str("</clipitem>\n");
    }
    out.push_str("</track></video>\n");
    if info.has_audio {
        let channels = info.audio_channels.unwrap_or(2);
        let sample_rate = info.audio_sample_rate.unwrap_or(48000);
        out.push_str(&format!("<audio><numOutputChannels>{channels}</numOutputChannels><format><samplecharacteristics><samplerate>{sample_rate}</samplerate></samplecharacteristics></format><track>\n"));
        for (i, segment) in segments.iter().enumerate() {
            out.push_str(&format!("<clipitem id=\"audio-{}\"><name>{name_xml}</name><duration>{}</duration><rate><timebase>{}</timebase><ntsc>{ntsc}</ntsc></rate><start>{}</start><end>{}</end><in>{}</in><out>{}</out><file id=\"file-1\"/><sourcetrack><mediatype>audio</mediatype><trackindex>1</trackindex></sourcetrack><link><linkclipref>video-{}</linkclipref><mediatype>video</mediatype><trackindex>1</trackindex><clipindex>{}</clipindex></link></clipitem>\n",
                i + 1, segment.duration(), rate.nominal, segment.record_in,
                segment.record_in + segment.duration(), segment.source_in, segment.source_out, i + 1, i + 1));
        }
        out.push_str("</track></audio>\n");
    }
    out.push_str("</media></sequence></children></project></xmeml>\n");
    Ok(out)
}


fn otio_rational_time(value: i64, rate: FrameRate) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "RationalTime.1",
        "value": value,
        "rate": rate.numerator as f64 / rate.denominator as f64
    })
}

fn otio_time_range(start: i64, duration: i64, rate: FrameRate) -> serde_json::Value {
    serde_json::json!({
        "OTIO_SCHEMA": "TimeRange.1",
        "start_time": otio_rational_time(start, rate),
        "duration": otio_rational_time(duration, rate)
    })
}

fn build_otio_track(
    name: &str,
    kind: &str,
    source: &Path,
    info: &MediaInfo,
    rate: FrameRate,
    segments: &[Segment],
) -> serde_json::Value {
    let source_name = source.file_name().and_then(|value| value.to_str()).unwrap_or("Source");
    let source_url = file_url(source);
    let source_duration = seconds_to_frames(info.duration, rate);

    let children: Vec<serde_json::Value> = segments.iter().enumerate().map(|(index, segment)| {
        serde_json::json!({
            "OTIO_SCHEMA": "Clip.2",
            "active_media_reference_key": "DEFAULT_MEDIA",
            "color": null,
            "effects": [],
            "enabled": true,
            "markers": [],
            "media_references": {
                "DEFAULT_MEDIA": {
                    "OTIO_SCHEMA": "ExternalReference.1",
                    "available_image_bounds": null,
                    "available_range": otio_time_range(0, source_duration, rate),
                    "metadata": {},
                    "name": source_name,
                    "target_url": source_url
                }
            },
            "metadata": {},
            "name": format!("{source_name} — Segment {}", index + 1),
            "source_range": otio_time_range(segment.source_in, segment.duration(), rate)
        })
    }).collect();

    serde_json::json!({
        "OTIO_SCHEMA": "Track.1",
        "children": children,
        "color": null,
        "effects": [],
        "enabled": true,
        "kind": kind,
        "markers": [],
        "metadata": {},
        "name": name,
        "source_range": null
    })
}

/// Writes an OpenTimelineIO JSON timeline referencing the original source media.
fn build_otio(
    source: &Path,
    info: &MediaInfo,
    rate: FrameRate,
    segments: &[Segment],
) -> Result<String, String> {
    let mut tracks = vec![
        build_otio_track("Video", "Video", source, info, rate, segments),
    ];
    if info.has_audio {
        tracks.push(build_otio_track("Audio", "Audio", source, info, rate, segments));
    }

    let timeline = serde_json::json!({
        "OTIO_SCHEMA": "Timeline.1",
        "global_start_time": otio_rational_time(0, rate),
        "metadata": {},
        "name": source.file_stem().and_then(|value| value.to_str()).unwrap_or("Blaze Edit"),
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "children": tracks,
            "color": null,
            "effects": [],
            "enabled": true,
            "markers": [],
            "metadata": {},
            "name": "Tracks",
            "source_range": null
        }
    });

    serde_json::to_string_pretty(&timeline)
        .map_err(|error| format!("Could not serialize the OpenTimelineIO timeline: {error}"))
}

fn build_fcpxml(
    source: &Path, info: &MediaInfo, rate: FrameRate, segments: &[Segment],
) -> Result<String, String> {
    let width = info.width.ok_or_else(|| "Source video dimensions are unavailable for FCPXML export.".to_string())?;
    let height = info.height.ok_or_else(|| "Source video dimensions are unavailable for FCPXML export.".to_string())?;
    let name = source.file_name().and_then(|s| s.to_str()).unwrap_or("Source");
    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("Blaze Edit");
    let path_url = xml_escape(&file_url(source));
    let name_xml = xml_escape(name);
    let stem_xml = xml_escape(stem);
    let total_frames: i64 = segments.iter().map(|s| s.duration()).sum();
    let channels = info.audio_channels.unwrap_or(2);
    let sample_rate = info.audio_sample_rate.unwrap_or(48000);
    let format_name = if rate.ntsc {
        format!("FFVideoFormat{}p{:.2}", height,
            rate.numerator as f64 / rate.denominator as f64)
    } else { format!("FFVideoFormat{}p{}", height, rate.nominal) };

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE fcpxml>\n<fcpxml version=\"1.14\"><resources>\n");
    out.push_str(&format!("<format id=\"r1\" name=\"{format_name}\" frameDuration=\"{}/{}s\" width=\"{width}\" height=\"{height}\"/>\n",
        rate.denominator, rate.numerator));
    out.push_str(&format!("<asset id=\"r2\" name=\"{name_xml}\" start=\"0s\" duration=\"{}\" hasVideo=\"1\" hasAudio=\"{}\" format=\"r1\"",
        frame_time(seconds_to_frames(info.duration, rate), rate), if info.has_audio { "1" } else { "0" }));
    if info.has_audio {
        out.push_str(&format!(" audioSources=\"1\" audioChannels=\"{channels}\" audioRate=\"{sample_rate}\">\n<media-rep kind=\"original-media\" src=\"{path_url}\"/>\n</asset>\n"));
    } else {
        out.push_str(&format!(">\n<media-rep kind=\"original-media\" src=\"{path_url}\"/>\n</asset>\n"));
    }

    out.push_str(&format!("</resources><library><event name=\"Blaze Edit\"><project name=\"{stem_xml}\"><sequence format=\"r1\" duration=\"{}\" tcStart=\"0s\" tcFormat=\"NDF\"",
        frame_time(total_frames, rate)));
    if info.has_audio {
        let layout = if channels == 1 { "mono" } else if channels == 2 { "stereo" } else { "surround" };
        let audio_rate = if sample_rate % 1000 == 0 {
            format!("{}k", sample_rate / 1000)
        } else {
            format!("{:.1}k", sample_rate as f64 / 1000.0)
        };
        out.push_str(&format!(" audioLayout=\"{layout}\" audioRate=\"{audio_rate}\""));
    }
    out.push_str("><spine>\n");
    for (i, segment) in segments.iter().enumerate() {
        out.push_str(&format!("<asset-clip name=\"{name_xml} ({})\" ref=\"r2\" offset=\"{}\" start=\"{}\" duration=\"{}\"",
            i + 1, frame_time(segment.record_in, rate),
            frame_time(segment.source_in, rate), frame_time(segment.duration(), rate)));
        if info.has_audio { out.push_str(" audioRole=\"dialogue\""); }
        out.push_str("/>\n");
    }
    out.push_str("</spine></sequence></project></event></library></fcpxml>\n");
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otio_export_contains_video_audio_tracks_and_source_ranges() {
        let rate = frame_rate(25.0).unwrap();
        let segments = keep_segments(&[(2.0, 4.0)], 10.0, rate);
        let info = MediaInfo {
            duration: 10.0,
            fps: Some(25.0),
            width: Some(1920),
            height: Some(1080),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            has_audio: true,
            audio_channels: Some(2),
            audio_sample_rate: Some(48000),
        };
        let timeline = build_otio(Path::new("/tmp/source clip.mp4"), &info, rate, &segments).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&timeline).unwrap();
        assert_eq!(parsed["OTIO_SCHEMA"], "Timeline.1");
        let tracks = parsed["tracks"]["children"].as_array().unwrap();
        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0]["kind"], "Video");
        assert_eq!(tracks[1]["kind"], "Audio");
        assert_eq!(tracks[0]["children"][0]["OTIO_SCHEMA"], "Clip.2");
        assert_eq!(tracks[0]["children"][0]["source_range"]["start_time"]["value"], 0);
        assert_eq!(tracks[0]["children"][0]["source_range"]["duration"]["value"], 50);
        assert_eq!(tracks[0]["children"][0]["media_references"]["DEFAULT_MEDIA"]["target_url"],
            "file:///tmp/source%20clip.mp4");
    }

    #[test]
    fn deletion_ranges_are_merged_and_inverted() {
        let rate = frame_rate(25.0).unwrap();
        let segments = keep_segments(&[(2.0, 4.0), (3.5, 5.0), (8.0, 9.0)], 10.0, rate);
        assert_eq!(segments.len(), 3);
        assert_eq!((segments[0].source_in, segments[0].source_out), (0, 50));
        assert_eq!((segments[1].source_in, segments[1].source_out), (125, 200));
        assert_eq!((segments[2].source_in, segments[2].source_out), (225, 250));
    }

    #[test]
    fn supplied_delete_list_produces_expected_keep_segments() {
        let rate = frame_rate(25.0).unwrap();
        let cuts = [
            (0.0, 12.0),
            (67.0, 71.0),
            (108.0, 148.0),
            (168.0, 172.0),
            (843.0, 900.0),
        ];
        let segments = keep_segments(&cuts, 900.0, rate);
        assert_eq!(segments.len(), 4);
        assert_eq!((segments[0].source_in, segments[0].source_out), (300, 1_675));
        assert_eq!((segments[1].source_in, segments[1].source_out), (1_775, 2_700));
        assert_eq!((segments[2].source_in, segments[2].source_out), (3_700, 4_200));
        assert_eq!((segments[3].source_in, segments[3].source_out), (4_300, 21_075));
        assert_eq!(segments.iter().map(|s| s.duration()).sum::<i64>(), 19_575);
    }

    #[test]
    fn fractional_frame_rate_is_preserved() {
        let rate = frame_rate(29.97002997).unwrap();
        assert_eq!((rate.numerator, rate.denominator, rate.nominal, rate.ntsc), (30000, 1001, 30, true));
        assert_eq!(frame_time(30, rate), "1001/1000s");
    }

    #[test]
    fn xml_escaping_handles_special_characters() {
        assert_eq!(xml_escape("A&B <clip> 'x'"), "A&amp;B &lt;clip&gt; &apos;x&apos;");
    }

    #[test]
    fn file_urls_escape_spaces_and_hashes() {
        assert_eq!(file_url(Path::new("/tmp/my video#1.mov")), "file:///tmp/my%20video%231.mov");
    }

    #[test]
    fn windows_paths_become_valid_file_urls() {
        assert_eq!(file_url_from_str(r"C:\Users\Ann\My Videos\clip#1.mp4", true),
            "file:///C:/Users/Ann/My%20Videos/clip%231.mp4");
        assert_eq!(file_url_from_str(r"\\?\C:\Users\Ann\clip.mp4", true),
            "file:///C:/Users/Ann/clip.mp4");
        assert_eq!(file_url_from_str(r"\\server\share\a b.mp4", true),
            "file://server/share/a%20b.mp4");
        assert_eq!(file_url_from_str(r"\\?\UNC\server\share\a.mp4", true),
            "file://server/share/a.mp4");
    }

    #[cfg(unix)]
    #[test]
    fn file_urls_preserve_non_utf8_unix_path_bytes() {
        use std::os::unix::ffi::OsStringExt;
        let path = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/a\xff b.mp4".to_vec()));
        assert_eq!(file_url(&path), "file:///tmp/a%FF%20b.mp4");
    }

    #[test]
    fn atomic_timeline_write_replaces_existing_destination() {
        let dir = std::env::temp_dir().join(format!("blaze-atomic-write-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let output = dir.join("timeline.xml");
        fs::write(&output, "old contents").unwrap();
        atomic_write(&output, "new contents").unwrap();
        assert_eq!(fs::read_to_string(&output).unwrap(), "new contents");
        assert!(fs::read_dir(&dir).unwrap().all(|entry| {
            !entry.unwrap().file_name().to_string_lossy().contains(".blaze_tmp_")
        }));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backslash_is_encoded_on_non_windows_paths() {
        assert_eq!(file_url_from_str("/tmp/a\\b.mp4", false), "file:///tmp/a%5Cb.mp4");
    }

    #[test]
    fn same_file_detection_sees_equivalent_spellings() {
        let dir = std::env::temp_dir().join(format!("blaze-same-file-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let source = dir.join("clip.mp4");
        fs::write(&source, b"x").unwrap();
        let indirect = dir.join(".").join("clip.mp4");
        assert!(is_same_file(&source, &indirect));
        assert!(!is_same_file(&source, &dir.join("clip_Blaze.mp4")));
        let _ = fs::remove_dir_all(&dir);
    }
}
