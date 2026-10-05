//! Dev tool: dump what `mc-assets` loads from the pack into PNG contact sheets.
//!
//! ```text
//! cargo run -p mc-assets --example dump -- <out_dir> [entity ids...]
//! ```
//!
//! Writes `tiles.png` (every block tile), `blocks.png` (per block: up, down,
//! north, south, east, west as the renderer would tint them, plus the
//! isometric item icon), `items.png` (the item icon atlas) and
//! `entities.png` + `entity_<name>.png` (front / right side / three-quarter
//! orthographic views of posed mob models, drawn by a tiny CPU rasteriser
//! with their pack textures).

use std::path::{Path, PathBuf};

use glam::{Mat4, Vec2, Vec3};
use mc_assets::image_ops::{alpha_over, scale_integer, tint};
use mc_assets::{Assets, EntityModel, ModelVertex, Pack};
use mc_core::render_types::BonePose;
use mc_core::{BlockId, Face, Rgba8Image};

fn save(img: &Rgba8Image, path: &Path) {
    image::save_buffer(
        path,
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgba8,
    )
    .unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
    println!("wrote {}", path.display());
}

/// Light/dark grey checkerboard (shows transparency).
fn checker(w: u32, h: u32, cell: u32) -> Rgba8Image {
    let mut img = Rgba8Image::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let on = ((x / cell) + (y / cell)) % 2 == 0;
            let c = if on { 200 } else { 160 };
            img.put(x, y, [c, c, c, 255]);
        }
    }
    img
}

fn over_at(dst: &mut Rgba8Image, src: &Rgba8Image, dx: u32, dy: u32) {
    let mut region = dst.crop(dx, dy, src.width, src.height);
    alpha_over(&mut region, src);
    dst.blit(&region, dx, dy);
}

fn tiles_sheet(a: &Assets) -> Rgba8Image {
    let (scale, cols, gap) = (3u32, 24u32, 2u32);
    let cell = 16 * scale + gap;
    let n = a.blocks.tiles.len() as u32;
    let rows = n.div_ceil(cols);
    let mut img = checker(cols * cell + gap, rows * cell + gap, 6);
    for (i, t) in a.blocks.tiles.iter().enumerate() {
        let (x, y) = (
            (i as u32 % cols) * cell + gap,
            (i as u32 / cols) * cell + gap,
        );
        over_at(&mut img, &scale_integer(t, scale), x, y);
    }
    img
}

fn blocks_sheet(a: &Assets) -> Rgba8Image {
    let scale = 3u32;
    let face = 16 * scale;
    let gap = 4u32;
    let row_h = face + gap;
    let cell_w = 6 * (face + 2) + gap + face + 12;
    let per_col = 34u32;
    let n = BlockId::count() as u32 - 1;
    let cols = n.div_ceil(per_col);
    let mut img = checker(cols * cell_w + gap, per_col * row_h + gap, 6);
    for (k, b) in BlockId::all().skip(1).enumerate() {
        let (cx, cy) = (
            (k as u32 / per_col) * cell_w + gap,
            (k as u32 % per_col) * row_h + gap,
        );
        let t = a.colormaps.default_tint(b);
        for (j, f) in Face::ALL.iter().enumerate() {
            let mut tile = a.blocks.tile(a.blocks.face(b, *f)).clone();
            if a.blocks.is_tinted(b, *f) {
                tint(&mut tile, t);
            }
            if let Some(ov) = a.blocks.overlay(b, *f) {
                let mut o = a.blocks.tile(ov).clone();
                tint(&mut o, t);
                alpha_over(&mut tile, &o);
            }
            over_at(
                &mut img,
                &scale_integer(&tile, scale),
                cx + j as u32 * (face + 2),
                cy,
            );
        }
        let item = mc_core::ItemId::from_block(b);
        if let Some(icon) = a.items.display_icon(item) {
            let icon = mc_assets::resize_nearest(icon, face, face);
            over_at(&mut img, &icon, cx + 6 * (face + 2) + 8, cy);
        }
    }
    img
}

