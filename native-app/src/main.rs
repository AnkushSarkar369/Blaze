use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use blaze_engine::{MediaEngine, MediaInfo};
use blaze_engine::export::{self, InterchangeFormat};
use blaze_engine::silence::{
    DetectionLevel, PaddingPreset, SilenceDetectionConfig, SilenceInterval,
};
// Silence detection UI integration follows.
use eframe::egui;
use playback::Playback;
use rfd::FileDialog;

mod playback;
mod waveform;

use waveform::Waveform;

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditingMode {
    Blaze,
    Precise,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolTab {
    ModeSelector,
    BlazeMode,
    PreciseMode,
    Export,
}

impl ToolTab {
    fn label(self) -> &'static str {
        match self {
            Self::ModeSelector => "Mode Selector",
            Self::BlazeMode => "Blaze Mode",
            Self::PreciseMode => "Precise Mode",
            Self::Export => "Export",
        }
    }
}

fn help_seen_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("blaze")
            .join("first_run_help_seen_v2")
    })
}

fn has_seen_first_run_help() -> bool {
    help_seen_path().is_some_and(|path| path.exists())
}

fn mark_first_run_help_seen() {
    if let Some(path) = help_seen_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, b"");
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct EditRegion {
    start: f64,
    end: f64,
}

type ProbeResult = (u64, Result<(PathBuf, MediaInfo), String>);
type SilenceResult = (u64, Result<Vec<SilenceInterval>, String>);
type KeyframeResult = (u64, Result<Vec<f64>, String>);
type ExportResult = (u64, Result<String, String>);
enum WaveformMessage {
    Progress { generation: u64, fraction: f32 },
    Complete { generation: u64, result: Result<Vec<u8>, String> },
}

struct BlazeApp {
    playback: Playback,
    selected_path: Option<PathBuf>,
    media_info: Option<MediaInfo>,
    loading: bool,
    seeking: bool,
    playback_position: f64,
    probe_generation: u64,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
    waveform: Option<Waveform>,
    waveform_tx: Sender<WaveformMessage>,
    waveform_rx: Receiver<WaveformMessage>,
    waveform_progress: Option<f32>,
    silence_tx: Sender<SilenceResult>,
    silence_rx: Receiver<SilenceResult>,
    silence_generation: u64,
    silence_running: bool,
    keyframe_tx: Sender<KeyframeResult>,
    keyframe_rx: Receiver<KeyframeResult>,
    keyframe_generation: u64,
    keyframes: Vec<f64>,
    keyframes_running: bool,
    keyframe_error: Option<String>,
    export_tx: Sender<ExportResult>,
    export_rx: Receiver<ExportResult>,
    export_generation: u64,
    export_running: bool,
    export_status: Option<(bool, String)>,
    detection_level: usize,
    padding_level: usize,
    minimum_silence_ms: u32,
    expert_mode: bool,
    custom_padding_before_ms: u32,
    custom_padding_after_ms: u32,
    custom_threshold_db: f64,
    custom_peak_gate_enabled: bool,
    custom_peak_ceiling_dbfs: f64,
    timeline_zoom: f32,
    timeline_offset: f64,
    edit_regions: Vec<EditRegion>,
    pending_region_start: Option<f64>,
    skip_cuts_enabled: bool,
    active_mode: Option<EditingMode>,
    active_tool_tab: ToolTab,
    show_help: bool,
    first_run_help_seen: bool,
    pending_preview_fit: bool,
}

fn nearest_keyframe(keyframes: &[f64], time: f64) -> Option<f64> {
    keyframes.iter().copied().min_by(|a, b| {
        (a - time).abs().total_cmp(&(b - time).abs())
    })
}

fn nearest_boundary_keyframe(keyframes: &[f64], time: f64, other: f64, is_start: bool) -> Option<f64> {
    keyframes.iter().copied()
        .filter(|candidate| if is_start { *candidate < other } else { *candidate > other })
        .min_by(|a, b| (a - time).abs().total_cmp(&(b - time).abs()))
}

