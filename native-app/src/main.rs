use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use blaze_engine::{MediaEngine, MediaInfo};
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

impl EditingMode {
    fn label(self) -> &'static str {
        match self {
            Self::Blaze => "Blaze Mode",
            Self::Precise => "Precise Mode",
        }
    }
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
        visuals.panel_fill = egui::Color32::from_rgb(22, 23, 25);
        visuals.window_fill = egui::Color32::from_rgb(25, 26, 28);
        visuals.extreme_bg_color = egui::Color32::from_rgb(14, 15, 17);
        visuals.faint_bg_color = egui::Color32::from_rgb(31, 33, 36);
        visuals.widgets.noninteractive.bg_fill = egui::Color32::from_rgb(29, 31, 34);
        visuals.widgets.inactive.bg_fill = egui::Color32::from_rgb(38, 40, 43);
        visuals.widgets.hovered.bg_fill = egui::Color32::from_rgb(51, 54, 58);
        visuals.widgets.active.bg_fill = egui::Color32::from_rgb(218, 105, 49);
        cc.egui_ctx.set_visuals(visuals);

        let playback = Playback::new(cc)?;
        let (probe_tx, probe_rx) = mpsc::channel();
        let (waveform_tx, waveform_rx) = mpsc::channel();
        let (silence_tx, silence_rx) = mpsc::channel();
        let (keyframe_tx, keyframe_rx) = mpsc::channel();

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
}

