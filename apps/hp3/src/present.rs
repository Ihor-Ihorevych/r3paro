//! Getting finished frames to the screen on a thread of its own.
//!
//! The game draws into one of two off-screen images and hands it over; this thread waits for the display (the swap
//! chain's next image, which blocks until vsync lets one go), copies the frame in and presents it. The game thread
//! meanwhile already ticks the next frame instead of standing in that wait.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// Off-screen images in flight: one being shown, one being drawn.
const RING: usize = 2;

/// The shot clock: world ticks since the scripted cast began when there is one (`HP3_CAST`), else since the start.
fn elapsed() -> u64 {
    let now = TICKS.load(Ordering::Relaxed);
    match hp3_world::casttest::START_TICK.load(Ordering::Relaxed) {
        0 if std::env::var_os("HP3_CAST").is_some() => 0,
        s => now.saturating_sub(s as u64),
    }
}

/// World ticks so far (counted by the game loop), which `HP3_SHOT` frames are measured in: the display may run faster than the 60 Hz world.
pub static TICKS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// A screenshot asked for with F4: the file to write and the lines of text burned into the picture (where the player is).
/// The presenter takes it with the next finished frame.
#[cfg(not(target_os = "android"))]
pub static SHOT_REQUEST: std::sync::Mutex<Option<(std::path::PathBuf, Vec<String>)>> = std::sync::Mutex::new(None);

enum Msg {
    Frame { slot: usize, image: wgpu::Texture, width: u32, height: u32 },
    /// The window got a new native surface (the app came back from the background): present to this one from now on.
    Surface(wgpu::Surface<'static>),
    /// The window changed: configure the surface again.
    Reconfigure { width: u32, height: u32 },
    /// The picture adjustment (exponent for gamma, brightness added, contrast multiplier) from the game's display options.
    Look([f32; 3]),
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
    /// Set by the thread when the surface could not be set up (its window is not there): the game makes a new one.
    broken: Arc<AtomicBool>,
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
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        })
        .collect()
}

impl Presenter {
    /// Takes over the surface. `None` when the presenting thread cannot be started.
    pub fn new(
        surface: wgpu::Surface<'static>,
        caps: &wgpu::SurfaceCapabilities,
        device: wgpu::Device,
        queue: wgpu::Queue,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) -> Option<Presenter> {
        // A surface that cannot be copied into (OpenGL's) gets frames drawn into it instead.
        let can_copy = caps.usages.contains(wgpu::TextureUsages::COPY_DST);
        let alpha_mode = caps.alpha_modes[0];
        let (tx, rx) = mpsc::channel();
        let (free_tx, free) = mpsc::channel();
        for i in 0..RING {
            let _ = free_tx.send(i);
        }
        let back = free_tx.clone();
        let broken = Arc::new(AtomicBool::new(false));
        let thread_broken = broken.clone();
        let (thread_device, thread_queue) = (device.clone(), queue);
        let thread = std::thread::Builder::new()
            .name("hp3-present".into())
            .spawn(move || present_loop(surface, alpha_mode, can_copy, thread_device, thread_queue, format, rx, back, thread_broken))
            .ok()?;
        let (width, height) = (width.max(1), height.max(1));
        let images = make_images(&device, format, width, height);
        let views = images.iter().map(|t| t.create_view(&Default::default())).collect();
        let presenter = Presenter { device, format, images, views, size: (width, height), free, free_tx, tx, broken, thread: Some(thread) };
        presenter.reconfigure(width, height);
        Some(presenter)
    }

    /// Presents to a surface made for the window anew, as after the app was in the background: the old one belongs to a
    /// native window that is gone.
    pub fn replace_surface(&self, surface: wgpu::Surface<'static>) {
        let _ = self.tx.send(Msg::Surface(surface));
    }