impl BlazeApp {
    fn new(
        cc: &eframe::CreationContext<'_>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let mut visuals = egui::Visuals::dark();
        let app_background = egui::Color32::from_rgb(16, 17, 18);
        let panel_border = egui::Color32::from_rgb(42, 44, 47);
        visuals.panel_fill = app_background;
        visuals.window_fill = app_background;
        visuals.extreme_bg_color = app_background;
        visuals.faint_bg_color = app_background;
        visuals.widgets.noninteractive.bg_fill = app_background;
        visuals.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, panel_border);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(34, 36, 39);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(46, 49, 52);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(255, 104, 79);
        visuals.hyperlink_color = egui::Color32::from_rgb(255, 120, 96);
        visuals.selection.bg_fill = egui::Color32::from_rgb(92, 45, 39);
        visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::from_rgb(255, 104, 79));
        cc.egui_ctx.set_visuals(visuals);

        // A single rounded geometric sans-serif system, with restrained hierarchy.
        cc.egui_ctx.all_styles_mut(|style| {
            use egui::{FontFamily, FontId, TextStyle};
            style.animation_time = 0.14;
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(12.0, 7.0);
            style.text_styles.insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Small, FontId::new(11.5, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Heading, FontId::new(22.0, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
        });

        let playback = Playback::new(cc)?;
        let (probe_tx, probe_rx) = mpsc::channel();
        let (waveform_tx, waveform_rx) = mpsc::channel();
        let (silence_tx, silence_rx) = mpsc::channel();
        let (keyframe_tx, keyframe_rx) = mpsc::channel();
        let (export_tx, export_rx) = mpsc::channel();

        Ok(Self {
            playback,
            selected_path: None,
            media_info: None,
            loading: false,
            seeking: false,
            playback_position: 0.0,
            probe_generation: 0,
            probe_tx,
            probe_rx,
            waveform: None,
            waveform_tx,
            waveform_rx,
            waveform_progress: None,
            silence_tx,
            silence_rx,
            silence_generation: 0,
            silence_running: false,
            keyframe_tx,
            keyframe_rx,
            keyframe_generation: 0,
            keyframes: Vec::new(),
            keyframes_running: false,
            keyframe_error: None,
            export_tx,
            export_rx,
            export_generation: 0,
            export_running: false,
            export_status: None,
            detection_level: 2,
            padding_level: 1,
            minimum_silence_ms: 300,
            expert_mode: false,
            custom_padding_before_ms: 60,
            custom_padding_after_ms: 60,
            custom_threshold_db: 22.0,
            custom_peak_gate_enabled: false,
            custom_peak_ceiling_dbfs: -35.0,
            timeline_zoom: 1.0,
            timeline_offset: 0.0,
            edit_regions: Vec::new(),
            pending_region_start: None,
            skip_cuts_enabled: false,
            active_mode: None,
            active_tool_tab: ToolTab::ModeSelector,
            show_help: !has_seen_first_run_help(),
            first_run_help_seen: has_seen_first_run_help(),
            pending_preview_fit: false,
        })
    }

    fn start_keyframe_analysis(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.keyframe_generation = self.keyframe_generation.wrapping_add(1);
        let generation = self.keyframe_generation;
        self.keyframes.clear();
        self.keyframes_running = true;
        self.keyframe_error = None;
        let sender = self.keyframe_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = engine.detect_keyframes(&path.to_string_lossy());
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_keyframes(&mut self, ctx: &egui::Context) {
        while let Ok((generation, result)) = self.keyframe_rx.try_recv() {
            if generation != self.keyframe_generation { continue; }
            self.keyframes_running = false;
            match result {
                Ok(mut keyframes) => {
                    keyframes.sort_by(f64::total_cmp);
                    keyframes.dedup_by(|a, b| (*a - *b).abs() < 0.000_001);
                    self.keyframes = keyframes;
                    if self.keyframes.is_empty() {
                        self.keyframe_error = Some("No usable video keyframes were found.".to_string());
                    }
                }
                Err(error) => self.keyframe_error = Some(error),
            }
            ctx.request_repaint();
        }
    }

    fn start_silence_detection(&mut self, ctx: &egui::Context) {
        let Some(path) = self.selected_path.clone() else { return; };
        if self.silence_running { return; }

        self.silence_generation = self.silence_generation.wrapping_add(1);
        let generation = self.silence_generation;
        self.silence_running = true;

        let level = match self.detection_level {
            0 => DetectionLevel::Comfortable,
            1 => DetectionLevel::Optimal,
            2 => DetectionLevel::Default,
            3 => DetectionLevel::Tight,
            _ => DetectionLevel::Aggressive,
        };
        let preset_padding = match self.padding_level {
            0 => PaddingPreset::Comfortable.milliseconds(),
            2 => PaddingPreset::Tight.milliseconds(),
            _ => PaddingPreset::Normal.milliseconds(),
        };
        let (padding_before_ms, padding_after_ms) = if self.expert_mode {
            (self.custom_padding_before_ms, self.custom_padding_after_ms)
        } else {
            (preset_padding, preset_padding)
        };
        let config = SilenceDetectionConfig {
            level,
            padding_before_ms,
            padding_after_ms,
            minimum_duration_ms: self.minimum_silence_ms,
            threshold_below_baseline_db: self.expert_mode.then_some(self.custom_threshold_db),
            peak_ceiling_dbfs: (self.expert_mode && self.custom_peak_gate_enabled)
                .then_some(self.custom_peak_ceiling_dbfs),
        };
        let tx = self.silence_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = blaze_engine::silence::detect_silence(&engine, &path.to_string_lossy(), config);
            let _ = tx.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_silence(&mut self) {
        while let Ok((generation, result)) = self.silence_rx.try_recv() {
            if generation != self.silence_generation { continue; }
            self.silence_running = false;
            match result {
                Ok(intervals) => {
                    // Silence detection is identical in both modes. Blaze applies
                    // keyframe constraints only when converting detections to cuts.
                    self.edit_regions = if self.active_mode == Some(EditingMode::Blaze) {
                        let keyframes = &self.keyframes;
                        intervals.into_iter().filter_map(|interval| {
                            let start = nearest_keyframe(keyframes, interval.start)?;
                            let end = nearest_boundary_keyframe(keyframes, interval.end, start, false)?;
                            (end > start).then_some(EditRegion { start, end })
                        }).collect()
                    } else {
                        intervals.into_iter()
                            .filter(|interval| interval.end > interval.start)
                            .map(|interval| EditRegion { start: interval.start, end: interval.end })
                            .collect()
                    };
                    self.pending_region_start = None;
                }
                Err(error) => eprintln!("Blaze: silence detection failed: {error}"),
            }
        }
    }

    fn media_duration(&self) -> f64 {
        self.media_info
            .as_ref()
            .map(|info| info.duration)
            .filter(|duration| duration.is_finite() && *duration > 0.0)
            .unwrap_or_else(|| self.playback.duration())
            .max(0.01)
    }

    fn open_video(&mut self, ctx: &egui::Context) {
        let Some(path) = FileDialog::new().pick_file() else {
            return;
        };

        self.probe_generation = self.probe_generation.wrapping_add(1);
        self.keyframe_generation = self.keyframe_generation.wrapping_add(1);
        self.keyframes.clear();
        self.keyframes_running = false;
        self.keyframe_error = None;
        self.silence_generation = self.silence_generation.wrapping_add(1);
        self.silence_running = false;
        let generation = self.probe_generation;
        self.loading = true;
        self.seeking = false;
        self.playback_position = 0.0;
        self.timeline_zoom = 1.0;
        self.timeline_offset = 0.0;
        self.edit_regions.clear();
        self.pending_region_start = None;
        self.media_info = None;
        self.waveform = None;
        self.waveform_progress = Some(0.0);
        self.selected_path = Some(path.clone());
        self.pending_preview_fit = false;
        self.start_keyframe_analysis(path.clone(), ctx);

        if let Err(error) = self.playback.load(&path) {
            eprintln!("Blaze: {error}");
            self.playback.stop();
            self.selected_path = None;
            self.loading = false;
            self.waveform_progress = None;
            return;
        }

        let sender = self.probe_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = engine
                .probe_media(&path.to_string_lossy())
                .map(|info| (path, info));
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_probe(&mut self, ctx: &egui::Context) {
        while let Ok((generation, result)) = self.probe_rx.try_recv() {
            if generation != self.probe_generation {
                continue;
            }

            match result {
                Ok((path, info)) => {
                    self.selected_path = Some(path.clone());
                    self.loading = false;

                    if info.has_audio {
                        let channels = info.audio_channels.unwrap_or(2).clamp(1, 32) as usize;
                        let duration = info.duration;
                        let sender = self.waveform_tx.clone();
                        let repaint = ctx.clone();
                        std::thread::spawn(move || {
                            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
                            let engine = MediaEngine::new([tools]);
                            let result = engine.generate_waveform_with_progress(
                                &path.to_string_lossy(),
                                channels,
                                duration,
                                |fraction| {
                                    let _ = sender.send(WaveformMessage::Progress {
                                        generation,
                                        fraction,
                                    });
                                    repaint.request_repaint();
                                },
                            );
                            let _ = sender.send(WaveformMessage::Complete { generation, result });
                            repaint.request_repaint();
                        });
                    } else {
                        self.waveform_progress = None;
                    }

                    self.media_info = Some(info);
                    self.pending_preview_fit = true;
                }
                Err(error) => {
                    self.loading = false;
                    self.selected_path = None;
                    self.media_info = None;
                    self.waveform = None;
                    self.waveform_progress = None;
                    self.playback_position = 0.0;
                    self.playback.stop();
                    eprintln!("Blaze: media analysis failed: {error}");
                }
            }
        }
    }

    fn poll_waveform(&mut self) {
        while let Ok(message) = self.waveform_rx.try_recv() {
            match message {
                WaveformMessage::Progress { generation, fraction }
                    if generation == self.probe_generation =>
                {
                    self.waveform_progress = Some(fraction.clamp(0.0, 1.0));
                }
                WaveformMessage::Complete { generation, result }
                    if generation == self.probe_generation =>
                {
                    self.waveform_progress = None;
                    match result {
                        Ok(bytes) => match Waveform::from_bytes(&bytes) {
                            Ok(waveform) => self.waveform = Some(waveform),
                            Err(error) => {
                                self.waveform = None;
                                eprintln!("Blaze: waveform data was invalid: {error}");
                            }
                        },
                        Err(error) => {
                            self.waveform = None;
                            eprintln!("Blaze: waveform generation failed: {error}");
                        }
                    }
                }
                _ => {}
            }
        }
    }

    fn start_blaze_export(&mut self, output: PathBuf, ctx: &egui::Context) {
        if self.export_running { return; }
        let (Some(source), Some(info)) = (self.selected_path.clone(), self.media_info.clone()) else { return; };
        let ranges: Vec<(f64, f64)> = self.edit_regions.iter().map(|r| (r.start, r.end)).collect();
        let duration = info.duration;
        self.export_generation = self.export_generation.wrapping_add(1);
        let generation = self.export_generation;
        self.export_running = true;
        self.export_status = None;
        let sender = self.export_tx.clone();
        let repaint = ctx.clone();

        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = export::export_blaze_video(&engine, &source, &output, duration, &ranges)
                .map(|report| {
                    let mut message = format!(
                        "Blaze Export completed.\n{}\nDuration: {:.2}s",
                        report.output_path.display(),
                        report.actual_duration
                    );
                    if let Some(warning) = report.warning {
                        message.push_str("\n");
                        message.push_str(&warning);
                    }
                    message
                });
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn start_interchange_export(
        &mut self,
        format: InterchangeFormat,
        output: PathBuf,
        ctx: &egui::Context,
    ) {
        if self.export_running { return; }
        let (Some(source), Some(info)) = (self.selected_path.clone(), self.media_info.clone()) else { return; };
        let ranges: Vec<(f64, f64)> = self.edit_regions.iter().map(|r| (r.start, r.end)).collect();
        self.export_generation = self.export_generation.wrapping_add(1);
        let generation = self.export_generation;
        self.export_running = true;
        self.export_status = None;
        let sender = self.export_tx.clone();
        let repaint = ctx.clone();

        std::thread::spawn(move || {
            let result = export::export_interchange(format, &source, &info, &ranges, &output)
                .map(|_| format!("Timeline exported:\n{}", output.display()));
            let _ = sender.send((generation, result));
            repaint.request_repaint();
        });
    }

    fn poll_exports(&mut self) {
        while let Ok((generation, result)) = self.export_rx.try_recv() {
            if generation != self.export_generation { continue; }
            self.export_running = false;
            self.export_status = Some(match result {
                Ok(message) => (true, message),
                Err(error) => (false, error),
            });
        }
    }

}

impl eframe::App for BlazeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_probe(ctx);
        self.poll_waveform();
        self.poll_silence();
        self.poll_keyframes(ctx);
        self.poll_exports();

        // Global transport shortcut: handle Space at the app level rather
        // than inside a particular panel, so it works regardless of focus
        // or which part of the interface the user is interacting with.
        let space_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Space)
        });
        let help_shortcut = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::H)
        });
        let mark_start_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::N)
        });
        let mark_end_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::M)
        });
        let blaze_mode = self.active_mode == Some(EditingMode::Blaze);
        if self.selected_path.is_some() && self.active_mode.is_some() && mark_start_pressed {
            if self.pending_region_start.is_some() {
                self.pending_region_start = None;
            } else {
                if blaze_mode {
                    if let Some(start) = nearest_keyframe(&self.keyframes, self.playback_position) {
                        self.pending_region_start = Some(start);
                    }
                } else {
                    self.pending_region_start = Some(self.playback_position);
                }
            }
        }
        if self.selected_path.is_some() && self.active_mode.is_some() && mark_end_pressed {
            if let Some(start) = self.pending_region_start {
                let end = if blaze_mode {
                    nearest_boundary_keyframe(&self.keyframes, self.playback_position, start, false)
                } else { Some(self.playback_position) };
                if end.is_some_and(|end| end > start) {
                    self.edit_regions.push(EditRegion { start, end: end.unwrap() });
                    self.pending_region_start = None;
                }
            }
        }
        if help_shortcut {
            self.show_help = !self.show_help;
        }
        if space_pressed && self.selected_path.is_some() {
            if let Err(error) = self.playback.toggle_pause() {
                eprintln!("Blaze: {error}");
            }
        }

        if let Some(error) = self.playback.poll_error() {
            eprintln!("Blaze: {error}");
            self.loading = false;
            self.selected_path = None;
            self.media_info = None;
            self.waveform = None;
            self.waveform_progress = None;
            self.playback_position = 0.0;
        }

        if self.selected_path.is_some() && !self.seeking {
            self.playback_position = self.playback.position();

            // Skip completed N–M regions during active playback. Extend the target
            // through overlapping or directly adjacent regions so no marked content
            // is played between consecutive cuts.
            if self.skip_cuts_enabled && !self.playback.is_paused() {
                let mut skip_target = self.playback_position;
                loop {
                    let next_target = self
                        .edit_regions
                        .iter()
                        .filter(|region| {
                            region.start <= skip_target && region.end > skip_target
                        })
                        .map(|region| region.end)
                        .fold(skip_target, f64::max);

                    if next_target <= skip_target {
                        break;
                    }
                    skip_target = next_target;
                }

                if skip_target > self.playback_position {
                    if let Err(error) = self.playback.seek(skip_target) {
                        eprintln!("Blaze: could not skip cut region: {error}");
                    } else {
                        self.playback_position = skip_target;
                    }
                }
            }

            // Keep the playhead visible while playing. If playback starts outside
            // the zoomed viewport, jump to it; once it reaches the right edge,
            // advance the viewport while keeping the playhead comfortably in view.
            if !self.playback.is_paused() {
                let duration = self.media_duration();
                let visible_duration = duration / self.timeline_zoom as f64;
                let view_end = self.timeline_offset + visible_duration;
                if self.playback_position < self.timeline_offset
                    || self.playback_position > view_end
                {
                    self.timeline_offset = (self.playback_position - visible_duration * 0.8)
                        .clamp(0.0, (duration - visible_duration).max(0.0));
                }
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        if self.loading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Make panel splitters easier to grab without changing their saved sizes.
        ui.style_mut().interaction.resize_grab_radius_side = 10.0;
        let root_rect = ui.available_rect_before_wrap();

        // Fit the initial preview width to the imported video's aspect ratio.
        // This runs once per import; later manual resizing remains untouched.
        if self.pending_preview_fit {
            if let Some(info) = self.media_info.as_ref() {
                let tools_width = default_tools_width(root_rect, info);
                let panel_id = ui.scope_id().with("tools");
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(tools_width, root_rect.height()),
                );
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(panel_id, egui::containers::panel::PanelState {
                        outer_rect: panel_rect,
                    });
                });
                self.pending_preview_fit = false;
            }
        }

        let timeline_panel = egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(220.0)
            .min_size(140.0)
            .show(ui, |ui| {
                let available = ui.available_rect_before_wrap();
                let ruler = egui::Rect::from_min_max(
                    egui::pos2(available.left() + 10.0, available.top() + 3.0),
                    egui::pos2(available.right() - 10.0, available.top() + 27.0),
                );
                let lane = egui::Rect::from_min_max(
                    egui::pos2(available.left() + 10.0, ruler.bottom() + 3.0),
                    egui::pos2(available.right() - 10.0, available.bottom() - 8.0),
                );

                let duration = self.media_duration();

                let can_seek = self.selected_path.is_some() && self.waveform_progress.is_none();
                let sense = if can_seek {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::hover()
                };
                let response = ui.allocate_rect(lane, sense);
                let ruler_response = ui.allocate_rect(ruler, sense);

                // Ctrl+wheel zooms; ordinary wheel input pans the timeline
                // horizontally. Horizontal trackpad input is also supported.
                if can_seek && (response.hovered() || ruler_response.hovered()) {
                    let (scroll, zoom_delta) = ui.input(|input| {
                        (input.smooth_scroll_delta, input.zoom_delta())
                    });
                    let visible_duration = (duration / self.timeline_zoom as f64).max(0.01);

                    // egui consumes Ctrl+wheel as its native zoom gesture and
                    // exposes it through zoom_delta(), not smooth_scroll_delta.
                    // Use that signal directly; it is 1.0 when no zoom gesture
                    // occurred (including ordinary wheel scrolling).
                    if (zoom_delta - 1.0).abs() > 0.0001 {
                        let pointer_x = ui.input(|input| input.pointer.hover_pos())
                            .map(|position| position.x)
                            .unwrap_or_else(|| lane.center().x);
                        let anchor = ((pointer_x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64;
                        let anchor_time = self.timeline_offset + anchor * visible_duration;
                        self.timeline_zoom = (self.timeline_zoom * zoom_delta).clamp(1.0, 128.0);
                        let new_visible = duration / self.timeline_zoom as f64;
                        self.timeline_offset = anchor_time - anchor * new_visible;
                    } else {
                        let pan_delta = if scroll.x.abs() > 0.0 {
                            scroll.x
                        } else {
                            scroll.y
                        };
                        if pan_delta.abs() > 0.0 {
                            self.timeline_offset -= pan_delta as f64 * visible_duration * 0.0015;
                        }
                    }

                    let visible_duration = duration / self.timeline_zoom as f64;
                    self.timeline_offset = self.timeline_offset.clamp(0.0, (duration - visible_duration).max(0.0));
                }

                let visible_duration = duration / self.timeline_zoom as f64;
                let view_start = self.timeline_offset / duration;
                let view_end = ((self.timeline_offset + visible_duration) / duration).min(1.0);

                // Adaptive time ruler: choose a clean 1–2–5 interval based on
                // available pixels, so labels remain legible at every zoom level.
                let raw_step = (visible_duration * 105.0 / lane.width().max(1.0) as f64).max(0.001);
                let magnitude = 10.0_f64.powf(raw_step.log10().floor());
                let normalized = raw_step / magnitude;
                let step = if normalized <= 1.0 { magnitude }
                    else if normalized <= 2.0 { 2.0 * magnitude }
                    else if normalized <= 5.0 { 5.0 * magnitude }
                    else { 10.0 * magnitude };
                let first_tick = (self.timeline_offset / step).ceil() * step;
                let mut tick = first_tick;
                while tick <= (self.timeline_offset + visible_duration).min(duration) + step * 0.0001 {
                    let x = lane.left()
                        + (((tick - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    if x >= ruler.left() && x <= ruler.right() {
                        ui.painter().line_segment(
                            [egui::pos2(x, ruler.bottom() - 5.0), egui::pos2(x, ruler.bottom())],
                            egui::Stroke::new(1.0, egui::Color32::from_rgb(91, 94, 98)),
                        );
                        ui.painter().text(
                            egui::pos2(x + 4.0, ruler.top() + 3.0),
                            egui::Align2::LEFT_TOP,
                            format_ruler_time(tick, step),
                            egui::FontId::proportional(11.5),
                            egui::Color32::from_rgb(145, 149, 153),
                        );
                    }
                    tick += step;
                }
                ui.painter().line_segment(
                    [ruler.left_bottom(), ruler.right_bottom()],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 49)),
                );

                ui.painter().rect_filled(
                    lane,
                    egui::CornerRadius::same(3),
                    egui::Color32::from_rgb(19, 20, 22),
                );

                if let Some(progress) = self.waveform_progress {
                    let fill_width = lane.width() * progress.clamp(0.0, 1.0);
                    if fill_width > 0.0 {
                        let fill = egui::Rect::from_min_size(
                            lane.min,
                            egui::vec2(fill_width, lane.height()),
                        );
                        ui.painter().rect_filled(
                            fill,
                            egui::CornerRadius::same(3),
                            egui::Color32::from_rgb(92, 45, 39),
                        );
                    }

                    let label = format!("Generating Waveform Data  ·  {}%", (progress * 100.0).round() as u32);
                    ui.painter().text(
                        lane.center(),
                        egui::Align2::CENTER_CENTER,
                        label,
                        egui::FontId::proportional(13.0),
                        egui::Color32::from_rgb(220, 220, 220),
                    );
                } else if let Some(waveform) = &self.waveform {
                    waveform.paint_range(ui.painter(), lane, view_start, view_end);
                } else {
                    ui.painter().line_segment(
                        [lane.left_center(), lane.right_center()],
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 46, 49)),
                    );
                }

                // Edit regions are independent of their origin (manual now,
                // silence analysis later). Their two boundary handles remain
                // freely movable in Precise Mode.
                let mut handle_interaction = false;
                let blaze_snapping = self.active_mode == Some(EditingMode::Blaze);
                let keyframes = self.keyframes.clone();
                for (index, region) in self.edit_regions.iter_mut().enumerate() {
                    let start_x = lane.left()
                        + (((region.start - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    let end_x = lane.left()
                        + (((region.end - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    let region_rect = egui::Rect::from_min_max(
                        egui::pos2(start_x, lane.top()),
                        egui::pos2(end_x, lane.bottom()),
                    );
                    if region_rect.intersects(lane) {
                        ui.painter().rect_filled(
                            region_rect.intersect(lane),
                            egui::CornerRadius::same(2),
                            egui::Color32::from_rgba_unmultiplied(255, 104, 79, 42),
                        );
                    }

                    for (is_start, x) in [(true, start_x), (false, end_x)] {
                        if x < lane.left() - 8.0 || x > lane.right() + 8.0 {
                            continue;
                        }
                        let handle_rect = egui::Rect::from_center_size(
                            egui::pos2(x, lane.center().y),
                            egui::vec2(14.0, lane.height().max(18.0)),
                        );
                        let id = ui.scope_id().with(("edit-region-handle", index, is_start));
                        let handle_response = ui.interact(handle_rect, id, egui::Sense::drag());
                        handle_interaction |= handle_response.hovered() || handle_response.dragged();
                        if handle_response.dragged() {
                            if let Some(pointer) = handle_response.interact_pointer_pos() {
                                let time = (self.timeline_offset
                                    + ((pointer.x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64
                                        * visible_duration)
                                    .clamp(0.0, duration);
                                if blaze_snapping {
                                    if let Some(snapped) = nearest_boundary_keyframe(&keyframes, time, if is_start { region.end } else { region.start }, is_start) {
                                        if is_start { region.start = snapped; } else { region.end = snapped; }
                                    }
                                } else if is_start {
                                    region.start = time.min(region.end - 0.001);
                                } else {
                                    region.end = time.max(region.start + 0.001).min(duration);
                                }
                            }
                        }
                        let color = if handle_response.hovered() || handle_response.dragged() {
                            egui::Color32::from_rgb(255, 151, 132)
                        } else {
                            egui::Color32::from_rgb(255, 104, 79)
                        };
                        ui.painter().line_segment(
                            [egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())],
                            egui::Stroke::new(if handle_response.hovered() { 4.0 } else { 3.0 }, color),
                        );
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(egui::pos2(x, lane.center().y), egui::vec2(7.0, 15.0)),
                            egui::CornerRadius::same(2),
                            color,
                        );
                    }
                }

                // An unpaired N marker is shown as a draggable starting jaw.
                if let Some(start) = self.pending_region_start {
                    let x = lane.left()
                        + (((start - self.timeline_offset) / visible_duration) as f32) * lane.width();
                    if x >= lane.left() - 8.0 && x <= lane.right() + 8.0 {
                        let handle_rect = egui::Rect::from_center_size(
                            egui::pos2(x, lane.center().y),
                            egui::vec2(14.0, lane.height().max(18.0)),
                        );
                        let response = ui.interact(
                            handle_rect,
                            ui.scope_id().with("pending-region-start"),
                            egui::Sense::drag(),
                        );
                        handle_interaction |= response.hovered() || response.dragged();
                        if response.dragged() {
                            if let Some(pointer) = response.interact_pointer_pos() {
                                let time = (self.timeline_offset
                                    + ((pointer.x - lane.left()) / lane.width()).clamp(0.0, 1.0) as f64
                                        * visible_duration).clamp(0.0, duration);
                                self.pending_region_start = Some(if blaze_snapping {
                                    nearest_keyframe(&keyframes, time).unwrap_or(start)
                                } else { time });
                            }
                        }
                        ui.painter().line_segment(
                            [egui::pos2(x, lane.top()), egui::pos2(x, lane.bottom())],
                            egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 104, 79)),
                        );
                    }
                }

                if self.selected_path.is_some() && self.waveform_progress.is_none()
                    && self.playback_position >= self.timeline_offset
                    && self.playback_position <= self.timeline_offset + visible_duration
                {
                    let progress = ((self.playback_position - self.timeline_offset) / visible_duration) as f32;
                    let playhead_x = lane.left() + lane.width() * progress;
                    let playhead_color = egui::Color32::from_rgb(255, 104, 79);
                    ui.painter().line_segment(
                        [
                            egui::pos2(playhead_x, lane.top() + 7.0),
                            egui::pos2(playhead_x, lane.bottom()),
                        ],
                        egui::Stroke::new(1.5, playhead_color),
                    );
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![
                            egui::pos2(playhead_x - 6.0, lane.top() - 2.0),
                            egui::pos2(playhead_x + 6.0, lane.top() - 2.0),
                            egui::pos2(playhead_x + 6.0, lane.top() + 2.0),
                            egui::pos2(playhead_x, lane.top() + 9.0),
                            egui::pos2(playhead_x - 6.0, lane.top() + 2.0),
                        ],
                        playhead_color,
                        egui::Stroke::NONE,
                    ));
                }

                if can_seek && !handle_interaction {
                    let timeline_offset = self.timeline_offset;
                    let pointer_time = |position: egui::Pos2| {
                        let progress = ((position.x - lane.left()) / lane.width()).clamp(0.0, 1.0);
                        timeline_offset + progress as f64 * visible_duration
                    };

                    if response.dragged() {
                        self.seeking = true;
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                    }

                    if response.drag_stopped() {
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                        self.seeking = false;
                        if let Err(error) = self.playback.seek(self.playback_position) {
                            eprintln!("Blaze: {error}");
                        }
                    } else if response.clicked() {
                        if let Some(position) = response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                            if let Err(error) = self.playback.seek(self.playback_position) {
                                eprintln!("Blaze: {error}");
                            }
                        }
                    }

                    // The ruler shares the waveform's time mapping and can be
                    // clicked or dragged to seek to the corresponding position.
                    if ruler_response.dragged() {
                        self.seeking = true;
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                    }
                    if ruler_response.drag_stopped() {
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                        }
                        self.seeking = false;
                        if let Err(error) = self.playback.seek(self.playback_position) {
                            eprintln!("Blaze: {error}");
                        }
                    } else if ruler_response.clicked() {
                        if let Some(position) = ruler_response.interact_pointer_pos() {
                            self.playback_position = pointer_time(position);
                            if let Err(error) = self.playback.seek(self.playback_position) {
                                eprintln!("Blaze: {error}");
                            }
                        }
                    }
                }

                // Keep the panel's requested height after a resize.
                ui.allocate_space(ui.available_size());
            });
        let timeline_split_rect = timeline_panel.response.rect;

        let tools_panel = egui::Panel::right("tools")
            .resizable(true)
            .default_size(310.0)
            .min_size(240.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut tabs = vec![ToolTab::ModeSelector];
                    if let Some(mode) = self.active_mode {
                        tabs.push(match mode {
                            EditingMode::Blaze => ToolTab::BlazeMode,
                            EditingMode::Precise => ToolTab::PreciseMode,
                        });
                    }
                    tabs.push(ToolTab::Export);

                    for tab in tabs {
                        if ui.selectable_label(self.active_tool_tab == tab, tab.label()).clicked() {
                            self.active_tool_tab = tab;
                        }
                    }
                });
                ui.separator();
                ui.add_space(8.0);

                match self.active_tool_tab {
                    ToolTab::ModeSelector => {
                        ui.heading("Choose an editing mode");
                        ui.add_space(12.0);

                        let blaze_ready = self.selected_path.is_none()
                            || (!self.keyframes_running && self.keyframe_error.is_none() && !self.keyframes.is_empty());
                        let blaze_button = egui::Button::new(
                            egui::RichText::new("⚡  Blaze Mode").strong().size(15.0)
                        ).min_size(egui::vec2(ui.available_width(), 48.0));
                        if ui.add_enabled(blaze_ready, blaze_button).clicked() {
                            let keyframes = self.keyframes.clone();
                            self.edit_regions = self.edit_regions.iter().filter_map(|region| {
                                let start = nearest_keyframe(&keyframes, region.start)?;
                                let end = nearest_boundary_keyframe(&keyframes, region.end, start, false)?;
                                (end > start).then_some(EditRegion { start, end })
                            }).collect();
                            self.pending_region_start = self.pending_region_start
                                .and_then(|start| nearest_keyframe(&keyframes, start));
                            self.active_mode = Some(EditingMode::Blaze);
                            self.active_tool_tab = ToolTab::BlazeMode;
                        }
                        ui.label("Keyframe-aligned cuts. Faster stream-copy editing at the cost of boundary precision.");
                        if self.keyframes_running && self.selected_path.is_some() {
                            ui.horizontal(|ui| { ui.spinner(); ui.small("Analysing GOP / keyframe structure…"); });
                        } else if let Some(error) = &self.keyframe_error {
                            ui.colored_label(egui::Color32::LIGHT_RED, format!("Keyframe analysis failed: {error}"));
                        } else if !self.keyframes.is_empty() {
                            ui.small(format!("{} video keyframes available.", self.keyframes.len()));
                        }

                        ui.add_space(16.0);
                        let precise_button = egui::Button::new(
                            egui::RichText::new("◎  Precise Mode").strong().size(15.0)
                        ).min_size(egui::vec2(ui.available_width(), 48.0));
                        if ui.add(precise_button).clicked() {
                            self.active_mode = Some(EditingMode::Precise);
                            self.active_tool_tab = ToolTab::PreciseMode;
                        }
                        ui.label("Unrestricted cut boundaries. Use when exact edit placement matters more than stream-copy constraints.");
                    }
                    ToolTab::BlazeMode | ToolTab::PreciseMode => {
                let expert_mode_changed = ui.checkbox(&mut self.expert_mode, "Expert Mode").changed();
                if expert_mode_changed && self.expert_mode {
                    // Initialize Expert Mode from the currently selected presets,
                    // rather than exposing unrelated hard-coded defaults.
                    let level = match self.detection_level {
                        0 => DetectionLevel::Comfortable,
                        1 => DetectionLevel::Optimal,
                        2 => DetectionLevel::Default,
                        3 => DetectionLevel::Tight,
                        _ => DetectionLevel::Aggressive,
                    };
                    self.custom_threshold_db = level.threshold_below_baseline_db();
                    let padding = match self.padding_level {
                        0 => PaddingPreset::Comfortable.milliseconds(),
                        2 => PaddingPreset::Tight.milliseconds(),
                        _ => PaddingPreset::Normal.milliseconds(),
                    };
                    self.custom_padding_before_ms = padding;
                    self.custom_padding_after_ms = padding;
                }
                ui.add_space(12.0);

                if self.expert_mode {
                    ui.strong("Detection Threshold");
                    ui.label("Level below the adaptive programme baseline");
                    ui.add(
                        egui::Slider::new(&mut self.custom_threshold_db, 3.0..=60.0)
                            .suffix(" dB")
                            .fixed_decimals(1),
                    );

                    ui.add_space(14.0);
                    ui.strong("Peak Ceiling");
                    ui.checkbox(&mut self.custom_peak_gate_enabled, "Enable peak ceiling");
                    if self.custom_peak_gate_enabled {
                        ui.add(
                            egui::Slider::new(&mut self.custom_peak_ceiling_dbfs, -90.0..=0.0)
                                .suffix(" dBFS")
                                .fixed_decimals(1),
                        );
                    }

                    ui.add_space(14.0);
                    ui.strong("Padding");
                    ui.label("Preserve audio around detected silence");
                    egui::Grid::new("expert_padding_controls")
                        .num_columns(2)
                        .spacing([12.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Before");
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut self.custom_padding_before_ms)
                                    .range(0..=2000)
                                    .speed(10));
                                ui.label("ms");
                            });
                            ui.end_row();

                            ui.label("After");
                            ui.horizontal(|ui| {
                                ui.add(egui::DragValue::new(&mut self.custom_padding_after_ms)
                                    .range(0..=2000)
                                    .speed(10));
                                ui.label("ms");
                            });
                            ui.end_row();
                        });

                    ui.add_space(14.0);
                    ui.strong("Minimum Silence Duration");
                    ui.horizontal(|ui| {
                        ui.add(egui::DragValue::new(&mut self.minimum_silence_ms)
                            .range(10..=10_000)
                            .speed(10));
                        ui.label("ms");
                    });
                } else {
                    ui.strong("Detection Level");
                    ui.add(
                        egui::Slider::new(&mut self.detection_level, 0..=4)
                            .step_by(1.0)
                            .show_value(false),
                    );
                    egui::Grid::new("detection_level_labels")
                        .num_columns(5)
                        .spacing([4.0, 4.0])
                        .show(ui, |ui| {
                            for (index, label) in ["Comfortable", "Optimal", "Default", "Tight", "Aggressive"].iter().enumerate() {
                                ui.label(egui::RichText::new(*label).size(10.0).color(
                                    if self.detection_level == index {
                                        ui.visuals().strong_text_color()
                                    } else {
                                        ui.visuals().weak_text_color()
                                    }
                                ));
                            }
                            ui.end_row();
                        });

                    ui.add_space(18.0);
                    ui.strong("Padding");
                    ui.add(
                        egui::Slider::new(&mut self.padding_level, 0..=2)
                            .step_by(1.0)
                            .show_value(false),
                    );
                    egui::Grid::new("padding_level_labels")
                        .num_columns(3)
                        .spacing([8.0, 4.0])
                        .show(ui, |ui| {
                            for (index, label) in ["Comfortable", "Normal", "Tight"].iter().enumerate() {
                                ui.label(egui::RichText::new(*label).size(10.0).color(
                                    if self.padding_level == index {
                                        ui.visuals().strong_text_color()
                                    } else {
                                        ui.visuals().weak_text_color()
                                    }
                                ));
                            }
                            ui.end_row();
                        });

                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        ui.strong("Minimum Silence Duration");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label("ms");
                            ui.add(egui::DragValue::new(&mut self.minimum_silence_ms)
                                .range(10..=10_000)
                                .speed(10));
                        });
                    });
                    ui.small("Default is 300 ms.");
                }

                ui.add_space(20.0);
                let blaze_ready = self.active_mode != Some(EditingMode::Blaze)
                    || (!self.keyframes_running && self.keyframe_error.is_none() && !self.keyframes.is_empty());
                let can_detect = self.selected_path.is_some()
                    && self.media_info.as_ref().is_some_and(|info| info.has_audio)
                    && !self.silence_running
                    && blaze_ready;
                if ui.add_enabled(
                    can_detect,
                    egui::Button::new(if self.silence_running { "Analysing Audio…" } else { "Detect Silence" })
                        .min_size(egui::vec2(ui.available_width(), 36.0)),
                ).clicked() {
                    self.start_silence_detection(ui.ctx());
                }
                if self.silence_running {
                    ui.add_space(6.0);
                    ui.spinner();
                    ui.small("Analysing audio. A rescan replaces existing regions.");
                }

                    }
                    ToolTab::Export => {
                        ui.heading("Export");
                        ui.add_space(12.0);

                        ui.strong("Video Export");
                        ui.add_space(6.0);

                        let blaze_reason = if self.selected_path.is_none() {
                            Some("Import a video to begin.")
                        } else if self.loading || self.media_info.is_none() {
                            Some("Wait for video analysis to finish.")
                        } else if self.active_mode != Some(EditingMode::Blaze) {
                            Some("Requires Blaze Mode. Switch to Blaze Mode to enable this export.")
                        } else if self.keyframes_running {
                            Some("Waiting for keyframe analysis to finish.")
                        } else if self.keyframe_error.is_some() || self.keyframes.is_empty() {
                            Some("Blaze Export requires usable video keyframes.")
                        } else if self.edit_regions.is_empty() {
                            Some("Create at least one cut region before exporting.")
                        } else {
                            None
                        };

                        let blaze_eligible = blaze_reason.is_none() && !self.export_running;
                        let blaze_clicked = ui.add_enabled(
                            blaze_eligible,
                            egui::Button::new(egui::RichText::new(
                                if self.export_running { "Blaze Export — Exporting…" } else { "Blaze Export" }
                            ).strong())
                                .min_size(egui::vec2(ui.available_width(), 38.0)),
                        ).clicked();

                        if let Some(reason) = blaze_reason {
                            ui.label(egui::RichText::new(reason).small().color(ui.visuals().weak_text_color()));
                        }
                        if blaze_clicked {
                            if let Some(source) = self.selected_path.clone() {
                                let extension = source.extension().and_then(|s| s.to_str()).unwrap_or("mkv").to_string();
                                let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
                                let filename = format!("{stem}_Blaze.{extension}");
                                if let Some(output) = FileDialog::new()
                                    .add_filter("Video", &[extension.as_str()])
                                    .set_file_name(&filename)
                                    .save_file()
                                {
                                    self.start_blaze_export(output, ui.ctx());
                                }
                            }
                        }

                        ui.add_space(8.0);
                        ui.add_enabled(
                            false,
                            egui::Button::new("Precision Export")
                                .min_size(egui::vec2(ui.available_width(), 38.0)),
                        );
                        ui.label(
                            egui::RichText::new("Coming Soon — re-encoded export for Precise Mode.")
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );

                        ui.add_space(22.0);
                        ui.separator();
                        ui.add_space(8.0);
                        ui.strong("Timestamp Export");
                        ui.label(
                            egui::RichText::new("Export the current edit as an editable NLE timeline.")
                                .small()
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.small("DaVinci Resolve and Premiere use FCP 7 XML interchange.");
                        ui.add_space(8.0);

                        let timestamp_available = self.selected_path.is_some()
                            && self.media_info.is_some()
                            && !self.loading
                            && !self.export_running;
                        let timestamp_formats = [
                            ("DaVinci Resolve XML", "xml", "DaVinci_Resolve", InterchangeFormat::DavinciResolve),
                            ("EDL", "edl", "EDL", InterchangeFormat::Edl),
                            ("FCPXML", "fcpxml", "FCPXML", InterchangeFormat::Fcpxml),
                            ("Premiere XML", "xml", "Premiere", InterchangeFormat::PremiereXml),
                        ];

                        for (label, extension, suffix, format) in timestamp_formats {
                            let clicked = ui.add_enabled(
                                timestamp_available,
                                egui::Button::new(label)
                                    .min_size(egui::vec2(ui.available_width(), 34.0)),
                            ).clicked();
                            if clicked {
                                if let Some(source) = self.selected_path.clone() {
                                    let stem = source.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
                                    let filename = format!("{stem}_{suffix}.{extension}");
                                    let output = FileDialog::new()
                                        .add_filter("Timeline interchange", &[extension])
                                        .set_file_name(&filename)
                                        .save_file();
                                    if let Some(output) = output {
                                        self.start_interchange_export(format, output, ui.ctx());
                                    }
                                }
                            }
                        }

                        if !timestamp_available {
                            ui.label(egui::RichText::new("Import a video to enable timestamp exports.")
                                .small().color(ui.visuals().weak_text_color()));
                        }

                        if self.export_running {
                            ui.add_space(8.0);
                            ui.horizontal(|ui| { ui.spinner(); ui.small("Preparing export…"); });
                        }
                        if let Some((success, message)) = &self.export_status {
                            ui.add_space(8.0);
                            let color = if *success { egui::Color32::LIGHT_GREEN } else { egui::Color32::LIGHT_RED };
                            ui.colored_label(color, message);
                        }
                    }
                }

                // Keep the resizable panel at the user-selected width instead of
                // allowing its contents to shrink it to their intrinsic width.
                ui.allocate_space(ui.available_size());
            });
        let tools_split_rect = tools_panel.response.rect;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(16, 17, 18)))
            .show(ui, |ui| {
                // One framed Preview Panel contains both the video viewport
                // and its transport bar. Tools remains a separate right panel.
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(16, 17, 18))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(42, 44, 47)))
                    .corner_radius(egui::CornerRadius::same(2))
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        let available = ui.available_rect_before_wrap();
                        let transport_height = 46.0_f32.min(available.height());
                        let preview_rect = egui::Rect::from_min_max(
                            available.min,
                            egui::pos2(available.max.x, available.max.y - transport_height),
                        );
                        let transport_rect = egui::Rect::from_min_max(
                            egui::pos2(available.min.x, preview_rect.max.y),
                            available.max,
                        );

                        ui.scope_builder(egui::UiBuilder::new().max_rect(preview_rect), |ui| {
                            let preview_rect = ui.max_rect();
                            ui.painter().rect_filled(
                                preview_rect,
                                egui::CornerRadius::same(3),
                                egui::Color32::from_rgb(10, 11, 12),
                            );

                            if self.selected_path.is_some() {
                                self.playback.paint(ui.painter(), preview_rect);
                            } else {
                                let button_rect = egui::Rect::from_center_size(
                                    preview_rect.center(),
                                    egui::vec2(156.0, 42.0),
                                );
                                if ui
                                    .put(button_rect, egui::Button::new("Import video"))
                                    .clicked()
                                {
                                    self.open_video(ui.ctx());
                                }
                            }
                        });

                        ui.scope_builder(egui::UiBuilder::new().max_rect(transport_rect), |ui| {
                            ui.painter().rect_filled(
                                ui.max_rect(),
                                0.0,
                                egui::Color32::from_rgb(16, 17, 18),
                            );

                            if self.selected_path.is_some() {
                                ui.with_layout(
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        ui.add_space(8.0);

                                        let paused = self.playback.is_paused();
                                        let icon = if paused { PlayerIcon::Play } else { PlayerIcon::Pause };
                                        let tooltip = if paused { "Play" } else { "Pause" };

                                        if allocated_icon_button(
                                            ui,
                                            "transport-play-pause",
                                            egui::vec2(34.0, 30.0),
                                            icon,
                                            tooltip,
                                        )
                                        .clicked()
                                        {
                                            if let Err(error) = self.playback.toggle_pause() {
                                                eprintln!("Blaze: {error}");
                                            }
                                        }

                                        ui.add_space(4.0);

                                        let duration = self.media_duration();
                                        self.playback_position = self.playback_position.clamp(0.0, duration);

                                        let time_label = format!(
                                            "{} / {}",
                                            format_time_millis(self.playback_position),
                                            format_time_millis(duration)
                                        );
                                        let time_font = egui::FontId::proportional(13.5);
                                        let time_width = ui
                                            .painter()
                                            .layout_no_wrap(
                                                time_label.clone(),
                                                time_font.clone(),
                                                ui.visuals().text_color(),
                                            )
                                            .size()
                                            .x
                                            + 2.0;
                                        let skip_button_width = 112.0;
                                        let slider_width = (ui.available_width()
                                            - time_width
                                            - skip_button_width
                                            - 34.0)
                                            .max(36.0);

                                        // Custom transport seekbar: a continuous orange fill,
                                        // dark remainder and a white circular handle.
                                        let (seek_id, seek_rect) =
                                            ui.allocate_space(egui::vec2(slider_width, 32.0));
                                        let seek_response = ui
                                            .interact(seek_rect, seek_id, egui::Sense::click_and_drag())
                                            .on_hover_text("Seek");
                                        let track_width = (seek_rect.width() - 18.0).max(1.0);
                                        let track = egui::Rect::from_center_size(
                                            seek_rect.center(),
                                            egui::vec2(track_width, 4.0),
                                        );

                                        let drag_stopped = seek_response.drag_stopped();
                                        let clicked = seek_response.clicked();
                                        let dragging = seek_response.dragged();

                                        if dragging || drag_stopped || clicked {
                                            if let Some(pointer) = seek_response.interact_pointer_pos() {
                                                let progress = ((pointer.x - track.left()) / track.width())
                                                    .clamp(0.0, 1.0) as f64;
                                                self.playback_position = progress * duration;

                                                // Keep the timeline viewport in sync with preview seeking.
                                                let visible_duration =
                                                    duration / self.timeline_zoom as f64;
                                                self.timeline_offset = (self.playback_position
                                                    - visible_duration * 0.5)
                                                    .clamp(0.0, (duration - visible_duration).max(0.0));
                                            }

                                            if drag_stopped || clicked {
                                                self.seeking = false;
                                                if let Err(error) = self.playback.seek(self.playback_position) {
                                                    eprintln!("Blaze: {error}");
                                                }
                                            } else {
                                                self.seeking = true;
                                            }
                                        }

                                        let progress =
                                            (self.playback_position / duration).clamp(0.0, 1.0) as f32;
                                        let thumb_x = track.left() + track.width() * progress;
                                        let track_center = track.center().y;
                                        let painter = ui.painter();
                                        painter.rect_filled(
                                            track,
                                            egui::CornerRadius::same(2),
                                            egui::Color32::from_rgb(67, 69, 72),
                                        );
                                        let filled_track = egui::Rect::from_min_max(
                                            track.min,
                                            egui::pos2(thumb_x, track.bottom()),
                                        );
                                        painter.rect_filled(
                                            filled_track,
                                            egui::CornerRadius::same(2),
                                            egui::Color32::from_rgb(255, 104, 79),
                                        );
                                        painter.circle_filled(
                                            egui::pos2(thumb_x, track_center),
                                            if seek_response.hovered() || seek_response.dragged() {
                                                8.0
                                            } else {
                                                7.0
                                            },
                                            egui::Color32::WHITE,
                                        );

                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [time_width, 26.0],
                                            egui::Label::new(
                                                egui::RichText::new(time_label)
                                                    .font(time_font),
                                            ),
                                        );

                                        ui.add_space(8.0);
                                        let skip_label = if self.skip_cuts_enabled {
                                            "Skip Cuts: ON"
                                        } else {
                                            "Skip Cuts: OFF"
                                        };
                                        let skip_button = egui::Button::new(
                                            egui::RichText::new(skip_label)
                                                .font(egui::FontId::proportional(13.5)),
                                        )
                                        .fill(if self.skip_cuts_enabled {
                                            egui::Color32::from_rgb(38, 111, 73)
                                        } else {
                                            egui::Color32::from_rgb(126, 48, 44)
                                        });
                                        if ui
                                            .add_sized([skip_button_width, 32.0], skip_button)
                                            .on_hover_text(
                                                "Automatically skip selected regions during playback",
                                            )
                                            .clicked()
                                        {
                                            self.skip_cuts_enabled = !self.skip_cuts_enabled;
                                        }
                                        // Keep a deliberate gap between the button and the preview panel edge.
                                        ui.add_space(16.0);
                                    },
                                );
                            }
                        });
                    });
            });

        // Read the actual native egui splitter responses. These widgets own the
        // drag and double-click gestures; no overlapping interaction layer is added.
        let tools_resize_response = ui
            .ctx()
            .read_response(ui.scope_id().with("tools").with("__resize"));
        if let Some(response) = &tools_resize_response {
            response.clone().on_hover_text("Drag to resize · Double-click to reset");
            if response.double_clicked() {
                let width = self.media_info.as_ref()
                    .map(|info| default_tools_width(root_rect, info))
                    .unwrap_or(310.0);
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, root_rect.height()),
                );
                let panel_id = ui.scope_id().with("tools");
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        panel_id,
                        egui::containers::panel::PanelState { outer_rect: panel_rect },
                    );
                });
                ui.ctx().request_repaint();
            }
        }

        let timeline_resize_response = ui
            .ctx()
            .read_response(ui.scope_id().with("timeline").with("__resize"));
        if let Some(response) = &timeline_resize_response {
            response.clone().on_hover_text("Drag to resize · Double-click to reset");
            if response.double_clicked() {
                let panel_id = ui.scope_id().with("timeline");
                let panel_rect = egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(root_rect.width(), 220.0),
                );
                ui.ctx().data_mut(|data| {
                    data.insert_persisted(
                        panel_id,
                        egui::containers::panel::PanelState { outer_rect: panel_rect },
                    );
                });
                ui.ctx().request_repaint();
            }
        }

        // Keep the compact visual grips centered on the real splitter edges.
        let handle_fill = egui::Color32::from_rgb(42, 45, 48);
        let handle_mark = egui::Color32::from_rgb(126, 130, 134);
        { let rect = tools_split_rect;
            let handle = egui::Rect::from_center_size(
                egui::pos2(rect.left(), rect.center().y),
                egui::vec2(10.0, 62.0),
            );
            let hovered = tools_resize_response.as_ref()
                .is_some_and(|response| response.hovered() || response.dragged());
            let fill = if hovered {
                egui::Color32::from_rgb(67, 71, 75)
            } else { handle_fill };
            ui.painter().rect_filled(handle, egui::CornerRadius::same(5), fill);
            for offset in [-8.0_f32, 0.0, 8.0] {
                ui.painter().circle_filled(
                    egui::pos2(handle.center().x, handle.center().y + offset),
                    1.25,
                    handle_mark,
                );
            }
        }
        { let rect = timeline_split_rect;
            let handle = egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top()),
                egui::vec2(62.0, 10.0),
            );
            let hovered = timeline_resize_response.as_ref()
                .is_some_and(|response| response.hovered() || response.dragged());
            let fill = if hovered {
                egui::Color32::from_rgb(67, 71, 75)
            } else { handle_fill };
            ui.painter().rect_filled(handle, egui::CornerRadius::same(5), fill);
            for offset in [-8.0_f32, 0.0, 8.0] {
                ui.painter().circle_filled(
                    egui::pos2(handle.center().x + offset, handle.center().y),
                    1.25,
                    handle_mark,
                );
            }
        }

        if self.show_help {
            egui::Window::new("Quick Help")
                .collapsible(false)
                .resizable(false)
                .default_width(430.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .open(&mut self.show_help)
                .show(ui.ctx(), |ui| {
                    ui.label("Keyboard shortcuts");
                    ui.add_space(6.0);
                    egui::Grid::new("blaze_quick_help_shortcuts")
                        .num_columns(2)
                        .spacing([18.0, 8.0])
                        .show(ui, |ui| {
                            ui.monospace("Ctrl + Scroll");
                            ui.label("Zoom timeline in / out");
                            ui.end_row();
                            ui.monospace("Double-click vertical splitter");
                            ui.label("Reset preview / tools proportions");
                            ui.end_row();
                            ui.monospace("Double-click timeline splitter");
                            ui.label("Reset timeline height");
                            ui.end_row();
                            ui.monospace("Scroll");
                            ui.label("Move timeline left / right");
                            ui.end_row();
                            ui.monospace("Space");
                            ui.label("Play / pause video");
                            ui.end_row();
                            ui.monospace("Ctrl + H");
                            ui.label("Open this help window");
                            ui.end_row();
                            ui.monospace("N");
                            ui.label("Create / cancel a starting jaw");
                            ui.end_row();
                            ui.monospace("M");
                            ui.label("Create an ending jaw");
                            ui.end_row();
                        });

                    ui.separator();
                    ui.label("Editing modes");
                    ui.add_space(6.0);
                    egui::Grid::new("blaze_quick_help_modes")
                        .num_columns(2)
                        .spacing([18.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Blaze Mode");
                            ui.label("Keyframe-aligned cuts with stream-copy constraints");
                            ui.end_row();
                            ui.strong("Precise Mode");
                            ui.label("Unrestricted boundary placement");
                            ui.end_row();
                            ui.strong("Export");
                            ui.label("Export video or an editable NLE timeline");
                            ui.end_row();
                        });
                    ui.add_space(10.0);
                    ui.label("You can reopen this guide anytime with Ctrl + H.");
                });
        }

        if !self.show_help && !self.first_run_help_seen {
            mark_first_run_help_seen();
            self.first_run_help_seen = true;
        }
    }

    fn on_exit(&mut self, gl: Option<&glow::Context>) {
        self.playback.shutdown(gl);
    }
}