fn items_sheet(a: &Assets) -> Rgba8Image {
    let atlas = &a.items.atlas;
    let mut img = Rgba8Image::filled(atlas.width + 8, atlas.height + 8, [139, 139, 139, 255]);
    let cell = a.items.atlas_cell;
    for y in (0..atlas.height).step_by(cell as usize) {
        for x in (0..atlas.width).step_by(cell as usize) {
            let slot = Rgba8Image::filled(cell - 2, cell - 2, [198, 198, 198, 255]);
            img.blit(&slot, x + 5, y + 5);
        }
    }
    over_at(&mut img, atlas, 4, 4);
    scale_integer(&img, 2)
}

// ---------------------------------------------------------------------------
// Tiny CPU rasteriser.

struct Canvas {
    img: Rgba8Image,
    depth: Vec<f32>,
}

impl Canvas {
    fn new(w: u32, h: u32, bg: [u8; 4]) -> Self {
        Canvas {
            img: Rgba8Image::filled(w, h, bg),
            depth: vec![f32::NEG_INFINITY; (w * h) as usize],
        }
    }
}

/// Draw `mesh` seen through `view` (model → camera, camera looks down -Z,
/// orthographic). `scale` = pixels per block, `center` = screen position of
/// the camera-space origin.
fn draw_mesh(
    c: &mut Canvas,
    mesh: &[ModelVertex],
    tex: &Rgba8Image,
    view: Mat4,
    scale: f32,
    center: Vec2,
) {
    let light = Vec3::new(0.35, 1.0, -0.6).normalize();
    for tri in mesh.chunks_exact(3) {
        let p: Vec<Vec3> = tri.iter().map(|v| view.transform_point3(v.pos)).collect();
        // No back-face culling: like the game, mob cutouts are double sided
        // (e.g. chicken legs are painted on the inside of a see-through box).
        let shade = 0.55 + 0.45 * tri[0].normal.dot(light).max(0.0);
        let s: Vec<Vec2> = p
            .iter()
            .map(|q| Vec2::new(center.x + q.x * scale, center.y - q.y * scale))
            .collect();
        let min = s[0].min(s[1]).min(s[2]).floor().max(Vec2::ZERO);
        let max = s[0].max(s[1]).max(s[2]).ceil().min(Vec2::new(
            c.img.width as f32 - 1.0,
            c.img.height as f32 - 1.0,
        ));
        let area = (s[1] - s[0]).perp_dot(s[2] - s[0]);
        if area.abs() < 1e-8 {
            continue;
        }
        for y in min.y as u32..=max.y.max(0.0) as u32 {
            for x in min.x as u32..=max.x.max(0.0) as u32 {
                let q = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w0 = (s[2] - s[1]).perp_dot(q - s[1]) / area;
                let w1 = (s[0] - s[2]).perp_dot(q - s[2]) / area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * p[0].z + w1 * p[1].z + w2 * p[2].z;
                let i = (y * c.img.width + x) as usize;
                if z <= c.depth[i] {
                    continue;
                }
                let uv = tri[0].uv * w0 + tri[1].uv * w1 + tri[2].uv * w2;
                let tx = ((uv.x * tex.width as f32) as i32).clamp(0, tex.width as i32 - 1) as u32;
                let ty = ((uv.y * tex.height as f32) as i32).clamp(0, tex.height as i32 - 1) as u32;
                let t = tex.get(tx, ty);
                if t[3] < 128 {
                    continue;
                }
                c.depth[i] = z;
                c.img.put(
                    x,
                    y,
                    [
                        (t[0] as f32 * shade) as u8,
                        (t[1] as f32 * shade) as u8,
                        (t[2] as f32 * shade) as u8,
                        255,
                    ],
                );
            }
        }
    }
}

