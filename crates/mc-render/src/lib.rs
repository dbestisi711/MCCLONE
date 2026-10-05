//! wgpu renderer, chunk meshing, lighting and chunk streaming.
//!
//! OWNER: optimization & visuals agent. Public API used by `mc-game` (keep
//! stable, add freely):
//! - [`Renderer::new`] (windowed), [`Renderer::new_offscreen`] (headless, for screenshots/tests)
//! - [`Renderer::resize`], [`Renderer::render`], [`Renderer::capture`]
//! - [`Renderer::prepare_blocking`], [`Renderer::set_render_distance`], [`RenderStats`]
//! - [`ChunkStreamer`]: background world generation, lighting, loading/unloading
//!
//! Module map:
//! - [`tasks`]: the only place that knows about threads (swap for the web)
//! - [`light`]: sky/block light flood fill (full + incremental)
//! - [`mesher`]: section meshing (AO, smooth light, tints, shapes)
//! - `chunks`: mesh bookkeeping, GPU arena, frustum + cave culling
//! - `textures`: block tile array with mipmaps and animation, texture cache
//! - `sky`, `dynamic`: atmosphere parameters and per-frame geometry
//!
//! Frame: sky → sun/moon → opaque → cutout → entities/items → clouds →
//! translucent (back to front) → crack overlay → outlines → UI, all in one
//! render pass with reversed-Z depth and camera-relative coordinates.

pub mod light;
pub mod mesher;
pub mod tasks;

mod arena;
mod chunks;
mod dynamic;
mod sky;
mod streamer;
mod textures;

use std::sync::Arc;

use glam::{Mat4, Vec3};
use mc_assets::Assets;
use mc_core::render_types::{Camera, FrameData};
use mc_core::{Rgba8Image, World};
use winit::window::Window;

pub use chunks::CullSettings;
pub use streamer::{ChunkStreamer, StreamStats};

use chunks::{ChunkMeshes, DrawItem, Frustum};
use dynamic::{
    CloudMap, CloudVertex, DynKind, DynList, DynVertex, LineVertex, UiBatch, UiKind, UiVertex,
};
use textures::{BlockArray, Tex2d, TexCache};

#[derive(Clone, Copy, Debug, Default)]
pub struct RenderStats {
    /// Sections drawn this frame.
    pub chunks_drawn: u32,
    /// Sections meshed since start.
    pub chunks_meshed: u32,
    pub triangles: u64,
    pub draw_calls: u32,
    pub gpu_name_hash: u64,
    /// Sections the visibility pass looked at / drew.
    pub sections_visited: u32,
    /// Meshed sections within the render distance (drawn + culled).
    pub sections_in_range: u32,
    pub mesh_queue: u32,
    pub mesh_in_flight: u32,
    pub mesh_uploaded: u32,
    pub upload_kb: u32,
    /// Average worker time per meshed section (µs).
    pub mesh_us_avg: f32,
    /// Sections meshed per second (last second).
    pub mesh_per_sec: f32,
    pub vertex_mb: f32,
    pub vertex_capacity_mb: f32,
    pub entities: u32,
    /// Wall time between frames (ms).
    pub frame_ms: f32,
    /// CPU time inside `render` (ms) and its parts.
    pub cpu_ms: f32,
    pub update_ms: f32,
    pub cull_ms: f32,
    pub build_ms: f32,
    pub encode_ms: f32,
    pub acquire_ms: f32,
    pub render_distance: i32,
}

impl RenderStats {
    /// Lines for the F3 overlay.
    pub fn debug_lines(&self) -> Vec<String> {
        vec![
            format!(
                "Render: {:.1} ms cpu (mesh {:.1}, cull {:.1}, build {:.1}, encode {:.1}, acquire {:.1})",
                self.cpu_ms,
                self.update_ms,
                self.cull_ms,
                self.build_ms,
                self.encode_ms,
                self.acquire_ms
            ),
            format!(
                "Sections: {} drawn / {} in range, {} draws, {:.1}k tris, rd {}",
                self.chunks_drawn,
                self.sections_in_range,
                self.draw_calls,
                self.triangles as f32 / 1000.0,
                self.render_distance
            ),
            format!(
                "Meshing: {:.0}/s, {:.0} us/section, queue {} running {} uploaded {} ({} KB)",
                self.mesh_per_sec,
                self.mesh_us_avg,
                self.mesh_queue,
                self.mesh_in_flight,
                self.mesh_uploaded,
                self.upload_kb
            ),
            format!(
                "Vertex memory: {:.1} / {:.1} MB",
                self.vertex_mb, self.vertex_capacity_mb
            ),
        ]
    }
}