    /// True once if the surface could not be set up since the last call.
    pub fn take_broken(&self) -> bool {
        self.broken.swap(false, Ordering::Relaxed)
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

    /// Sets the picture adjustment of the display options: `[gamma exponent, brightness, contrast]`, `[1, 0, 1]` for none.
    pub fn set_look(&self, look: [f32; 3]) {
        let _ = self.tx.send(Msg::Look(look));
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
    can_copy: bool,
    device: wgpu::Device,
    queue: wgpu::Queue,
    format: wgpu::TextureFormat,
    messages: Receiver<Msg>,
    free: Sender<usize>,
    broken: Arc<AtomicBool>,
) {
    // Configuring a surface can fail (its window may be gone or not there yet): that is reported, not a crash.
    let configure = |surface: &wgpu::Surface<'static>, width: u32, height: u32| -> bool {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        surface.configure(
            &device,
            &wgpu::SurfaceConfiguration {
                usage: if can_copy { wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_DST } else { wgpu::TextureUsages::RENDER_ATTACHMENT },
                format,
                width,
                height,
                present_mode: wgpu::PresentMode::AutoVsync,
                desired_maximum_frame_latency: 2,
                alpha_mode,
                view_formats: vec![],
            },
        );
        match pollster::block_on(device.pop_error_scope()) {
            Some(e) => {
                eprintln!("present: surface: {e}");
                broken.store(true, Ordering::Relaxed);
                false
            }
            None => true,
        }
    };
    let mut configured = (0, 0);
    let mut look = NEUTRAL;
    // Debug: `HP3_SHOT=file.png[@frame]` saves that frame (default the 300th) and quits, to look at what the game draws.
    #[cfg(not(target_os = "android"))]
    let shot: Option<(String, Vec<u64>)> = std::env::var("HP3_SHOT").ok().map(|s| match s.rsplit_once('@') {
        Some((p, n)) if n.split(',').all(|f| f.parse::<u64>().is_ok()) => (p.to_string(), n.split(',').map(|f| f.parse().unwrap()).collect()),
        _ => (s, vec![300]),
    });
    let mut frames_seen = 0u64;
    let mut adjust: Option<Adjust> = None;
    let fxaa = fxaa_enabled();
    for message in messages {
        match message {
            Msg::Quit => break,
            Msg::Look(l) => look = l,
            Msg::Surface(new) => {
                surface = new;
                configured = (0, 0);
            }
            Msg::Reconfigure { width, height } => {
                configured = if configure(&surface, width, height) { (width, height) } else { (0, 0) };
            }
            Msg::Frame { slot, image, width, height } => {
                #[cfg(not(target_os = "android"))]
                if let Some((path, lines)) = SHOT_REQUEST.lock().ok().and_then(|mut r| r.take()) {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    save_shot(&device, &queue, &image, format, width, height, &path.to_string_lossy(), &lines);
                }
                #[cfg(not(target_os = "android"))]
                if let Some((path, at)) = &shot
                    && let Some(&due) = at.get(frames_seen as usize)
                    && elapsed() >= due
                {
                    let file = if at.len() > 1 { format!("{}-{due}.png", path.trim_end_matches(".png")) } else { path.clone() };
                    save_shot(&device, &queue, &image, format, width, height, &file, &[]);
                    frames_seen += 1;
                    if frames_seen as usize == at.len() {
                        std::process::exit(0);
                    }
                }
                if configured != (width, height) {
                    configured = if configure(&surface, width, height) { (width, height) } else { (0, 0) };
                }
                let frame = match if configured == (0, 0) { Err(wgpu::SurfaceError::Lost) } else { surface.get_current_texture() } {
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
                    if can_copy && look == NEUTRAL && !fxaa {
                        encoder.copy_texture_to_texture(image.as_image_copy(), frame.texture.as_image_copy(), wgpu::Extent3d { width: width.min(frame.texture.width()), height: height.min(frame.texture.height()), depth_or_array_layers: 1 });
                    } else {
                        let a = adjust.get_or_insert_with(|| Adjust::new(&device, format));
                        a.draw(&device, &queue, &mut encoder, &image, &frame.texture, look, fxaa);
                    }
                    queue.submit([encoder.finish()]);
                    frame.present();
                }
                let _ = free.send(slot);
            }
        }
    }
}

