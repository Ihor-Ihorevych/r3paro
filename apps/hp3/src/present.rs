//! Getting finished frames to the screen on a thread of its own.
//!
//! The game draws into one of two off-screen images and hands it over; this thread waits for the display (the swap
//! chain's next image, which blocks until vsync lets one go), copies the frame in and presents it. The game thread
//! meanwhile already ticks the next frame instead of standing in that wait.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// Off-screen images in flight: one being shown, one being drawn.
const RING: usize = 2;

enum Msg {
    Frame { slot: usize, image: wgpu::Texture, width: u32, height: u32 },
    /// The window got a new native surface (the app came back from the background): present to this one from now on.
    Surface(wgpu::Surface<'static>),
    /// The window changed: configure the surface again.
    Reconfigure { width: u32, height: u32 },
    Quit,
}

pub struct Presenter {
    device: wgpu::Device,
    format: wgpu::TextureFormat,
    images: Vec<wgpu::Texture>,
    views: Vec<wgpu::TextureView>,
    size: (u32, u32),
    free: Receiver<usize>,
    free_tx: Sender<usize>,
    tx: Sender<Msg>,
    thread: Option<JoinHandle<()>>,
}

fn make_images(device: &wgpu::Device, format: wgpu::TextureFormat, width: u32, height: u32) -> Vec<wgpu::Texture> {
    (0..RING)
        .map(|_| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("frame"),
                size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        })
        .collect()
}

impl Presenter {
    /// Takes over the surface. `None` when the surface cannot be a copy target (the caller then has no way to present).
    pub fn new(
        surface: wgpu::Surface<'static>,
        caps: &wgpu::SurfaceCapabilities,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Option<Presenter> {
        if !caps.usages.contains(wgpu::TextureUsages::COPY_DST) {
            return None;
        }
        let alpha_mode = caps.alpha_modes[0];
        let (tx, rx) = mpsc::channel();
        let (free_tx, free) = mpsc::channel();
        for i in 0..RING {
            let _ = free_tx.send(i);
        }
        let back = free_tx.clone();
        let (thread_device, thread_queue) = (device.clone(), queue);
        let thread = std::thread::Builder::new()
            .name("hp3-present".into())
            .spawn(move || present_loop(surface, alpha_mode, thread_device, thread_queue, format, rx, back))
            .ok()?;
        let (width, height) = (width.max(1), height.max(1));
        let images = make_images(&device, format, width, height);
        let views = images.iter().map(|t| t.create_view(&Default::default())).collect();
        let presenter = Presenter { device, format, images, views, size: (width, height), free, free_tx, tx, thread: Some(thread) };
        presenter.reconfigure(width, height);
        Some(presenter)
    }

    /// Presents to a surface made for the window anew, as after the app was in the background: the old one belongs to a
    /// native window that is gone.
    pub fn replace_surface(&self, surface: wgpu::Surface<'static>) {
        let _ = self.tx.send(Msg::Surface(surface));
    }

    /// Configures the surface for this size again (after a resize or the window coming back).
    pub fn reconfigure(&self, width: u32, height: u32) {
        let _ = self.tx.send(Msg::Reconfigure { width: width.max(1), height: height.max(1) });
    }

    /// New off-screen images for a new window size.
    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        self.images = make_images(&self.device, self.format, width, height);
        self.views = self.images.iter().map(|t| t.create_view(&Default::default())).collect();
        self.size = (width, height);
        self.reconfigure(width, height);
    }

    /// Waits for an off-screen image that is not on its way to the screen any more.
    pub fn take(&self) -> usize {
        self.free.recv().unwrap_or(0)
    }

    pub fn view(&self, slot: usize) -> &wgpu::TextureView {
        &self.views[slot]
    }

    /// Hands a drawn image over to be shown.
    pub fn show(&self, slot: usize) {
        let image = self.images[slot].clone();
        let (width, height) = self.size;
        if self.tx.send(Msg::Frame { slot, image, width, height }).is_err() {
            let _ = self.free_tx.send(slot);
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn present_loop(
    mut surface: wgpu::Surface<'static>,
    alpha_mode: wgpu::CompositeAlphaMode,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    messages: Receiver<Msg>,
    free: Sender<usize>,
) {
    let configure = |surface: &wgpu::Surface<'static>, width: u32, height: u32| {
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST,
                format,
                width,
                height,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode,
                view_formats: vec![],
            },
        );
    };
    let mut configured = (0, 0);
    for message in messages {
        match message {
            Msg::Quit => break,
            Msg::Surface(new) => {
                surface = new;
                configured = (0, 0);
            }
            Msg::Reconfigure { width, height } => {
                configure(&surface, width, height);
                configured = (width, height);
            }
            Msg::Frame { slot, image, width, height } => {
                if configured != (width, height) {
                    configure(&surface, width, height);
                    configured = (width, height);
                }
                let frame = match surface.get_current_texture() {
                    // Configuring a surface whose window is gone is an error: the frame is dropped and the surface is set
                    // up again when the next one arrives or the window gives a new one.
                    Err(wgpu::SurfaceError::Outdated | wgpu::SurfaceError::Lost) => {
                        configured = (0, 0);
                        None
                    }
                    Ok(f) => Some(f),
                    Err(e) => {
                        eprintln!("present: {e}");
                        None
                    }
                };
                if let Some(frame) = frame {
                    let mut encoder = device.create_command_encoder(&Default::default());
                    encoder.copy_texture_to_texture(image.as_image_copy(), frame.texture.as_image_copy(), wgpu::Extent3d { width: width.min(frame.texture.width()), height: height.min(frame.texture.height()), depth_or_array_layers: 1 });
                    queue.submit([encoder.finish()]);
                    frame.present();
                }
                let _ = free.send(slot);
            }
        }
    }
}
