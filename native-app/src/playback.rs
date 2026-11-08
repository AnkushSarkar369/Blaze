use std::{
    ffi::CString,
    io,
    os::raw::c_void,
    path::Path,
    sync::{Arc, Mutex},
};

use eframe::egui;
use glow::HasContext;
use rsmpv::{
    render::{OpenGlFbo, OwnedRenderContext},
    Event, Mpv,
};

#[derive(Default)]
struct VideoSurface {
    framebuffer: Option<glow::Framebuffer>,
    texture: Option<glow::Texture>,
    size: [i32; 2],
    has_frame: bool,
}

impl VideoSurface {
    fn ensure(
        &mut self,
        gl: &glow::Context,
        width: i32,
        height: i32,
    ) -> Result<bool, String> {
        if self.size == [width, height] && self.framebuffer.is_some() {
            return Ok(false);
        }

        self.destroy(gl);

        unsafe {
            let texture = gl.create_texture().map_err(|error| error.to_string())?;
            gl.bind_texture(glow::TEXTURE_2D, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MIN_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_2D,
                glow::TEXTURE_MAG_FILTER,
                glow::LINEAR as i32,
            );
            gl.tex_image_2d(
                glow::TEXTURE_2D,
                0,
                glow::RGBA8 as i32,
                width,
                height,
                0,
                glow::RGBA,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(None),
            );

            let framebuffer = match gl.create_framebuffer() {
                Ok(framebuffer) => framebuffer,
                Err(error) => {
                    gl.delete_texture(texture);
                    gl.bind_texture(glow::TEXTURE_2D, None);
                    return Err(error);
                }
            };

            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            gl.framebuffer_texture_2d(
                glow::FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::TEXTURE_2D,
                Some(texture),
                0,
            );
            let status = gl.check_framebuffer_status(glow::FRAMEBUFFER);
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);

            if status != glow::FRAMEBUFFER_COMPLETE {
                gl.delete_framebuffer(framebuffer);
                gl.delete_texture(texture);
                return Err(format!("Video framebuffer is incomplete (0x{status:04x})"));
            }

            self.texture = Some(texture);
            self.framebuffer = Some(framebuffer);
            self.size = [width, height];
            self.has_frame = false;
        }

        Ok(true)
    }

    fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            if let Some(framebuffer) = self.framebuffer.take() {
                gl.delete_framebuffer(framebuffer);
            }
            if let Some(texture) = self.texture.take() {
                gl.delete_texture(texture);
            }
        }
        self.size = [0, 0];
        self.has_frame = false;
    }
}