/// Whether the shown frame goes through FXAA (edge smoothing). `HP3_FXAA=0|1` decides; by default on for the desktop and
/// off on the phone, where the extra full-screen pass costs more than a plain copy.
fn fxaa_enabled() -> bool {
    match std::env::var("HP3_FXAA").ok().as_deref() {
        Some("0" | "off" | "false") => false,
        Some(_) => true,
        None => !cfg!(target_os = "android"),
    }
}

/// No picture adjustment: gamma exponent 1, nothing added, contrast 1.
const NEUTRAL: [f32; 3] = [1.0, 0.0, 1.0];

/// The pass that shows a frame through the display options: `(c - 0.5) * contrast + 0.5 + brightness`, then the gamma
/// exponent.
struct Adjust {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    sampler: wgpu::Sampler,
}

const ADJUST_SHADER: &str = "
struct Params { look: vec4<f32> };
@group(0) @binding(0) var frame: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var smp: sampler;
@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}
fn luma(c: vec3<f32>) -> f32 { return dot(c, vec3<f32>(0.299, 0.587, 0.114)); }
// FXAA (the compact edge-direction blur): finds a contrast edge around the pixel and averages along it.
fn fxaa(pos: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let px = 1.0 / size;
    let uv = pos * px;
    let rgbM = textureSampleLevel(frame, smp, uv, 0.0).rgb;
    let rgbNW = textureSampleLevel(frame, smp, uv + vec2<f32>(-1.0, -1.0) * px, 0.0).rgb;
    let rgbNE = textureSampleLevel(frame, smp, uv + vec2<f32>(1.0, -1.0) * px, 0.0).rgb;
    let rgbSW = textureSampleLevel(frame, smp, uv + vec2<f32>(-1.0, 1.0) * px, 0.0).rgb;
    let rgbSE = textureSampleLevel(frame, smp, uv + vec2<f32>(1.0, 1.0) * px, 0.0).rgb;
    let lM = luma(rgbM); let lNW = luma(rgbNW); let lNE = luma(rgbNE); let lSW = luma(rgbSW); let lSE = luma(rgbSE);
    let lMin = min(lM, min(min(lNW, lNE), min(lSW, lSE)));
    let lMax = max(lM, max(max(lNW, lNE), max(lSW, lSE)));
    if (lMax - lMin < max(0.0312, lMax * 0.125)) { return rgbM; }
    var dir = vec2<f32>(-((lNW + lNE) - (lSW + lSE)), (lNW + lSW) - (lNE + lSE));
    let reduce = max((lNW + lNE + lSW + lSE) * 0.25 * 0.125, 1.0 / 128.0);
    let rcp = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * rcp, vec2<f32>(-8.0), vec2<f32>(8.0)) * px;
    let a = 0.5 * (textureSampleLevel(frame, smp, uv + dir * (1.0 / 3.0 - 0.5), 0.0).rgb + textureSampleLevel(frame, smp, uv + dir * (2.0 / 3.0 - 0.5), 0.0).rgb);
    let b = a * 0.5 + 0.25 * (textureSampleLevel(frame, smp, uv + dir * -0.5, 0.0).rgb + textureSampleLevel(frame, smp, uv + dir * 0.5, 0.0).rgb);
    let lB = luma(b);
    if (lB < lMin || lB > lMax) { return a; }
    return b;
}
@fragment fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = vec2<i32>(textureDimensions(frame));
    var c = textureLoad(frame, min(vec2<i32>(pos.xy), size - vec2<i32>(1)), 0).rgb;
    if (params.look.w > 0.5) { c = fxaa(pos.xy, vec2<f32>(size)); }
    var v = (c - vec3<f32>(0.5)) * params.look.z + vec3<f32>(0.5 + params.look.y);
    v = pow(clamp(v, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(params.look.x));
    return vec4<f32>(v, 1.0);
}
";

