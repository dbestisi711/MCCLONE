//! wgpu renderer, chunk meshing, chunk streaming.
//!
//! OWNER: optimization & visuals agent. Public API used by `mc-game` (keep
//! stable, add freely):
//! - [`Renderer::new`] (windowed), [`Renderer::new_offscreen`] (headless, for screenshots/tests)
//! - [`Renderer::resize`], [`Renderer::render`], [`Renderer::capture`]
//! - [`ChunkStreamer`]: background world generation + loading/unloading around the player
//!
//! Stub: clears to the sky colour; streamer generates chunks synchronously.

use std::sync::Arc;

use mc_assets::Assets;
use mc_core::render_types::FrameData;
use mc_core::{ChunkPos, Rgba8Image, World};
use mc_worldgen::WorldGenerator;
use winit::window::Window;

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    pub chunks_drawn: u32,
    pub chunks_meshed: u32,
    pub triangles: u64,
    pub draw_calls: u32,
    pub gpu_name_hash: u64,
}

enum Target {
    Window {
        surface: wgpu::Surface<'static>,
        config: wgpu::SurfaceConfiguration,
    },
    Offscreen {
        texture: wgpu::Texture,
        width: u32,
        height: u32,
    },
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    pub stats: RenderStats,
    pub adapter_info: String,
    #[allow(dead_code)]
    assets: Arc<Assets>,
}

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

impl Renderer {
    /// Create a renderer drawing into a window.
    pub fn new(window: Arc<Window>, assets: Arc<Assets>) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance
            .create_surface(window.clone())
            .expect("create surface");
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no GPU adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("request device");
        let size = window.inner_size();
        let config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        surface.configure(&device, &config);
        let info = adapter.get_info();
        Renderer {
            device,
            queue,
            target: Target::Window { surface, config },
            stats: RenderStats::default(),
            adapter_info: format!("{} ({:?})", info.name, info.backend),
            assets,
        }
    }

    /// Create a headless renderer (uses any adapter, including software ones like lavapipe).
    pub fn new_offscreen(assets: Arc<Assets>, width: u32, height: u32) -> Self {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
            apply_limit_buckets: false,
        }))
        .expect("no GPU adapter");
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .expect("request device");
        let texture = Self::make_offscreen(&device, width, height);
        let info = adapter.get_info();
        Renderer {
            device,
            queue,
            target: Target::Offscreen {
                texture,
                width,
                height,
            },
            stats: RenderStats::default(),
            adapter_info: format!("{} ({:?})", info.name, info.backend),
            assets,
        }
    }

    fn make_offscreen(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    pub fn size(&self) -> (u32, u32) {
        match &self.target {
            Target::Window { config, .. } => (config.width, config.height),
            Target::Offscreen { width, height, .. } => (*width, *height),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        let (width, height) = (width.max(1), height.max(1));
        match &mut self.target {
            Target::Window { surface, config } => {
                config.width = width;
                config.height = height;
                surface.configure(&self.device, config);
            }
            Target::Offscreen {
                texture,
                width: w,
                height: h,
            } => {
                *texture = Self::make_offscreen(&self.device, width, height);
                *w = width;
                *h = height;
            }
        }
    }

    /// Draw one frame: world (chunks from `world`), entities, then UI.
    pub fn render(&mut self, world: &World, frame: &FrameData) {
        let _ = world;
        let sky = frame.sky.sky_color;
        let d = frame.sky.daylight as f64;
        let clear = wgpu::Color {
            r: ((sky >> 16) & 255) as f64 / 255.0 * d,
            g: ((sky >> 8) & 255) as f64 / 255.0 * d,
            b: (sky & 255) as f64 / 255.0 * d,
            a: 1.0,
        };
        let (surface_tex, view) = match &self.target {
            Target::Window { surface, config } => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                    let v = t
                        .texture
                        .create_view(&wgpu::TextureViewDescriptor::default());
                    (Some(t), v)
                }
                _ => {
                    surface.configure(&self.device, config);
                    return;
                }
            },
            Target::Offscreen { texture, .. } => (
                None,
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
            ),
        };
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let _pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        self.queue.submit([enc.finish()]);
        if let Some(t) = surface_tex {
            self.queue.present(t);
        }
    }

    /// Read back the last rendered frame (offscreen renderers only).
    pub fn capture(&mut self) -> Option<Rgba8Image> {
        let Target::Offscreen {
            texture,
            width,
            height,
        } = &self.target
        else {
            return None;
        };
        let (w, h) = (*width, *height);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("capture"),
            size: (bpr * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buf,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("map capture buffer");
        let mut img = Rgba8Image::new(w, h);
        for y in 0..h {
            let row = &data[(y * bpr) as usize..(y * bpr + w * 4) as usize];
            img.data[(y * w * 4) as usize..((y + 1) * w * 4) as usize].copy_from_slice(row);
        }
        Some(img)
    }
}

/// Loads/generates chunks around the player and unloads far ones.
pub struct ChunkStreamer {
    pub generator: Arc<WorldGenerator>,
    /// Radius in chunks.
    pub render_distance: i32,
}

impl ChunkStreamer {
    pub fn new(generator: Arc<WorldGenerator>, render_distance: i32) -> Self {
        ChunkStreamer {
            generator,
            render_distance,
        }
    }

    /// Called every frame. Kicks off generation for missing chunks near
    /// `center`, inserts finished ones into `world`, unloads far ones.
    pub fn update(&mut self, world: &mut World, center: ChunkPos) {
        let r = self.render_distance;
        let mut budget = 4;
        for d in 0..=r {
            for dz in -d..=d {
                for dx in -d..=d {
                    if dx.abs() != d && dz.abs() != d {
                        continue;
                    }
                    let p = center.offset(dx, dz);
                    if !world.has_chunk(p) && budget > 0 {
                        world.insert_chunk(self.generator.generate(p));
                        budget -= 1;
                    }
                }
            }
        }
        let far: Vec<_> = world
            .chunk_positions()
            .filter(|p| p.chebyshev(center) > r + 2)
            .collect();
        for p in far {
            world.remove_chunk(p);
        }
    }

    /// Block until every chunk within `radius` of `center` is loaded (startup / screenshots).
    pub fn load_blocking(&mut self, world: &mut World, center: ChunkPos, radius: i32) {
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let p = center.offset(dx, dz);
                if !world.has_chunk(p) {
                    world.insert_chunk(self.generator.generate(p));
                }
            }
        }
    }
}