pub struct Playback {
    // Drop the GL renderer before the player core.
    renderer: Option<Arc<Mutex<OwnedRenderContext>>>,
    surface: Option<Arc<Mutex<VideoSurface>>>,
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
                .set_property("hwdec", "vaapi-copy")?
                // Haswell VA-API exposes MPEG-2, H.264 and VC-1 decode profiles.
                // HEVC and other unsupported codecs deliberately fall back to software.
                .set_property("hwdec-codecs", "h264,mpeg2video,vc1")?
                // Add a small buffer cushion for low-power systems without changing
                // the source audio or resampling it.
                .set_property("audio-buffer", 0.30_f64)?
                // Keep audio/video in sync on weak CPUs by dropping late decoded frames.
                .set_property("framedrop", "decoder+vo")?
                .set_property("video-timing-offset", 0.0_f64)?
                .set_property("keep-open", "yes")?
                .set_property("osc", false)?
                .set_property("osd-level", 0_i64)?
                .set_property("input-default-bindings", false)?
                .set_property("input-vo-keyboard", false)?
                .build()?,
        );
        mpv.request_log_messages("warn")?;

        let get_proc_address = Box::new(move |name: &str| -> *mut c_void {
            let name = CString::new(name).expect("OpenGL function names cannot contain NUL");
            proc_loader(&name) as *mut c_void
        });

        // eframe creates the Glow context before invoking the app creator.
        // Renderer calls and destruction stay on the GL-backed UI thread.
        let mut renderer = unsafe {
            OwnedRenderContext::new_opengl(mpv.clone(), false, get_proc_address)?
        };

        let repaint = cc.egui_ctx.clone();
        renderer.set_update_callback(move || repaint.request_repaint());

        Ok(Self {
            renderer: Some(Arc::new(Mutex::new(renderer))),
            surface: Some(Arc::new(Mutex::new(VideoSurface::default()))),
            mpv,
        })
    }

    pub fn load(&self, path: &Path) -> Result<(), String> {
        let path = path.to_string_lossy();
        self.mpv
            .command(&["loadfile", path.as_ref(), "replace"])
            .map_err(|error| format!("MPV could not open this file: {error}"))?;
        self.mpv
            .set_property("pause", false)
            .map_err(|error| format!("MPV could not start playback: {error}"))?;

        if let Some(surface) = &self.surface {
            if let Ok(mut surface) = surface.lock() {
                surface.has_frame = false;
            }
        }
        Ok(())
    }

    pub fn stop(&self) {
        let _ = self.mpv.command(&["stop"]);
        if let Some(surface) = &self.surface {
            if let Ok(mut surface) = surface.lock() {
                surface.has_frame = false;
            }
        }
    }

    pub fn toggle_pause(&self) -> Result<(), String> {
        let at_end = self
            .mpv
            .get_property::<bool>("eof-reached")
            .unwrap_or(false);

        if at_end {
            self.seek(0.0)?;
            return self
                .mpv
                .set_property("pause", false)
                .map_err(|error| format!("Could not restart playback: {error}"));
        }

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
        self.mpv
            .get_property::<bool>("pause")
            .unwrap_or(true)
            || self
                .mpv
                .get_property::<bool>("eof-reached")
                .unwrap_or(false)
    }

    pub fn poll_error(&self) -> Option<String> {
        let mut latest_error = None;
        while let Some(event) = self.mpv.poll_event() {
            match event {
                Event::LogMessage(message) => {
                    // Optional hardware-backend failures can be logged even when
                    // software decoding successfully continues. Keep diagnostics in
                    // the terminal; only EndFile events are surfaced as failures.
                    eprintln!(
                        "Blaze MPV [{}] {}: {}",
                        message.level,
                        message.prefix,
                        message.text.trim()
                    );
                }
                Event::EndFile {
                    error: Some(error),
                    ..
                } => {
                    latest_error = Some(format!("Playback ended with an error: {error}"));
                }
                _ => {}
            }
        }
        latest_error
    }

    pub fn paint(&self, painter: &egui::Painter, rect: egui::Rect) {
        let Some(renderer) = self.renderer.as_ref().map(Arc::downgrade) else {
            return;
        };
        let Some(surface) = self.surface.as_ref().map(Arc::downgrade) else {
            return;
        };

        let callback = egui_glow::painter::CallbackFn::new(move |info, painter| {
            let (Some(renderer), Some(surface)) = (renderer.upgrade(), surface.upgrade()) else {
                return;
            };

            let viewport = info.viewport_in_pixels();
            let width = viewport.width_px;
            let height = viewport.height_px;
            if width <= 0 || height <= 0 {
                return;
            }

            let gl = painter.gl();
            let Ok(mut surface) = surface.lock() else {
                return;
            };

            let target_framebuffer = painter.intermediate_fbo();
            let ensure_result = surface.ensure(gl, width, height);
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, target_framebuffer);
            }
            let resized = match ensure_result {
                Ok(resized) => resized,
                Err(error) => {
                    eprintln!("Blaze: could not allocate video surface: {error}");
                    return;
                }
            };

            let Ok(mut renderer) = renderer.lock() else {
                return;
            };

            if resized || renderer.update() {
                let Some(framebuffer) = surface.framebuffer else {
                    return;
                };
                let target = OpenGlFbo {
                    fbo: framebuffer.0.get() as i32,
                    width,
                    height,
                    internal_format: glow::RGBA8 as i32,
                };

                match renderer.render_opengl(target, true, false) {
                    Ok(()) => surface.has_frame = true,
                    Err(error) => {
                        surface.has_frame = false;
                        eprintln!("Blaze: libmpv video rendering failed: {error}");
                    }
                }
            }

            if surface.has_frame {
                unsafe {
                    let scissor_was_enabled = gl.is_enabled(glow::SCISSOR_TEST);
                    let mut previous_scissor_box = [0_i32; 4];
                    gl.get_parameter_i32_slice(glow::SCISSOR_BOX, &mut previous_scissor_box);

                    gl.enable(glow::SCISSOR_TEST);
                    gl.scissor(
                        viewport.left_px,
                        viewport.from_bottom_px,
                        width,
                        height,
                    );
                    gl.bind_framebuffer(glow::READ_FRAMEBUFFER, surface.framebuffer);
                    gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, target_framebuffer);
                    gl.blit_framebuffer(
                        0,
                        0,
                        width,
                        height,
                        viewport.left_px,
                        viewport.from_bottom_px,
                        viewport.left_px + width,
                        viewport.from_bottom_px + height,
                        glow::COLOR_BUFFER_BIT,
                        glow::LINEAR,
                    );

                    gl.scissor(
                        previous_scissor_box[0],
                        previous_scissor_box[1],
                        previous_scissor_box[2],
                        previous_scissor_box[3],
                    );
                    if !scissor_was_enabled {
                        gl.disable(glow::SCISSOR_TEST);
                    }
                }
            }
            unsafe {
                gl.bind_framebuffer(glow::FRAMEBUFFER, target_framebuffer);
            }
        });

        painter.add(egui::Shape::Callback(egui::PaintCallback {
            rect,
            callback: Arc::new(callback),
        }));
    }

    pub fn shutdown(&mut self, gl: Option<&glow::Context>) {
        if let Some(renderer) = self.renderer.take() {
            if let Ok(mut renderer) = renderer.lock() {
                renderer.clear_update_callback();
            }
            drop(renderer);
        }

        if let Some(surface) = self.surface.take() {
            if let Some(gl) = gl {
                if let Ok(mut surface) = surface.lock() {
                    surface.destroy(gl);
                }
            }
            drop(surface);
        }
    }
}