/// Front (camera in front of the face, looking +Z), right side (camera on
/// the model's right, +X), and a three-quarter view from front-right-above.
fn views() -> [(&'static str, Mat4); 3] {
    [
        (
            "front",
            Mat4::look_to_rh(Vec3::new(0.0, 0.0, -10.0), Vec3::Z, Vec3::Y),
        ),
        (
            "side",
            Mat4::look_to_rh(Vec3::new(10.0, 0.0, 0.0), Vec3::NEG_X, Vec3::Y),
        ),
        (
            "3/4",
            Mat4::look_to_rh(
                Vec3::new(7.0, 6.0, -8.0),
                -Vec3::new(7.0, 6.0, -8.0).normalize(),
                Vec3::Y,
            ),
        ),
    ]
}

fn render_entity(
    model: &EntityModel,
    tex: &Rgba8Image,
    poses: &[BonePose],
    cell: u32,
) -> Rgba8Image {
    let mesh = model.mesh(poses);
    let vs = views();
    let mut out = Rgba8Image::new(cell * vs.len() as u32, cell);
    for (k, (_, view)) in vs.iter().enumerate() {
        // Fit the model in the cell.
        let pts: Vec<Vec3> = mesh.iter().map(|v| view.transform_point3(v.pos)).collect();
        let lo = pts.iter().fold(Vec3::splat(f32::MAX), |a, p| a.min(*p));
        let hi = pts.iter().fold(Vec3::splat(f32::MIN), |a, p| a.max(*p));
        let size = (hi - lo).truncate().max(Vec2::splat(0.1));
        let scale = (cell as f32 * 0.86) / size.x.max(size.y);
        let mid = (lo + hi).truncate() * 0.5;
        let center = Vec2::new(
            cell as f32 * 0.5 - mid.x * scale,
            cell as f32 * 0.5 + mid.y * scale,
        );
        let shade = if k % 2 == 0 { 225 } else { 210 };
        let mut c = Canvas::new(cell, cell, [shade, shade, 235, 255]);
        // Ground line at model y = 0.
        let gy = view.transform_point3(Vec3::ZERO).y;
        let ground = (center.y - gy * scale).round();
        if k < 2 && ground >= 0.0 && ground < cell as f32 {
            for x in 0..cell {
                c.img.put(x, ground as u32, [90, 120, 90, 255]);
            }
        }
        draw_mesh(&mut c, &mesh, tex, *view, scale, center);
        out.blit(&c.img, k as u32 * cell, 0);
    }
    out
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let mut args = std::env::args().skip(1);
    let out_dir = PathBuf::from(args.next().unwrap_or_else(|| "asset_dump".into()));
    let mut ids: Vec<String> = args.collect();
    if ids.is_empty() {
        ids = [
            "pig", "cow", "sheep", "chicken", "zombie", "skeleton", "creeper", "spider",
        ]
        .map(String::from)
        .to_vec();
    }
    std::fs::create_dir_all(&out_dir).expect("create output dir");
    let pack = Pack::find().expect("resource pack not found (set MC_PACK_DIR)");
    let t0 = std::time::Instant::now();
    let a = Assets::load(pack);
    println!(
        "Assets::load took {:.0} ms; {:?}",
        t0.elapsed().as_secs_f64() * 1000.0,
        a.report
    );

    save(&tiles_sheet(&a), &out_dir.join("tiles.png"));
    save(&blocks_sheet(&a), &out_dir.join("blocks.png"));
    save(&items_sheet(&a), &out_dir.join("items.png"));

    let cell = 220;
    let mut rows = Vec::new();
    for id in &ids {
        let Some(ce) = a.models.client_entity(id) else {
            eprintln!("no client entity '{id}'");
            continue;
        };
        let Some((model, tex_path)) = a.models.entity_model(id) else {
            eprintln!("no model for '{id}'");
            continue;
        };
        let tex = a
            .pack
            .load_texture(tex_path)
            .unwrap_or_else(|| mc_assets::magenta_checker(64));
        let poses = a.models.rest_pose_for(ce, model);
        println!(
            "{id}: {} ({} cubes, {} tris) texture {tex_path} {}x{}, rest pose {:?}",
            model.identifier,
            model.cube_count(),
            model.vertex_count() / 3,
            tex.width,
            tex.height,
            poses
                .iter()
                .map(|p| (&*p.bone, p.rotation))
                .collect::<Vec<_>>()
        );
        let img = render_entity(model, &tex, &poses, cell);
        save(
            &scale_integer(&img, 2),
            &out_dir.join(format!("entity_{}.png", id.replace(':', "_"))),
        );
        rows.push(img);
    }
    if !rows.is_empty() {
        let per_row = 2usize;
        let w = rows[0].width;
        let mut sheet = Rgba8Image::new(
            w * per_row as u32,
            cell * rows.len().div_ceil(per_row) as u32,
        );
        for (i, r) in rows.iter().enumerate() {
            sheet.blit(r, (i % per_row) as u32 * w, (i / per_row) as u32 * cell);
        }
        save(&sheet, &out_dir.join("entities.png"));
    }
}