/// Wall clock (seconds since first call); swap for `performance.now()` on the web.
#[cfg(not(target_arch = "wasm32"))]
pub fn clock() -> f64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
}
#[cfg(target_arch = "wasm32")]
pub fn clock() -> f64 {
    0.0
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

const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Globals {
    view_proj: [[f32; 4]; 4],
    inv_view_proj: [[f32; 4]; 4],
    cam: [f32; 4],
    sun: [f32; 4],
    zenith: [f32; 4],
    horizon: [f32; 4],
    glow: [f32; 4],
    fog: [f32; 4],
    fog_color: [f32; 4],
    sky_light: [f32; 4],
    block_light: [f32; 4],
    screen: [f32; 4],
    cloud: [f32; 4],
    cloud_color: [f32; 4],
}

struct Pipelines {
    sky: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
    opaque: wgpu::RenderPipeline,
    cutout: wgpu::RenderPipeline,
    translucent: wgpu::RenderPipeline,
    dyn_tex: wgpu::RenderPipeline,
    dyn_block: wgpu::RenderPipeline,
    crack: wgpu::RenderPipeline,
    cloud_depth: wgpu::RenderPipeline,
    cloud_color: wgpu::RenderPipeline,
    line: wgpu::RenderPipeline,
    ui_tex: wgpu::RenderPipeline,
    ui_block: wgpu::RenderPipeline,
}

/// A buffer that grows (power of two) when written with more data.
struct GrowBuffer {
    buf: wgpu::Buffer,
    cap: u64,
    usage: wgpu::BufferUsages,
    label: &'static str,
}

impl GrowBuffer {
    fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        cap: u64,
    ) -> Self {
        let usage = usage | wgpu::BufferUsages::COPY_DST;
        GrowBuffer {
            buf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: cap,
                usage,
                mapped_at_creation: false,
            }),
            cap,
            usage,
            label,
        }
    }

    fn write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) {
        if data.is_empty() {
            return;
        }
        if data.len() as u64 > self.cap {
            self.cap = (data.len() as u64).next_power_of_two();
            self.buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.cap,
                usage: self.usage,
                mapped_at_creation: false,
            });
        }
        queue.write_buffer(&self.buf, 0, data);
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    target: Target,
    color_format: wgpu::TextureFormat,
    depth: wgpu::TextureView,
    pub stats: RenderStats,
    pub adapter_info: String,
    assets: Arc<Assets>,
    /// Render distance in chunks (sections farther away are not drawn).
    pub render_distance: i32,
    /// Culling switches (for benchmarking).
    pub cull: CullSettings,
    /// Draw a placeholder box for entity models without geometry.
    pub fallback_entity_boxes: bool,
    pipes: Pipelines,
    globals: wgpu::Buffer,
    group0: wgpu::BindGroup,
    blocks: BlockArray,
    opaque_tiles: Vec<bool>,
    tex: TexCache,
    chunks: ChunkMeshes,
    quad_index: wgpu::Buffer,
    quad_index_quads: u32,
    instances: GrowBuffer,
    dyn_buf: GrowBuffer,
    line_buf: GrowBuffer,
    ui_buf: GrowBuffer,
    cloud_buf: GrowBuffer,
    draw_items: Vec<DrawItem>,
    sky_list: DynList,
    world_list: DynList,
    overlay_list: DynList,
    lines: Vec<LineVertex>,
    ui_verts: Vec<UiVertex>,
    ui_batches: Vec<UiBatch>,
    clouds: CloudMap,
    cloud_verts: Vec<CloudVertex>,
    cloud_key: Option<(i32, i32)>,
    cloud_quads: u32,
    sun_tex: Arc<Tex2d>,
    moon_tex: Arc<Tex2d>,
    last_frame: f64,
    last_tick: u64,
    last_tick_time: f64,
    mesh_rate: (f64, u64, f32),
}

