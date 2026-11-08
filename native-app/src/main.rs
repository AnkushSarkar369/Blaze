use std::{
    fs,
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use blaze_engine::{MediaEngine, MediaInfo};
use eframe::egui;
use playback::Playback;
use rfd::FileDialog;

mod playback;
mod waveform;

use waveform::Waveform;

#[derive(Clone, Copy, PartialEq, Eq)]
enum ToolTab {
    BlazeMode,
    PreciseMode,
    Export,
}

impl ToolTab {
    const ALL: [Self; 3] = [Self::BlazeMode, Self::PreciseMode, Self::Export];

    fn label(self) -> &'static str {
        match self {
            Self::BlazeMode => "Blaze Mode",
            Self::PreciseMode => "Precise Mode",
            Self::Export => "Export",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::BlazeMode => "Fast, keyframe-aligned cutting for near-instant edits.",
            Self::PreciseMode => "Frame-accurate cutting for edits that need exact timing.",
            Self::Export => "Configure and export the finished video.",
        }
    }
}

fn help_seen_path() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join(".config")
            .join("blaze")
            .join("first_run_help_seen")
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

type ProbeResult = (u64, Result<(PathBuf, MediaInfo), String>);
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
    timeline_zoom: f32,
    timeline_offset: f64,
    active_tool_tab: ToolTab,
    show_help: bool,
    first_run_help_seen: bool,
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
            timeline_zoom: 1.0,
            timeline_offset: 0.0,
            active_tool_tab: ToolTab::BlazeMode,
            show_help: !has_seen_first_run_help(),
            first_run_help_seen: has_seen_first_run_help(),
        })
    }

    fn open_video(&mut self, ctx: &egui::Context) {
        let Some(path) = FileDialog::new().pick_file() else {
            return;
        };

        self.probe_generation = self.probe_generation.wrapping_add(1);
        let generation = self.probe_generation;
        self.loading = true;
        self.seeking = false;
        self.playback_position = 0.0;
        self.timeline_zoom = 1.0;
        self.timeline_offset = 0.0;
        self.media_info = None;
        self.waveform = None;
        self.waveform_progress = Some(0.0);
        self.selected_path = Some(path.clone());

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

        // Global transport shortcut: handle Space at the app level rather
        // than inside a particular panel, so it works regardless of focus
        // or which part of the interface the user is interacting with.
        let space_pressed = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::NONE, egui::Key::Space)
        });
        let help_shortcut = ctx.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::H)
        });
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

                let duration = self
                    .media_info
                    .as_ref()
                    .map(|info| info.duration)
                    .filter(|duration| duration.is_finite() && *duration > 0.0)
                    .unwrap_or_else(|| self.playback.duration())
                    .max(0.01);

                let can_seek = self.selected_path.is_some() && self.waveform_progress.is_none();
                let sense = if can_seek {
                    egui::Sense::click_and_drag()
                } else {
                    egui::Sense::hover()
                };
                let response = ui.allocate_rect(lane, sense);

                // Timeline navigation is deliberately modifier-based so ordinary
                // wheel input does not interfere with the rest of the interface.
                if can_seek && response.hovered() {
                    let (modifiers, scroll, zoom_delta) = ui.input(|input| {
                        (input.modifiers, input.smooth_scroll_delta, input.zoom_delta())
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
                    } else if modifiers.alt && scroll.y.abs() > 0.0 {
                        self.timeline_offset -= scroll.y as f64 * visible_duration * 0.0015;
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

                if can_seek {
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
                    for tab in ToolTab::ALL {
                        if ui
                            .selectable_label(self.active_tool_tab == tab, tab.label())
                            .clicked()
                        {
                            self.active_tool_tab = tab;
                        }
                    }
                });
                ui.separator();
                ui.add_space(8.0);
                ui.label(self.active_tool_tab.description());
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

                                        let player_duration = self.playback.duration();
                                        let probed_duration = self
                                            .media_info
                                            .as_ref()
                                            .map(|info| info.duration)
                                            .unwrap_or(0.0);
                                        let duration = if player_duration.is_finite() && player_duration > 0.0 {
                                            player_duration
                                        } else {
                                            probed_duration
                                        }
                                        .max(0.01);
                                        self.playback_position = self.playback_position.clamp(0.0, duration);

                                        let time_label = format!(
                                            "{} / {}",
                                            format_time(self.playback_position),
                                            format_time(duration)
                                        );
                                        let time_width = if duration >= 3600.0 { 148.0 } else { 104.0 };
                                        let slider_width = (ui.available_width() - time_width - 28.0).max(40.0);

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
                        });

                    ui.separator();
                    ui.label("Editing modes");
                    ui.add_space(6.0);
                    egui::Grid::new("blaze_quick_help_modes")
                        .num_columns(2)
                        .spacing([18.0, 8.0])
                        .show(ui, |ui| {
                            ui.strong("Blaze Mode");
                            ui.label("Instant, keyframe-aligned cuts");
                            ui.end_row();
                            ui.strong("Precise Mode");
                            ui.label("Frame-accurate cuts");
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