#[derive(Clone, Copy)]
enum PlayerIcon {
    Play,
    Pause,
}

fn icon_button_at(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    icon: PlayerIcon,
    tooltip: &str,
) -> egui::Response {
    let response = ui.interact(rect, id, egui::Sense::click());
    paint_player_icon(ui, rect, &response, icon);
    response.on_hover_text(tooltip)
}

fn allocated_icon_button(
    ui: &mut egui::Ui,
    id_source: &'static str,
    size: egui::Vec2,
    icon: PlayerIcon,
    tooltip: &str,
) -> egui::Response {
    let (id, rect) = ui.allocate_space(size);
    icon_button_at(ui, rect, id.with(id_source), icon, tooltip)
}

fn paint_player_icon(
    ui: &egui::Ui,
    rect: egui::Rect,
    response: &egui::Response,
    icon: PlayerIcon,
) {
    let visuals = ui.visuals();
    let widget = if response.is_pointer_button_down_on() {
        &visuals.widgets.active
    } else if response.hovered() {
        &visuals.widgets.hovered
    } else {
        &visuals.widgets.inactive
    };

    let painter = ui.painter();
    painter.rect_filled(rect, egui::CornerRadius::same(5), widget.bg_fill);

    let center = rect.center();
    let color = visuals.text_color();
    match icon {
        PlayerIcon::Play => {
            painter.add(egui::Shape::convex_polygon(
                vec![
                    egui::pos2(center.x - 4.5, center.y - 7.0),
                    egui::pos2(center.x + 7.0, center.y),
                    egui::pos2(center.x - 4.5, center.y + 7.0),
                ],
                color,
                egui::Stroke::NONE,
            ));
        }
        PlayerIcon::Pause => {
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(center.x - 3.5, center.y),
                    egui::vec2(3.5, 14.0),
                ),
                egui::CornerRadius::same(1),
                color,
            );
            painter.rect_filled(
                egui::Rect::from_center_size(
                    egui::pos2(center.x + 3.5, center.y),
                    egui::vec2(3.5, 14.0),
                ),
                egui::CornerRadius::same(1),
                color,
            );
        }
    }
}