fn request_device(adapter: &wgpu::Adapter, layers: u32) -> (wgpu::Device, wgpu::Queue, u32) {
    let alim = adapter.limits();
    let mut limits = wgpu::Limits::default();
    let max_layers = alim
        .max_texture_array_layers
        .max(limits.max_texture_array_layers);
    limits.max_texture_array_layers = layers
        .clamp(256, max_layers)
        .min(alim.max_texture_array_layers.max(256));
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("mcclone"),
        required_limits: limits.clone(),
        ..Default::default()
    }))
    .expect("request device");
    (device, queue, limits.max_texture_array_layers)
}

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
        let tiles = textures::build_tileset(&assets);
        let (device, queue, max_layers) = request_device(&adapter, tiles.mips.len() as u32);
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .expect("surface config");
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().copied().find(|f| f.is_srgb()) {
            config.format = f;
        }
        let color_format = config.format.add_srgb_suffix();
        if color_format != config.format {
            config.view_formats = vec![color_format];
        }
        if caps.alpha_modes.contains(&wgpu::CompositeAlphaMode::Opaque) {
            config.alpha_mode = wgpu::CompositeAlphaMode::Opaque;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let info = adapter.get_info();
        let target = Target::Window { surface, config };
        Self::init(
            device,
            queue,
            target,
            color_format,
            assets,
            tiles,
            max_layers,
            &info,
        )
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
        let tiles = textures::build_tileset(&assets);
        let (device, queue, max_layers) = request_device(&adapter, tiles.mips.len() as u32);
        let texture = Self::make_offscreen(&device, width, height);
        let info = adapter.get_info();
        let target = Target::Offscreen {
            texture,
            width,
            height,
        };
        Self::init(
            device,
            queue,
            target,
            OFFSCREEN_FORMAT,
            assets,
            tiles,
            max_layers,
            &info,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn init(
        device: wgpu::Device,
        queue: wgpu::Queue,
        target: Target,
        color_format: wgpu::TextureFormat,
        assets: Arc<Assets>,
        tiles: textures::TileSet,
        max_layers: u32,
        info: &wgpu::AdapterInfo,
    ) -> Self {
        let opaque_tiles: Vec<bool> = assets
            .blocks
            .tiles
            .iter()
            .map(|t| mesher::tile_alpha(t).1 || !mesher::tile_alpha(t).0)
            .collect();
        let blocks = BlockArray::new(&device, &queue, tiles, max_layers);
        log::info!(
            "block tiles: {} layers in {} array(s), {} mip levels, {} animated",
            blocks.layer_count,
            blocks.views.len(),
            blocks.levels,
            blocks.anims.len()
        );
        let tex = TexCache::new(&device, &queue);

        let block_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("blocks"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mut entries = vec![
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ];
        for i in 0..blocks.views.len() {
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 + i as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2Array,
                    multisampled: false,
                },
                count: None,
            });
        }
        let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &entries,
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut bg_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&block_sampler),
            },
        ];
        for (i, v) in blocks.views.iter().enumerate() {
            bg_entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: wgpu::BindingResource::TextureView(v),
            });
        }
        let group0 = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &layout0,
            entries: &bg_entries,
        });
        let src = shader_source(blocks.views.len(), blocks.layers_per_array);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("world.wgsl"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let pipes = create_pipelines(&device, &module, &layout0, &tex.layout, color_format);

        let (w, h) = match &target {
            Target::Window { config, .. } => (config.width, config.height),
            Target::Offscreen { width, height, .. } => (*width, *height),
        };
        let depth = make_depth(&device, w, h);
        let tables = Arc::new(mesher::MeshTables::new(&assets));
        let max_buffer = device.limits().max_buffer_size;
        let chunks = ChunkMeshes::new(tables, max_buffer);
        let quad_index_quads = 1 << 16;
        let quad_index = make_quad_index(&device, &queue, quad_index_quads);
        let v = wgpu::BufferUsages::VERTEX;
        let clouds = CloudMap::new(
            assets
                .pack
                .load_image("textures/environment/clouds")
                .as_ref(),
        );
        let mut tex = tex;
        let sun_tex = tex.get(&device, &queue, &assets, "textures/environment/sun");
        let moon_tex = tex.get(&device, &queue, &assets, "textures/environment/moon_phases");
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        std::hash::Hash::hash(&info.name, &mut hasher);
        let stats = RenderStats {
            gpu_name_hash: std::hash::Hasher::finish(&hasher),
            ..Default::default()
        };
        Renderer {
            instances: GrowBuffer::new(&device, "section instances", v, 64 << 10),
            dyn_buf: GrowBuffer::new(&device, "dynamic vertices", v, 256 << 10),
            line_buf: GrowBuffer::new(&device, "lines", v, 16 << 10),
            ui_buf: GrowBuffer::new(&device, "ui vertices", v, 256 << 10),
            cloud_buf: GrowBuffer::new(&device, "clouds", v, 256 << 10),
            device,
            queue,
            target,
            color_format,
            depth,
            stats,
            adapter_info: format!("{} ({:?})", info.name, info.backend),
            assets,
            render_distance: 8,
            cull: CullSettings {
                frustum: true,
                occlusion: true,
            },
            fallback_entity_boxes: true,
            pipes,
            globals,
            group0,
            blocks,
            opaque_tiles,
            tex,
            chunks,
            quad_index,
            quad_index_quads,
            draw_items: Vec::new(),
            sky_list: DynList::default(),
            world_list: DynList::default(),
            overlay_list: DynList::default(),
            lines: Vec::new(),
            ui_verts: Vec::new(),
            ui_batches: Vec::new(),
            clouds,
            cloud_verts: Vec::new(),
            cloud_key: None,
            cloud_quads: 0,
            sun_tex,
            moon_tex,
            last_frame: 0.0,
            last_tick: u64::MAX,
            last_tick_time: 0.0,
            mesh_rate: (0.0, 0, 0.0),
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
        self.depth = make_depth(&self.device, width, height);
    }

    /// Render distance in chunks (also sets the fog distance).
    pub fn set_render_distance(&mut self, rd: i32) {
        self.render_distance = rd.clamp(2, 64);
    }

    /// Mesh every meshable section now, in parallel (startup, screenshots,
    /// benchmarks). Returns (sections meshed, seconds).
    pub fn prepare_blocking(&mut self, world: &World, camera: &Camera) -> (u64, f64) {
        let t0 = clock();
        let before = self.chunks.stats.sections_meshed_total;
        loop {
            self.chunks.update(
                world,
                &self.device,
                &self.queue,
                camera.position,
                None,
                true,
            );
            if self.chunks.idle() {
                break;
            }
            self.chunks.wait_one();
        }
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        (
            self.chunks.stats.sections_meshed_total - before,
            clock() - t0,
        )
    }

    /// Average worker time per meshed section so far (µs).
    pub fn mesh_micros_avg(&self) -> f64 {
        let s = &self.chunks.stats;
        s.mesh_micros_total as f64 / s.sections_meshed_total.max(1) as f64
    }

    /// Wait for the GPU to finish submitted work (benchmarks).
    pub fn wait_gpu(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Draw one frame: world (chunks from `world`), entities, then UI.
    pub fn render(&mut self, world: &World, frame: &FrameData) {
        let t_start = clock();
        if self.last_frame > 0.0 {
            self.stats.frame_ms = ((t_start - self.last_frame) * 1000.0) as f32;
        }
        self.last_frame = t_start;
        let (w, h) = self.size();
        let cam = frame.camera;
        let aspect = w as f32 / h.max(1) as f32;
        let rd = self.render_distance;

        // Dynamic texture uploads.
        for (key, img) in &frame.ui.upload {
            self.tex
                .upload(&self.device, &self.queue, key.as_str(), img);
        }

        // Matrices (camera-relative).
        let view = Mat4::look_to_rh(Vec3::ZERO, cam.forward(), Vec3::Y);
        let proj = Mat4::perspective_infinite_reverse_rh(cam.fov_y, aspect, cam.near.max(0.01));
        let vp = proj * view;
        let far = (rd as f32 * 16.0 * 1.5).max(64.0);
        let frustum = Frustum::from_matrix(
            Mat4::perspective_rh(cam.fov_y, aspect, cam.near.max(0.01), far) * view,
        );

        // Chunk meshes.
        let t0 = clock();
        self.chunks.update(
            world,
            &self.device,
            &self.queue,
            cam.position,
            Some(&frustum),
            false,
        );
        let t1 = clock();
        let mut items = std::mem::take(&mut self.draw_items);
        self.chunks
            .collect_visible(cam.position, &frustum, rd, &self.cull, &mut items);
        self.draw_items = items;
        let t2 = clock();

        // Globals.
        let tick_now = clock();
        if world.tick != self.last_tick {
            self.last_tick = world.tick;
            self.last_tick_time = tick_now;
        }
        let partial = ((tick_now - self.last_tick_time) * 20.0).clamp(0.0, 1.0);
        let anim_secs = ((world.tick % 1_728_000) as f64 + partial) / 20.0;
        let sp = sky::sky_params(&frame.sky, world.tick);
        let fog_end = rd as f32 * 16.0;
        let (fog, fog_color) = if frame.sky.in_lava {
            ([0.0, 2.5, 2.0, 0.0], [0.55, 0.08, 0.0, 1.0])
        } else if frame.sky.underwater {
            let p = cam.position.floor().as_ivec3();
            let wc = sky::rgb_linear(world.biome(p.x, p.z).def().water_color);
            let lvl = (world.light(p) >> 4) as f32;
            let b = (0.8f32.powf(15.0 - (lvl - sp.sky_dim).max(0.0))).max(0.08);
            let c = wc * 0.55 * b;
            ([-6.0, 36.0, 1.0, 0.0], [c.x, c.y, c.z, 1.0])
        } else {
            ([fog_end * 0.7, fog_end, 0.0, 0.0], [0.0; 4])
        };
        let cloud_range = (fog_end * 2.0).clamp(160.0, 384.0);
        let cloud_origin = self.update_clouds(cam.position, anim_secs, cloud_range);
        let g = Globals {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            cam: [
                cam.position.x,
                cam.position.y,
                cam.position.z,
                anim_secs as f32,
            ],
            sun: sp.sun_dir.extend(sp.day).to_array(),
            zenith: sp.zenith.extend(sp.stars).to_array(),
            horizon: sp.horizon.extend(sp.glow_strength).to_array(),
            glow: sp.glow.extend(sp.sky_dim).to_array(),
            fog,
            fog_color,
            sky_light: sp.sky_light.extend(world.tick as f32).to_array(),
            block_light: sp.block_light.extend(0.0).to_array(),
            screen: [w as f32, h as f32, 1.0 / w as f32, 1.0 / h as f32],
            cloud: cloud_origin.extend(0.8).to_array(),
            cloud_color: sp.cloud.extend(cloud_range).to_array(),
        };
        self.queue
            .write_buffer(&self.globals, 0, bytemuck::bytes_of(&g));

        // Per-frame geometry.
        self.build_dynamic(world, frame, &sp);
        let t3 = clock();

        // Surface.
        let (surface_tex, view) = match &self.target {
            Target::Window { surface, config } => match surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t)
                | wgpu::CurrentSurfaceTexture::Suboptimal(t) => {
                    let v = t.texture.create_view(&wgpu::TextureViewDescriptor {
                        format: Some(self.color_format),
                        ..Default::default()
                    });
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
        let t4 = clock();
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        self.blocks.animate(&mut enc, world.tick);
        self.encode(&mut enc, &view, frame);
        self.queue.submit([enc.finish()]);
        if let Some(t) = surface_tex {
            self.queue.present(t);
        }
        let t5 = clock();

        // Stats.
        let cs = self.chunks.stats;
        let s = &mut self.stats;
        s.chunks_meshed = cs.sections_meshed_total as u32;
        s.chunks_drawn = cs.visible;
        s.sections_visited = cs.visited;
        s.mesh_queue = cs.queued + cs.ready;
        s.mesh_in_flight = cs.in_flight;
        s.mesh_uploaded = cs.uploaded_this_frame;
        s.upload_kb = (cs.upload_bytes_this_frame / 1024) as u32;
        s.mesh_us_avg = cs.mesh_micros_total as f32 / cs.sections_meshed_total.max(1) as f32;
        s.vertex_mb = self.chunks.arena.used_bytes() as f32 / (1 << 20) as f32;
        s.vertex_capacity_mb = self.chunks.arena.capacity_bytes() as f32 / (1 << 20) as f32;
        s.entities = frame.entities.len() as u32;
        s.update_ms = ((t1 - t0) * 1000.0) as f32;
        s.cull_ms = ((t2 - t1) * 1000.0) as f32;
        s.build_ms = ((t3 - t2) * 1000.0) as f32;
        s.acquire_ms = ((t4 - t3) * 1000.0) as f32;
        s.encode_ms = ((t5 - t4) * 1000.0) as f32;
        s.cpu_ms = ((t5 - t_start) * 1000.0) as f32;
        s.render_distance = rd;
        let (rt, rn, rate) = &mut self.mesh_rate;
        if t5 - *rt >= 1.0 {
            *rate = (cs.sections_meshed_total - *rn) as f32 / (t5 - *rt) as f32;
            *rt = t5;
            *rn = cs.sections_meshed_total;
        }
        s.mesh_per_sec = *rate;
        if t5 - self.mesh_rate.0 < 0.001 {
            self.stats.sections_in_range = self.chunks.meshed_in_range(cam.position, rd);
        }
    }

    /// Rebuild the cloud mesh when the camera or the drift crosses a cell.
    /// Returns the camera-relative origin of the mesh.
    fn update_clouds(&mut self, cam: Vec3, secs: f64, range: f32) -> Vec3 {
        let drift = (secs * 0.6) % (self.clouds.w as f64 * dynamic::CLOUD_CELL as f64);
        let cell = dynamic::CLOUD_CELL as f64;
        let n = ((range / dynamic::CLOUD_CELL).ceil() as i32) * 2 + 1;
        let ci = ((cam.x as f64 - drift) / cell).floor() as i32 - n / 2;
        let cj = (cam.z as f64 / cell).floor() as i32 - n / 2;
        if self.cloud_key != Some((ci, cj)) {
            self.cloud_key = Some((ci, cj));
            self.clouds.mesh(ci, cj, n, &mut self.cloud_verts);
            self.cloud_quads = (self.cloud_verts.len() / 4) as u32;
            self.cloud_buf.write(
                &self.device,
                &self.queue,
                bytemuck::cast_slice(&self.cloud_verts),
            );
            self.ensure_quad_index(self.cloud_quads);
        }
        Vec3::new(
            (ci as f64 * cell + drift - cam.x as f64) as f32,
            dynamic::CLOUD_HEIGHT - cam.y,
            (cj as f64 * cell - cam.z as f64) as f32,
        )
    }

    fn ensure_quad_index(&mut self, quads: u32) {
        if quads > self.quad_index_quads {
            self.quad_index_quads = quads.next_power_of_two();
            self.quad_index = make_quad_index(&self.device, &self.queue, self.quad_index_quads);
        }
    }

    fn build_dynamic(&mut self, world: &World, frame: &FrameData, sp: &sky::SkyParams) {
        let cam = frame.camera.position;
        let (device, queue, assets) = (&self.device, &self.queue, &self.assets);
        let tables = self.chunks.tables.clone();

        // Sun and moon.
        self.sky_list.clear();
        if !frame.sky.underwater && !frame.sky.in_lava {
            if sp.sun_alpha > 0.0 {
                dynamic::sky_quad(
                    &mut self.sky_list,
                    &DynKind::Sprite(self.sun_tex.clone()),
                    sp.sun_dir,
                    30.0,
                    [0.0, 0.0, 1.0, 1.0],
                    [1.0, 1.0, 1.0, sp.sun_alpha],
                );
            }
            if sp.moon_alpha > 0.0 {
                let p = sp.moon_phase;
                let (u, v) = ((p % 4) as f32 * 0.25, (p / 4) as f32 * 0.5);
                dynamic::sky_quad(
                    &mut self.sky_list,
                    &DynKind::Sprite(self.moon_tex.clone()),
                    -sp.sun_dir,
                    20.0,
                    [u, v, u + 0.25, v + 0.5],
                    [1.0, 1.0, 1.0, sp.moon_alpha * 0.9],
                );
            }
        }

        // Entities, items, falling blocks, particles.
        self.world_list.clear();
        for e in &frame.entities {
            let mut mesh = assets
                .models
                .get(&e.model)
                .map(|m| m.mesh(&e.poses))
                .unwrap_or_default();
            if mesh.is_empty() {
                if !self.fallback_entity_boxes {
                    continue;
                }
                mesh = dynamic::fallback_box();
            }
            let t = self.tex.get(device, queue, assets, e.texture.as_str());
            dynamic::entity_mesh(
                &mut self.world_list,
                &DynKind::Tex(t),
                &mesh,
                e.transform,
                cam,
                e.tint,
                e.light,
                e.hurt,
            );
        }
        for it in &frame.items {
            match it.item.block() {
                Some(b) => dynamic::block_model(
                    &mut self.world_list,
                    &tables,
                    &self.opaque_tiles,
                    b,
                    it.transform,
                    cam,
                    it.light,
                    &DynKind::Block,
                ),
                None => {
                    let t = self
                        .tex
                        .item_icon(device, queue, assets, it.item)
                        .unwrap_or_else(|| self.tex.missing.clone());
                    dynamic::icon_quad(
                        &mut self.world_list,
                        &DynKind::Tex(t),
                        it.transform,
                        cam,
                        it.light,
                    );
                }
            }
        }
        for b in &frame.block_models {
            dynamic::block_model(
                &mut self.world_list,
                &tables,
                &self.opaque_tiles,
                b.block,
                b.transform,
                cam,
                b.light,
                &DynKind::Block,
            );
        }
        let right = Vec3::new(frame.camera.yaw.cos(), 0.0, frame.camera.yaw.sin());
        let up = right.cross(frame.camera.forward()).normalize_or_zero();
        for s in &frame.sprites {
            let t = self.tex.get(device, queue, assets, s.texture.as_str());
            let c = s.center - cam;
            let (hx, hy) = (right * s.size.x * 0.5, up * s.size.y * 0.5);
            let corners = [c - hx - hy, c + hx - hy, c + hx + hy, c - hx + hy];
            let uvs = [
                [s.uv[0], s.uv[3]],
                [s.uv[2], s.uv[3]],
                [s.uv[2], s.uv[1]],
                [s.uv[0], s.uv[1]],
            ];
            let col = s.color.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8);
            let mut v = [DynVertex::default(); 4];
            for k in 0..4 {
                v[k] = DynVertex {
                    pos: corners[k].to_array(),
                    uv: uvs[k],
                    color: col,
                    light: dynamic::light_bytes(s.light, 1.0, 0.0),
                    layer: 0,
                };
            }
            self.world_list.quad(&DynKind::Tex(t), v);
        }

        // Crack overlay.
        self.overlay_list.clear();
        if let Some((pos, progress)) = frame.breaking {
            if !self.blocks.destroy.is_empty() && !world.block(pos).is_air() {
                let n = self.blocks.destroy.len();
                let stage = ((progress.clamp(0.0, 0.999) * n as f32) as usize).min(n - 1);
                dynamic::crack(&mut self.overlay_list, pos, self.blocks.destroy[stage], cam);
            }
        }

        // Outlines.
        self.lines.clear();
        for b in &frame.boxes {
            dynamic::box_lines(&mut self.lines, b, cam);
        }

        // UI.
        let (w, h) = self.size();
        let tex = &mut self.tex;
        dynamic::build_ui(
            &frame.ui,
            |k| tex.get(device, queue, assets, k),
            frame.ui.dim_world.then_some((w as f32, h as f32)),
            &mut self.ui_verts,
            &mut self.ui_batches,
        );

        // Upload.
        let mut all: Vec<DynVertex> = Vec::with_capacity(
            self.sky_list.verts.len() + self.world_list.verts.len() + self.overlay_list.verts.len(),
        );
        all.extend_from_slice(&self.sky_list.verts);
        all.extend_from_slice(&self.world_list.verts);
        all.extend_from_slice(&self.overlay_list.verts);
        self.dyn_buf
            .write(device, queue, bytemuck::cast_slice(&all));
        self.line_buf
            .write(device, queue, bytemuck::cast_slice(&self.lines));
        self.ui_buf
            .write(device, queue, bytemuck::cast_slice(&self.ui_verts));
        let inst: Vec<[f32; 4]> = self
            .draw_items
            .iter()
            .map(|d| d.origin.to_array())
            .collect();
        self.instances
            .write(device, queue, bytemuck::cast_slice(&inst));
        let max_quads = self
            .draw_items
            .iter()
            .flat_map(|d| d.layers.iter().flatten())
            .map(|l| l.2)
            .max()
            .unwrap_or(0);
        self.ensure_quad_index(max_quads);
    }

    fn encode(
        &mut self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        frame: &FrameData,
    ) {
        let mut draws = 0u32;
        let mut tris = 0u64;
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("world"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.group0, &[]);

        // Sky.
        pass.set_pipeline(&self.pipes.sky);
        pass.draw(0..3, 0..1);
        draws += 1;

        let sky_n = self.sky_list.verts.len() as u32;
        let world_n = self.world_list.verts.len() as u32;
        let dyn_bases = [0, sky_n, sky_n + world_n];
        if !self.sky_list.batches.is_empty() {
            pass.set_vertex_buffer(0, self.dyn_buf.buf.slice(..));
            pass.set_pipeline(&self.pipes.sprite);
            for b in &self.sky_list.batches {
                if let DynKind::Sprite(t) = &b.kind {
                    pass.set_bind_group(1, &t.bind_group, &[]);
                    pass.draw(
                        dyn_bases[0] + b.start..dyn_bases[0] + b.start + b.count,
                        0..1,
                    );
                    draws += 1;
                }
            }
        }

        // Chunks: opaque and cutout front to back.
        pass.set_index_buffer(self.quad_index.slice(..), wgpu::IndexFormat::Uint32);
        pass.set_vertex_buffer(1, self.instances.buf.slice(..));
        for (layer, pipe) in [(0usize, &self.pipes.opaque), (1, &self.pipes.cutout)] {
            pass.set_pipeline(pipe);
            let mut page = u32::MAX;
            for (i, d) in self.draw_items.iter().enumerate() {
                let Some((pg, start, quads)) = d.layers[layer] else {
                    continue;
                };
                if pg != page {
                    page = pg;
                    pass.set_vertex_buffer(
                        0,
                        self.chunks.arena.pages[pg as usize].buffer.slice(..),
                    );
                }
                pass.draw_indexed(0..quads * 6, (start * 4) as i32, i as u32..i as u32 + 1);
                draws += 1;
                tris += quads as u64 * 2;
            }
        }

        // Entities, items, falling blocks.
        if !self.world_list.batches.is_empty() {
            pass.set_vertex_buffer(0, self.dyn_buf.buf.slice(..));
            for b in &self.world_list.batches {
                let r = dyn_bases[1] + b.start..dyn_bases[1] + b.start + b.count;
                match &b.kind {
                    DynKind::Tex(t) => {
                        pass.set_pipeline(&self.pipes.dyn_tex);
                        pass.set_bind_group(1, &t.bind_group, &[]);
                    }
                    _ => pass.set_pipeline(&self.pipes.dyn_block),
                }
                tris += b.count as u64 / 3;
                pass.draw(r, 0..1);
                draws += 1;
            }
        }

        // Clouds: depth first so overlapping faces don't double-blend.
        if self.cloud_quads > 0 && !frame.sky.underwater && !frame.sky.in_lava {
            pass.set_vertex_buffer(0, self.cloud_buf.buf.slice(..));
            for p in [&self.pipes.cloud_depth, &self.pipes.cloud_color] {
                pass.set_pipeline(p);
                pass.draw_indexed(0..self.cloud_quads * 6, 0, 0..1);
                draws += 1;
            }
            tris += self.cloud_quads as u64 * 4;
        }

        // Translucent chunks back to front.
        pass.set_pipeline(&self.pipes.translucent);
        pass.set_vertex_buffer(1, self.instances.buf.slice(..));
        let mut page = u32::MAX;
        for (i, d) in self.draw_items.iter().enumerate().rev() {
            let Some((pg, start, quads)) = d.layers[2] else {
                continue;
            };
            if pg != page {
                page = pg;
                pass.set_vertex_buffer(0, self.chunks.arena.pages[pg as usize].buffer.slice(..));
            }
            pass.draw_indexed(0..quads * 6, (start * 4) as i32, i as u32..i as u32 + 1);
            draws += 1;
            tris += quads as u64 * 2;
        }

        // Crack overlay.
        if !self.overlay_list.batches.is_empty() {
            pass.set_vertex_buffer(0, self.dyn_buf.buf.slice(..));
            pass.set_pipeline(&self.pipes.crack);
            for b in &self.overlay_list.batches {
                pass.draw(
                    dyn_bases[2] + b.start..dyn_bases[2] + b.start + b.count,
                    0..1,
                );
                draws += 1;
            }
        }

        // Outlines.
        if !self.lines.is_empty() {
            pass.set_pipeline(&self.pipes.line);
            pass.set_vertex_buffer(0, self.line_buf.buf.slice(..));
            pass.draw(0..self.lines.len() as u32, 0..1);
            draws += 1;
        }

        // UI.
        if !self.ui_batches.is_empty() {
            pass.set_vertex_buffer(0, self.ui_buf.buf.slice(..));
            for b in &self.ui_batches {
                match &b.kind {
                    UiKind::Tex(t) => {
                        pass.set_pipeline(&self.pipes.ui_tex);
                        pass.set_bind_group(1, &t.bind_group, &[]);
                    }
                    UiKind::Blocks => pass.set_pipeline(&self.pipes.ui_block),
                }
                pass.draw(b.start..b.start + b.count, 0..1);
                draws += 1;
            }
        }
        drop(pass);
        self.stats.draw_calls = draws;
        self.stats.triangles = tris;
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

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w.max(1),
                height: h.max(1),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn make_quad_index(device: &wgpu::Device, queue: &wgpu::Queue, quads: u32) -> wgpu::Buffer {
    let mut idx: Vec<u32> = Vec::with_capacity(quads as usize * 6);
    for q in 0..quads {
        let b = q * 4;
        idx.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
    }
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quad indices"),
        size: (idx.len() * 4) as u64,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, bytemuck::cast_slice(&idx));
    buf
}

const WORLD_WGSL: &str = include_str!("shaders/world.wgsl");

/// Shader source with the block-array bindings for `arrays` texture arrays.
fn shader_source(arrays: usize, per_array: u32) -> String {
    let mut decl = String::new();
    for i in 0..arrays {
        decl += &format!(
            "@group(0) @binding({}) var blocks{i}: texture_2d_array<f32>;\n",
            2 + i
        );
    }
    let sample = if arrays <= 1 {
        "fn sample_block(uv: vec2<f32>, layer: u32) -> vec4<f32> {\n    return textureSample(blocks0, samp_blocks, uv, layer);\n}\n".to_string()
    } else {
        // Gradients are taken first (uniform control flow), then the right
        // array is sampled with explicit gradients.
        let mut s = String::from(
            "fn sample_block(uv: vec2<f32>, layer: u32) -> vec4<f32> {\n    let dx = dpdx(uv);\n    let dy = dpdy(uv);\n",
        );
        s += &format!("    let a = layer / {per_array}u;\n    let l = layer % {per_array}u;\n");
        for i in 0..arrays {
            s += &format!(
                "    if (a == {i}u) {{ return textureSampleGrad(blocks{i}, samp_blocks, uv, l, dx, dy); }}\n"
            );
        }
        s += "    return vec4<f32>(1.0, 0.0, 1.0, 1.0);\n}\n";
        s
    };
    WORLD_WGSL
        .replace("//#BLOCK_ARRAYS#", &decl)
        .replace("//#SAMPLE_BLOCK#", &sample)
}

struct PipeDesc<'a> {
    label: &'a str,
    layout: &'a wgpu::PipelineLayout,
    vs: &'a str,
    fs: &'a str,
    buffers: &'a [Option<wgpu::VertexBufferLayout<'a>>],
    topology: wgpu::PrimitiveTopology,
    cull: Option<wgpu::Face>,
    depth_write: bool,
    depth_compare: wgpu::CompareFunction,
    blend: Option<wgpu::BlendState>,
    write_mask: wgpu::ColorWrites,
}

