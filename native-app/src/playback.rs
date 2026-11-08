use std::{
    ffi::CString,
    io,
    os::raw::c_void,
    path::Path,
    sync::{Arc, Mutex, Weak},
};

use eframe::egui;
use rsmpv::{
    render::{OpenGlFbo, OwnedRenderContext},
    Mpv,
};

pub struct Playback {
    // Drop the GL renderer before the player core.
    renderer: Option<Arc<Mutex<OwnedRenderContext>>>,
    mpv: Arc<Mpv>,
}

impl Playback {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let proc_loader = cc.get_proc_address.clone().ok_or_else(|| {
            io::Error::other("Blaze requires eframe's OpenGL function loader for libmpv rendering")
        })?;

        let mpv = Arc::new(
            Mpv::builder()?
                .set_property("vo", "libmpv")?
                .set_property("hwdec", "auto-safe")?
                .set_property("keep-open", "yes")?
                .set_property("osc", "no")?
                .set_property("osd-level", "0")?
                .set_property("input-default-bindings", "no")?
                .set_property("input-vo-keyboard", "no")?
                .build()?,
        );

        let get_proc_address = Box::new(move |name: &str| -> *mut c_void {
            let name = CString::new(name).expect("OpenGL function names cannot contain NUL");
            proc_loader(&name) as *mut c_void
        });

        // eframe creates the Glow context before invoking the app creator.
        // All renderer calls and its destruction happen from the same GL-backed UI thread.
        let mut renderer = unsafe {
            OwnedRenderContext::new_opengl(mpv.clone(), false, get_proc_address)?
        };

        let repaint = cc.egui_ctx.clone();
        renderer.set_update_callback(move || repaint.request_repaint());

        Ok(Self {
            renderer: Some(Arc::new(Mutex::new(renderer))),
            mpv,
        })
    }

    pub fn load(&self, path: &Path) -> Result<(), String> {
        let path = path.to_string_lossy();
        self.mpv
            .command(&["loadfile", path.as_ref(), "replace"])
            .map_err(|error| format!("MPV could not open this file: {error}"))
    }

    pub fn toggle_pause(&self) -> Result<(), String> {
        let paused = self.mpv.get_property::<bool>("pause").unwrap_or(false);
        self.mpv
            .set_property("pause", !paused)
            .map_err(|error| format!("Playback control failed: {error}"))
    }

    pub fn seek(&self, seconds: f64) -> Result<(), String> {
        let seconds = seconds.max(0.0).to_string();
        self.mpv
            .command(&["seek", &seconds, "absolute+exact"])
            .map_err(|error| format!("Seeking failed: {error}"))
    }

    pub fn position(&self) -> f64 {
        self.mpv.get_property::<f64>("time-pos").unwrap_or(0.0)
    }

    pub fn duration(&self) -> f64 {
        self.mpv.get_property::<f64>("duration").unwrap_or(0.0)
    }

    pub fn is_paused(&self) -> bool {
        self.mpv.get_property::<bool>("pause").unwrap_or(true)
    }

    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(renderer) = self.renderer.as_ref().map(Arc::downgrade) else {
            return;
        };

        let callback = egui_glow::painter::CallbackFn::new(move |info, _painter| {
            let Some(renderer) = renderer.upgrade() else {
                return;
            };
            let viewport = info.viewport_in_pixels();
            if viewport.width_px <= 0 || viewport.height_px <= 0 {
                return;
            }

            let Ok(mut renderer) = renderer.lock() else {
                return;
            };

            if renderer.update() {
                let target = OpenGlFbo {
                    fbo: 0,
                    width: viewport.width_px,
                    height: viewport.height_px,
                    internal_format: 0,
                };
                if let Err(error) = renderer.render_opengl(target, true, false) {
                    eprintln!("Blaze: libmpv video rendering failed: {error}");
                }
            }
        });

        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }

    pub fn shutdown(&mut self) {
        if let Some(renderer) = self.renderer.take() {
            if let Ok(mut renderer) = renderer.lock() {
                renderer.clear_update_callback();
            }
            drop(renderer);
        }
    }
}