impl eframe::App for BlazeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_probe(ctx);
        self.poll_waveform();
        self.poll_silence();
        self.poll_keyframes(ctx);

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
        egui::Panel::bottom("timeline")
            .resizable(true)
            .default_size(220.0)
            .min_size(140.0)
            .show(ui, |ui| {
                let available = ui.available_rect_before_wrap();
                let lane = available.shrink2(egui::vec2(10.0, 8.0));

                let duration = self.media_duration();

                let can_seek = self.selected_path.is_some() && self.waveform_progress.is_none();
                let sense = if can_seek {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::hover()
                };
                let response = ui.allocate_rect(lane, sense);

                // Ctrl+wheel zooms; ordinary wheel input pans the timeline
                // horizontally. Horizontal trackpad input is also supported.
                if can_seek && response.hovered() {
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

                ui.painter().rect_filled(
                    lane,
                    egui::CornerRadius::same(3),
                    egui::Color32::from_rgb(17, 19, 21),
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
                            egui::Color32::from_rgb(105, 55, 37),
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
                        egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 61, 62)),
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
                            egui::Color32::from_rgba_unmultiplied(190, 66, 62, 105),
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
                            egui::Color32::from_rgb(255, 190, 145)
                        } else {
                            egui::Color32::from_rgb(238, 115, 83)
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
                            egui::Stroke::new(3.0, egui::Color32::from_rgb(235, 165, 75)),
                        );
                    }
                }

                if self.selected_path.is_some() && self.waveform_progress.is_none()
                    && self.playback_position >= self.timeline_offset
                    && self.playback_position <= self.timeline_offset + visible_duration
                {
                    let progress = ((self.playback_position - self.timeline_offset) / visible_duration) as f32;
                    let playhead_x = lane.left() + lane.width() * progress;
                    ui.painter().line_segment(
                        [
                            egui::pos2(playhead_x, lane.top()),
                            egui::pos2(playhead_x, lane.bottom()),
                        ],
                        egui::Stroke::new(1.5, egui::Color32::from_rgb(235, 133, 83)),
                    );
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
                }

                // Keep the panel's requested height after a resize.
                ui.allocate_space(ui.available_size());
            });

        egui::Panel::right("tools")
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
                ui.checkbox(&mut self.expert_mode, "Expert Mode");
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
                        ui.label("Export configuration will appear here.");
                    }
                }

                // Keep the resizable panel at the user-selected width instead of
                // allowing its contents to shrink it to their intrinsic width.
                ui.allocate_space(ui.available_size());
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(14, 15, 17)))
            .show(ui, |ui| {
                // One framed Preview Panel contains both the video viewport
                // and its transport bar. Tools remains a separate right panel.
                egui::Frame::new()
                    .fill(egui::Color32::from_rgb(17, 18, 20))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(48, 50, 53)))
                    .corner_radius(egui::CornerRadius::same(6))
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
                                egui::Color32::from_rgb(8, 9, 10),
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
                                egui::Color32::from_rgb(17, 18, 20),
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

                                        ui.add_space(10.0);

                                        let duration = self.media_duration();
                                        self.playback_position = self.playback_position.clamp(0.0, duration);

                                        let time_label = format!(
                                            "{} / {}",
                                            format_time(self.playback_position),
                                            format_time(duration)
                                        );
                                        let time_width = if duration >= 3600.0 { 148.0 } else { 104.0 };
                                        let skip_button_width = 112.0;
                                        let slider_width = (ui.available_width()
                                            - time_width
                                            - skip_button_width
                                            - 48.0)
                                            .max(32.0);

                                        // egui Slider has its own preferred width. Override it
                                        // locally so the seek track truly fills the preview.
                                        let mut slider_response = None;
                                        ui.scope(|ui| {
                                            ui.spacing_mut().slider_width = slider_width;
                                            slider_response = Some(
                                                ui.add_sized(
                                                    [slider_width, 22.0],
                                                    egui::Slider::new(
                                                        &mut self.playback_position,
                                                        0.0..=duration,
                                                    )
                                                    .show_value(false),
                                                )
                                                .on_hover_text("Seek"),
                                            );
                                        });
                                        let response = slider_response.expect("slider scope always runs");

                                        if response.dragged() {
                                            self.seeking = true;
                                        }

                                        // Preview seeking always moves the timeline viewport
                                        // with it, even when the target is still inside the old
                                        // viewport. Keep the seek position centred where possible.
                                        if response.changed() {
                                            let visible_duration = duration / self.timeline_zoom as f64;
                                            self.timeline_offset = (self.playback_position
                                                - visible_duration * 0.5)
                                                .clamp(0.0, (duration - visible_duration).max(0.0));
                                        }

                                        if response.drag_stopped()
                                            || (response.changed() && !response.dragged() && !self.seeking)
                                        {
                                            self.seeking = false;
                                            if let Err(error) = self.playback.seek(self.playback_position) {
                                                eprintln!("Blaze: {error}");
                                            }
                                        }

                                        ui.add_space(10.0);
                                        ui.add_sized(
                                            [time_width, 22.0],
                                            egui::Label::new(
                                                egui::RichText::new(time_label).monospace().small(),
                                            ),
                                        );

                                        ui.add_space(8.0);
                                        let skip_button = egui::Button::new(if self.skip_cuts_enabled {
                                            "Skip Cuts: ON"
                                        } else {
                                            "Skip Cuts: OFF"
                                        })
                                        .fill(if self.skip_cuts_enabled {
                                            egui::Color32::from_rgb(35, 112, 72)
                                        } else {
                                            egui::Color32::from_rgb(130, 55, 48)
                                        });
                                        if ui
                                            .add_sized([skip_button_width, 26.0], skip_button)
                                            .on_hover_text(
                                                "Automatically skip all completed N–M cut regions during playback",
                                            )
                                            .clicked()
                                        {
                                            self.skip_cuts_enabled = !self.skip_cuts_enabled;
                                        }
                                        ui.add_space(8.0);
                                    },
                                );
                            }
                        });
                    });
            });

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
                            ui.monospace("Alt + Scroll");
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
                            ui.label("Render the finished video");
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

fn format_time(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;

    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Blaze")
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([820.0, 580.0]),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };

    eframe::run_native(
        "Blaze",
        options,
        Box::new(|cc| {
            let app = BlazeApp::new(cc)?;
            Ok(Box::new(app))
        }),
    )
}
