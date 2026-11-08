use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use blaze_engine::{MediaEngine, MediaInfo};
use eframe::egui;
use playback::Playback;
use rfd::FileDialog;

mod playback;

type ProbeResult = (u64, Result<(PathBuf, MediaInfo), String>);

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
        self.media_info = None;
        self.selected_path = Some(path.clone());

        if let Err(error) = self.playback.load(&path) {
            eprintln!("Blaze: {error}");
            self.playback.stop();
            self.selected_path = None;
            self.loading = false;
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

    fn poll_probe(&mut self) {
        while let Ok((generation, result)) = self.probe_rx.try_recv() {
            if generation != self.probe_generation {
                continue;
            }

            match result {
                Ok((path, info)) => {
                    self.selected_path = Some(path);
                    self.media_info = Some(info);
                    self.loading = false;
                }
                Err(error) => {
                    self.loading = false;
                    self.selected_path = None;
                    self.media_info = None;
                    self.playback_position = 0.0;
                    self.playback.stop();
                    eprintln!("Blaze: media analysis failed: {error}");
                }
            }
        }
    }
}

impl eframe::App for BlazeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_probe();

        if let Some(error) = self.playback.poll_error() {
            eprintln!("Blaze: {error}");
            self.loading = false;
            self.selected_path = None;
            self.media_info = None;
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
                ui.heading("Timeline");
                ui.add_space(12.0);
                let available = ui.available_rect_before_wrap();
                let lane = egui::Rect::from_min_size(
                    egui::pos2(available.left(), available.center().y - 28.0),
                    egui::vec2(available.width(), 56.0),
                );
                ui.painter().rect_filled(
                    lane,
                    egui::CornerRadius::same(3),
                    egui::Color32::from_rgb(17, 19, 21),
                );
                ui.painter().line_segment(
                    [lane.left_center(), lane.right_center()],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 61, 62)),
                );
                ui.allocate_rect(lane, egui::Sense::hover());

            });

        egui::Panel::right("tools")
            .resizable(true)
            .default_size(310.0)
            .min_size(240.0)
            .show(ui, |ui| {
                ui.heading("Tools");
                ui.separator();
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(14, 15, 17)))
            .show(ui, |ui| {
                // Preview and transport form one column. The right Tools panel is
                // outside this central area, so both automatically share its width.
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

                ui.allocate_ui_at_rect(preview_rect, |ui| {
                    let preview_rect = ui.max_rect();
                    ui.painter().rect_filled(
                        preview_rect,
                        egui::CornerRadius::same(4),
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

                ui.allocate_ui_at_rect(transport_rect, |ui| {
                    ui.painter().rect_filled(
                        ui.max_rect(),
                        0.0,
                        egui::Color32::from_rgb(22, 23, 25),
                    );

                    if self.selected_path.is_some() {
                        ui.with_layout(
                            egui::Layout::left_to_right(egui::Align::Center),
                            |ui| {
                                ui.add_space(12.0);

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

                                ui.add_space(12.0);

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
                                let remaining = ui.available_width() - time_width - 36.0;
                                let slider_width = remaining.max(40.0);

                                let response = ui.add_sized(
                                    [slider_width, 22.0],
                                    egui::Slider::new(&mut self.playback_position, 0.0..=duration)
                                        .show_value(false),
                                ).on_hover_text("Seek");

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

                                ui.add_space(12.0);
                                ui.add_sized(
                                    [time_width, 22.0],
                                    egui::Label::new(
                                        egui::RichText::new(time_label).monospace().small(),
                                    ),
                                );
                                ui.add_space(12.0);
                            },
                        );
                    }
                });
            });
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
    let stroke = egui::Stroke::new(1.8, color);

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