impl Adjust {
    fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Adjust {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("adjust"), source: wgpu::ShaderSource::Wgsl(ADJUST_SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("adjust"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("adjust"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("adjust"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState { module: &module, entry_point: Some("vs"), compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });
        let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("adjust"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        Adjust { pipeline, layout, params, sampler }
    }

    fn draw(&self, device: &wgpu::Device, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder, image: &wgpu::Texture, target: &wgpu::Texture, look: [f32; 3], fxaa: bool) {
        let bytes: Vec<u8> = [look[0], look[1], look[2], if fxaa { 1.0f32 } else { 0.0 }].iter().flat_map(|v| v.to_le_bytes()).collect();
        queue.write_buffer(&self.params, 0, &bytes);
        let view = image.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("adjust"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: self.params.as_entire_binding() }, wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) }],
        });
        let target_view = target.create_view(&Default::default());
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("adjust"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &target_view, resolve_target: None, depth_slice: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

/// Writes a finished frame to a PNG (debugging: see `HP3_SHOT`).
#[cfg(not(target_os = "android"))]
fn save_shot(device: &wgpu::Device, queue: &wgpu::Queue, image: &wgpu::Texture, format: wgpu::TextureFormat, width: u32, height: u32, path: &str, caption: &[String]) {
    let bytes_per_row = (width * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shot"),
        size: (bytes_per_row * height) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        image.as_image_copy(),
        wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(height) } },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    let _ = device.poll(wgpu::PollType::Wait);
    let data = slice.get_mapped_range();
    let bgr = matches!(format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for row in 0..height {
        let start = (row * bytes_per_row) as usize;
        for px in data[start..start + (width * 4) as usize].chunks_exact(4) {
            let p: [u8; 4] = if bgr { [px[2], px[1], px[0], 255] } else { [px[0], px[1], px[2], 255] };
            rgba.extend_from_slice(&p);
        }
    }
    stamp(&mut rgba, width, height, caption);
    let result = std::fs::File::create(path).map_err(|e| e.to_string()).and_then(|f| {
        let mut encoder = png::Encoder::new(std::io::BufWriter::new(f), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header().and_then(|mut w| w.write_image_data(&rgba)).map_err(|e| e.to_string())
    });
    match result {
        Ok(()) => eprintln!("shot: wrote {path} ({width}x{height})"),
        Err(e) => eprintln!("shot: {path}: {e}"),
    }
}

/// 5x7 glyphs for ASCII 0x20..=0x5F (columns, bit 0 on top): enough for the coordinates stamped on a screenshot.
#[cfg(not(target_os = "android"))]
const FONT: [[u8; 5]; 64] = [
    [0x00, 0x00, 0x00, 0x00, 0x00], [0x00, 0x00, 0x5F, 0x00, 0x00], [0x00, 0x07, 0x00, 0x07, 0x00], [0x14, 0x7F, 0x14, 0x7F, 0x14],
    [0x24, 0x2A, 0x7F, 0x2A, 0x12], [0x23, 0x13, 0x08, 0x64, 0x62], [0x36, 0x49, 0x55, 0x22, 0x50], [0x00, 0x05, 0x03, 0x00, 0x00],
    [0x00, 0x1C, 0x22, 0x41, 0x00], [0x00, 0x41, 0x22, 0x1C, 0x00], [0x14, 0x08, 0x3E, 0x08, 0x14], [0x08, 0x08, 0x3E, 0x08, 0x08],
    [0x00, 0x50, 0x30, 0x00, 0x00], [0x08, 0x08, 0x08, 0x08, 0x08], [0x00, 0x60, 0x60, 0x00, 0x00], [0x20, 0x10, 0x08, 0x04, 0x02],
    [0x3E, 0x51, 0x49, 0x45, 0x3E], [0x00, 0x42, 0x7F, 0x40, 0x00], [0x42, 0x61, 0x51, 0x49, 0x46], [0x21, 0x41, 0x45, 0x4B, 0x31],
    [0x18, 0x14, 0x12, 0x7F, 0x10], [0x27, 0x45, 0x45, 0x45, 0x39], [0x3C, 0x4A, 0x49, 0x49, 0x30], [0x01, 0x71, 0x09, 0x05, 0x03],
    [0x36, 0x49, 0x49, 0x49, 0x36], [0x06, 0x49, 0x49, 0x29, 0x1E], [0x00, 0x36, 0x36, 0x00, 0x00], [0x00, 0x56, 0x36, 0x00, 0x00],
    [0x08, 0x14, 0x22, 0x41, 0x00], [0x14, 0x14, 0x14, 0x14, 0x14], [0x00, 0x41, 0x22, 0x14, 0x08], [0x02, 0x01, 0x51, 0x09, 0x06],
    [0x32, 0x49, 0x79, 0x41, 0x3E], [0x7E, 0x11, 0x11, 0x11, 0x7E], [0x7F, 0x49, 0x49, 0x49, 0x36], [0x3E, 0x41, 0x41, 0x41, 0x22],
    [0x7F, 0x41, 0x41, 0x22, 0x1C], [0x7F, 0x49, 0x49, 0x49, 0x41], [0x7F, 0x09, 0x09, 0x09, 0x01], [0x3E, 0x41, 0x49, 0x49, 0x7A],
    [0x7F, 0x08, 0x08, 0x08, 0x7F], [0x00, 0x41, 0x7F, 0x41, 0x00], [0x20, 0x40, 0x41, 0x3F, 0x01], [0x7F, 0x08, 0x14, 0x22, 0x41],
    [0x7F, 0x40, 0x40, 0x40, 0x40], [0x7F, 0x02, 0x0C, 0x02, 0x7F], [0x7F, 0x04, 0x08, 0x10, 0x7F], [0x3E, 0x41, 0x41, 0x41, 0x3E],
    [0x7F, 0x09, 0x09, 0x09, 0x06], [0x3E, 0x41, 0x51, 0x21, 0x5E], [0x7F, 0x09, 0x19, 0x29, 0x46], [0x46, 0x49, 0x49, 0x49, 0x31],
    [0x01, 0x01, 0x7F, 0x01, 0x01], [0x3F, 0x40, 0x40, 0x40, 0x3F], [0x1F, 0x20, 0x40, 0x20, 0x1F], [0x3F, 0x40, 0x38, 0x40, 0x3F],
    [0x63, 0x14, 0x08, 0x14, 0x63], [0x07, 0x08, 0x70, 0x08, 0x07], [0x61, 0x51, 0x49, 0x45, 0x43], [0x00, 0x7F, 0x41, 0x41, 0x00],
    [0x02, 0x04, 0x08, 0x10, 0x20], [0x00, 0x41, 0x41, 0x7F, 0x00], [0x04, 0x02, 0x01, 0x02, 0x04], [0x40, 0x40, 0x40, 0x40, 0x40],
];

/// Burns the lines of text into the top left corner of an RGBA picture (yellow on a dark box).
#[cfg(not(target_os = "android"))]
fn stamp(rgba: &mut [u8], width: u32, height: u32, lines: &[String]) {
    if lines.is_empty() {
        return;
    }
    let scale = (height / 360).max(2);
    let (cell_w, cell_h) = (6 * scale, 9 * scale);
    let chars = lines.iter().map(|l| l.chars().count() as u32).max().unwrap_or(0);
    let (box_w, box_h) = ((chars * cell_w + 2 * scale * 2).min(width), (lines.len() as u32 * cell_h + 2 * scale * 2).min(height));
    let mut put = |x: u32, y: u32, c: [u8; 3], blend: bool| {
        if x < width && y < height {
            let i = ((y * width + x) * 4) as usize;
            for k in 0..3 {
                rgba[i + k] = if blend { (rgba[i + k] as u32 / 3) as u8 } else { c[k] };
            }
        }
    };
    for y in 0..box_h {
        for x in 0..box_w {
            put(x, y, [0; 3], true);
        }
    }
    for (row, line) in lines.iter().enumerate() {
        for (col, ch) in line.chars().enumerate() {
            let code = ch.to_ascii_uppercase() as u32;
            let glyph = if (0x20..=0x5F).contains(&code) { FONT[(code - 0x20) as usize] } else { FONT[0x1F] };
            let (ox, oy) = (2 * scale + col as u32 * cell_w, 2 * scale + row as u32 * cell_h);
            for (gx, bits) in glyph.iter().enumerate() {
                for gy in 0..7u32 {
                    if bits >> gy & 1 == 1 {
                        for dy in 0..scale {
                            for dx in 0..scale {
                                put(ox + gx as u32 * scale + dx, oy + gy * scale + dy, [255, 230, 80], false);
                            }
                        }
                    }
                }
            }
        }
    }
}