fn create_pipelines(
    device: &wgpu::Device,
    module: &wgpu::ShaderModule,
    layout0: &wgpu::BindGroupLayout,
    tex_layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> Pipelines {
    let l0 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("globals only"),
        bind_group_layouts: &[Some(layout0)],
        immediate_size: 0,
    });
    let l01 = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("globals + texture"),
        bind_group_layouts: &[Some(layout0), Some(tex_layout)],
        immediate_size: 0,
    });
    let make = |d: PipeDesc| {
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some(d.label),
            layout: Some(d.layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some(d.vs),
                compilation_options: Default::default(),
                buffers: d.buffers,
            },
            primitive: wgpu::PrimitiveState {
                topology: d.topology,
                cull_mode: d.cull,
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(d.depth_write),
                depth_compare: Some(d.depth_compare),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some(d.fs),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: d.blend,
                    write_mask: d.write_mask,
                })],
            }),
            multiview_mask: None,
            cache: None,
        })
    };
    use wgpu::{CompareFunction as C, PrimitiveTopology as T, VertexFormat as F};
    let chunk_attrs = [
        wgpu::VertexAttribute {
            format: F::Uint32,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: F::Uint32,
            offset: 4,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: F::Uint32,
            offset: 8,
            shader_location: 2,
        },
    ];
    let inst_attrs = [wgpu::VertexAttribute {
        format: F::Float32x4,
        offset: 0,
        shader_location: 3,
    }];
    let chunk_buffers = [
        Some(wgpu::VertexBufferLayout {
            array_stride: 12,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &chunk_attrs,
        }),
        Some(wgpu::VertexBufferLayout {
            array_stride: 16,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &inst_attrs,
        }),
    ];
    let dyn_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Unorm8x4, 4 => Uint32];
    let dyn_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<DynVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &dyn_attrs,
    })];
    let cloud_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32];
    let cloud_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<CloudVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &cloud_attrs,
    })];
    let line_attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4];
    let line_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<LineVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &line_attrs,
    })];
    let ui_attrs =
        wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Uint32];
    let ui_buffers = [Some(wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<UiVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ui_attrs,
    })];
    let alpha = Some(wgpu::BlendState::ALPHA_BLENDING);
    let additive = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent::OVER,
    });
    let multiply = Some(wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    });
    let all = wgpu::ColorWrites::ALL;
    let base = |label, layout, vs, fs, buffers| PipeDesc {
        label,
        layout,
        vs,
        fs,
        buffers,
        topology: T::TriangleList,
        cull: None,
        depth_write: false,
        depth_compare: C::GreaterEqual,
        blend: None,
        write_mask: all,
    };
    Pipelines {
        sky: make(PipeDesc {
            depth_compare: C::Always,
            ..base("sky", &l0, "vs_sky", "fs_sky", &[])
        }),
        sprite: make(PipeDesc {
            depth_compare: C::Always,
            blend: additive,
            ..base("sun/moon", &l01, "vs_sprite", "fs_sprite", &dyn_buffers)
        }),
        opaque: make(PipeDesc {
            cull: Some(wgpu::Face::Back),
            depth_write: true,
            ..base(
                "chunks opaque",
                &l0,
                "vs_chunk",
                "fs_chunk_opaque",
                &chunk_buffers,
            )
        }),
        cutout: make(PipeDesc {
            cull: Some(wgpu::Face::Back),
            depth_write: true,
            ..base(
                "chunks cutout",
                &l0,
                "vs_chunk",
                "fs_chunk_cutout",
                &chunk_buffers,
            )
        }),
        translucent: make(PipeDesc {
            blend: alpha,
            ..base(
                "chunks translucent",
                &l0,
                "vs_chunk",
                "fs_chunk_translucent",
                &chunk_buffers,
            )
        }),
        dyn_tex: make(PipeDesc {
            depth_write: true,
            ..base("entities", &l01, "vs_dyn", "fs_dyn_tex", &dyn_buffers)
        }),
        dyn_block: make(PipeDesc {
            cull: Some(wgpu::Face::Back),
            depth_write: true,
            ..base("block models", &l0, "vs_dyn", "fs_dyn_block", &dyn_buffers)
        }),
        crack: make(PipeDesc {
            cull: Some(wgpu::Face::Back),
            blend: multiply,
            ..base("crack", &l0, "vs_dyn", "fs_crack", &dyn_buffers)
        }),
        cloud_depth: make(PipeDesc {
            depth_write: true,
            write_mask: wgpu::ColorWrites::empty(),
            ..base("clouds depth", &l0, "vs_cloud", "fs_cloud", &cloud_buffers)
        }),
        cloud_color: make(PipeDesc {
            depth_compare: C::Equal,
            blend: alpha,
            ..base("clouds", &l0, "vs_cloud", "fs_cloud", &cloud_buffers)
        }),
        line: make(PipeDesc {
            topology: T::LineList,
            blend: alpha,
            ..base("lines", &l0, "vs_line", "fs_line", &line_buffers)
        }),
        ui_tex: make(PipeDesc {
            depth_compare: C::Always,
            blend: alpha,
            ..base("ui", &l01, "vs_ui", "fs_ui_tex", &ui_buffers)
        }),
        ui_block: make(PipeDesc {
            depth_compare: C::Always,
            blend: alpha,
            ..base("ui blocks", &l0, "vs_ui", "fs_ui_block", &ui_buffers)
        }),
    }
}