fn default_tools_width(root_rect: egui::Rect, info: &MediaInfo) -> f32 {
    let aspect = info
        .width
        .zip(info.height)
        .filter(|(width, height)| *width > 0 && *height > 0)
        .map(|(width, height)| width as f32 / height as f32)
        .unwrap_or(16.0 / 9.0);
    let preview_height = (root_rect.height() - 220.0 - 54.0).max(160.0);
    let preview_width = (preview_height * aspect + 14.0)
        .clamp(240.0, (root_rect.width() - 240.0).max(240.0));
    (root_rect.width() - preview_width).max(240.0)
}

fn format_ruler_time(seconds: f64, step: f64) -> String {
    let seconds = seconds.max(0.0);
    if step >= 60.0 {
        let whole_minutes = (seconds / 60.0).floor() as u64;
        let remainder = (seconds - whole_minutes as f64 * 60.0).round() as u64;
        if remainder == 0 {
            format!("{whole_minutes}m")
        } else {
            format!("{whole_minutes}m {remainder}s")
        }
    } else if step >= 1.0 {
        format!("{}s", seconds.round() as u64)
    } else {
        format!("{:.1}s", seconds)
    }
}

fn format_time_millis(seconds: f64) -> String {
    let total_millis = (seconds.max(0.0) * 1000.0).round() as u64;
    let hours = total_millis / 3_600_000;
    let minutes = (total_millis / 60_000) % 60;
    let seconds = (total_millis / 1000) % 60;
    let millis = total_millis % 1000;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}.{millis:03}")
    } else {
        format!("{minutes:02}:{seconds:02}.{millis:03}")
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Blaze — Ankush Sarkar")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([820.0, 580.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Blaze — Ankush Sarkar",
        options,
        Box::new(|cc| {
            let app = BlazeApp::new(cc)?;
            Ok(Box::new(app))
        }),
    )
}
