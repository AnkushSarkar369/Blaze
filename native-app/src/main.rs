use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};

use blaze_engine::{MediaEngine, MediaInfo};
use eframe::egui;
use rfd::FileDialog;

type ProbeResult = Result<(PathBuf, MediaInfo), String>;

struct BlazeApp {
    selected_path: Option<PathBuf>,
    media_info: Option<MediaInfo>,
    loading: bool,
    status: String,
    probe_tx: Sender<ProbeResult>,
    probe_rx: Receiver<ProbeResult>,
}

impl BlazeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
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

        let (probe_tx, probe_rx) = mpsc::channel();

        Self {
            selected_path: None,
            media_info: None,
            loading: false,
            status: "Ready".to_owned(),
            probe_tx,
            probe_rx,
        }
    }

    fn open_video(&mut self, ctx: &egui::Context) {
        let Some(path) = FileDialog::new()
            .add_filter("Video files", &["mp4", "mkv", "mov", "webm", "avi", "m4v", "mts", "m2ts"])
            .pick_file()
        else {
            return;
        };

        self.loading = true;
        self.status = "Reading media information…".to_owned();
        self.media_info = None;
        self.selected_path = Some(path.clone());

        let sender = self.probe_tx.clone();
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let tools = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../BIN");
            let engine = MediaEngine::new([tools]);
            let result = engine
                .probe_media(&path.to_string_lossy())
                .map(|info| (path, info));
            let _ = sender.send(result);
            repaint.request_repaint();
        });
    }

    fn poll_probe(&mut self) {
        match self.probe_rx.try_recv() {
            Ok(Ok((path, info))) => {
                self.selected_path = Some(path);
                self.media_info = Some(info);
                self.loading = false;
                self.status = "Media loaded".to_owned();
            }
            Ok(Err(error)) => {
                self.loading = false;
                self.status = error;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.loading = false;
                self.status = "Media analysis worker stopped unexpectedly".to_owned();
            }
        }
    }

    fn filename(&self) -> &str {
        self.selected_path
            .as_deref()
            .and_then(PathBuf::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("No footage loaded")
    }
}

impl eframe::App for BlazeApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_probe();
        if self.loading {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar")
            .exact_height(48.0)
            .show_inside(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.heading(egui::RichText::new("BLAZE").strong().size(18.0));
                    ui.separator();
                    if ui.button("Open video").clicked() {
                        self.open_video(ui.ctx());
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(self.filename()).color(egui::Color32::LIGHT_GRAY));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(egui::RichText::new("NATIVE BUILD").small().weak());
                    });
                });
            });

        egui::TopBottomPanel::bottom("timeline")
            .resizable(true)
            .default_height(220.0)
            .min_height(140.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.heading("Timeline");
                    ui.separator();
                    ui.label("Single visual waveform lane");
                });
                ui.add_space(12.0);
                let available = ui.available_rect_before_wrap();
                let lane = egui::Rect::from_min_size(
                    egui::pos2(available.left(), available.center().y - 28.0),
                    egui::vec2(available.width(), 56.0),
                );
                ui.painter().rect_filled(
                    lane,
                    3.0,
                    egui::Color32::from_rgb(17, 19, 21),
                );
                ui.painter().line_segment(
                    [lane.left_center(), lane.right_center()],
                    egui::Stroke::new(1.0, egui::Color32::from_rgb(55, 61, 62)),
                );
                ui.allocate_rect(lane, egui::Sense::hover());
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Timeline interaction and waveform rendering are not connected yet.")
                        .small()
                        .weak(),
                );
            });

        egui::SidePanel::right("tools")
            .resizable(true)
            .default_width(310.0)
            .min_width(240.0)
            .show_inside(ui, |ui| {
                ui.heading("Tools");
                ui.separator();
                ui.add_space(8.0);
                ui.label("The existing Tools workflow is intentionally not being migrated in this stage.");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("It comes after native playback and the timeline are working.")
                        .small()
                        .weak(),
                );
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::from_rgb(14, 15, 17)))
            .show_inside(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(18.0);
                    ui.label(egui::RichText::new("PREVIEW").small().weak());
                    ui.add_space(12.0);

                    let preview = ui.available_rect_before_wrap();
                    ui.painter().rect_filled(
                        preview.shrink(8.0),
                        4.0,
                        egui::Color32::from_rgb(8, 9, 10),
                    );

                    ui.allocate_ui_at_rect(preview.shrink(8.0), |ui| {
                        ui.centered_and_justified(|ui| {
                            if self.loading {
                                ui.spinner();
                            } else if let Some(info) = &self.media_info {
                                ui.vertical_centered(|ui| {
                                    ui.heading(egui::RichText::new(self.filename()).size(20.0));
                                    ui.add_space(8.0);
                                    let dimensions = match (info.width, info.height) {
                                        (Some(w), Some(h)) => format!("{w} × {h}"),
                                        _ => "Unknown dimensions".to_owned(),
                                    };
                                    let fps = info.fps.map(|v| format!("{v:.3} fps")).unwrap_or_else(|| "Unknown frame rate".to_owned());
                                    ui.label(format!("{dimensions}  ·  {fps}"));
                                    ui.label(format!("Duration: {:.2} seconds", info.duration));
                                    ui.add_space(12.0);
                                    ui.label("Media probing works. Native video playback is the next integration.");
                                });
                            } else {
                                ui.vertical_centered(|ui| {
                                    ui.heading("No footage loaded");
                                    ui.add_space(6.0);
                                    ui.label("Open a video to inspect its media information.");
                                });
                            }
                        });
                    });
                });
            });

        egui::TopBottomPanel::bottom("status")
            .exact_height(24.0)
            .show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&self.status).small().weak());
                    if let Some(info) = &self.media_info {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let audio = if info.has_audio {
                                format!("Audio · {} ch", info.audio_channels.unwrap_or(0))
                            } else {
                                "No audio stream".to_owned()
                            };
                            ui.label(egui::RichText::new(audio).small().weak());
                        });
                    }
                });
            });
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
        Box::new(|cc| Ok(Box::new(BlazeApp::new(cc)))),
    )
}
