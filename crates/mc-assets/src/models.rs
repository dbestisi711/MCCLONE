//! Entity models parsed from the pack's Bedrock `.geo.json` files, posed
//! into triangle meshes, plus the client entity definitions (`entity/*.json`)
//! that say which geometry and texture each mob uses.
//!
//! Supported geometry formats:
//! - modern: `"minecraft:geometry": [{ "description": {identifier,
//!   texture_width, texture_height, ...}, "bones": [...] }]`
//! - legacy (`format_version` 1.8 / 1.10): `"geometry.name": {texturewidth,
//!   textureheight, bones}` including inheritance keys
//!   `"geometry.child:geometry.parent"` (child bones with the same name are
//!   merged into the parent's: fields override, cubes are added — an
//!   identical cube replaces the inherited one — and `"reset": true` drops
//!   the inherited cubes).
//!
//! Bones: name, parent, pivot, rotation, bind_pose_rotation (rotates only
//! the bone's own cubes), mirror, inflate, neverRender, locators, cubes
//! (origin, size, box UV `[u, v]` or per-face `{uv, uv_size, uv_rotation}`,
//! rotation + pivot, inflate, mirror). Poly meshes and texture meshes are
//! ignored.
//!
//! Conventions of the output (see also the crate docs): blocks, +Y up, feet
//! at y = 0, model faces -Z, its right side at +X (geo files have X
//! mirrored; the parser converts), CCW triangles seen from outside, UVs
//! normalised by the geometry's texture size.

use std::sync::Arc;

use glam::{Mat3, Mat4, Vec2, Vec3};
use mc_core::Face;
use mc_core::render_types::BonePose;
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;

use crate::Pack;
use crate::animations::{Animation, condition_holds, load_animations, load_controllers};

/// Vertex produced by posing a model, in model space (units: blocks).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ModelVertex {
    pub pos: Vec3,
    pub normal: Vec3,
    /// Normalised texture coordinates.
    pub uv: Vec2,
}

/// UV rectangle of one cube face, in texture pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceUv {
    /// `(u1, v1)` maps to the face's top-left corner as seen from outside
    /// the cube, `(u2, v2)` to its bottom-right (u2 < u1 = mirrored).
    /// For the top face "top" is the -Z edge; for the bottom face the +Z edge.
    pub rect: [f32; 4],
    /// Quarter turns of the mapping (`uv_rotation / 90`).
    pub rotation: u8,
}

/// One box of a bone, in model space pixels (X already converted).
#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    /// Minimum corner before inflation.
    pub from: Vec3,
    pub size: Vec3,
    /// Grows the box on every side (pixels); UVs are unaffected.
    pub inflate: f32,
    /// Rotation pivot (defaults to the box centre).
    pub pivot: Vec3,
    /// Rotation in degrees, geo-file convention (see crate docs).
    pub rotation: Vec3,
    /// Per face (`Face::index()` order); `None` = face not drawn.
    pub faces: [Option<FaceUv>; 6],
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bone {
    pub name: Arc<str>,
    /// Index into `EntityModel::bones` (always smaller than this bone's index).
    pub parent: Option<usize>,
    /// Pivot in model space pixels.
    pub pivot: Vec3,
    /// Rest rotation in degrees (geo convention).
    pub rotation: Vec3,
    /// Rotation applied to this bone's own cubes only (legacy models).
    pub bind_pose_rotation: Vec3,
    /// Cubes are not drawn (the bone still transforms its children).
    pub never_render: bool,
    pub cubes: Vec<Cube>,
    /// Named points (lead, item attachment...), model space pixels.
    pub locators: Vec<(Arc<str>, Vec3)>,
}

#[derive(Clone, Debug, Default)]
pub struct EntityModel {
    pub identifier: Arc<str>,
    pub texture_width: u32,
    pub texture_height: u32,
    /// Bones ordered so that parents come before children.
    pub bones: Vec<Bone>,
    /// `visible_bounds_width`, `visible_bounds_height` (blocks).
    pub visible_bounds: Vec2,
    /// `visible_bounds_offset` (blocks).
    pub visible_bounds_offset: Vec3,
    /// File the geometry came from.
    pub source: Arc<str>,
}

/// Rotation matrix (model space) for geo-convention Euler angles in degrees:
/// X first, then Y, then Z, with the X axis mirrored relative to the files.
pub fn geo_rotation(deg: Vec3) -> Mat3 {
    Mat3::from_rotation_z(deg.z.to_radians())
        * Mat3::from_rotation_y(-deg.y.to_radians())
        * Mat3::from_rotation_x(-deg.x.to_radians())
}

fn about(pivot: Vec3, m: Mat3) -> Mat4 {
    Mat4::from_translation(pivot) * Mat4::from_mat3(m) * Mat4::from_translation(-pivot)
}

/// Corners (top-left, top-right, bottom-right, bottom-left as seen from
/// outside) of a box face.
fn face_corners(f: Face, a: Vec3, b: Vec3) -> [Vec3; 4] {
    let v = Vec3::new;
    match f {
        Face::North => [
            v(b.x, b.y, a.z),
            v(a.x, b.y, a.z),
            v(a.x, a.y, a.z),
            v(b.x, a.y, a.z),
        ],
        Face::South => [
            v(a.x, b.y, b.z),
            v(b.x, b.y, b.z),
            v(b.x, a.y, b.z),
            v(a.x, a.y, b.z),
        ],
        Face::East => [
            v(b.x, b.y, b.z),
            v(b.x, b.y, a.z),
            v(b.x, a.y, a.z),
            v(b.x, a.y, b.z),
        ],
        Face::West => [
            v(a.x, b.y, a.z),
            v(a.x, b.y, b.z),
            v(a.x, a.y, b.z),
            v(a.x, a.y, a.z),
        ],
        Face::Up => [
            v(a.x, b.y, a.z),
            v(b.x, b.y, a.z),
            v(b.x, b.y, b.z),
            v(a.x, b.y, b.z),
        ],
        Face::Down => [
            v(a.x, a.y, b.z),
            v(b.x, a.y, b.z),
            v(b.x, a.y, a.z),
            v(a.x, a.y, a.z),
        ],
    }
}

impl EntityModel {
    /// Bone index by name (case-insensitive, like the pack).
    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones
            .iter()
            .position(|b| b.name.eq_ignore_ascii_case(name))
    }

    /// Per-bone transforms in model-space **pixels** with poses applied.
    /// Second value: bone hidden (pose scale 0 on it or an ancestor).
    fn bone_transforms(&self, poses: &[BonePose]) -> (Vec<Mat4>, Vec<bool>) {
        let mut world = Vec::with_capacity(self.bones.len());
        let mut hidden = Vec::with_capacity(self.bones.len());
        for bone in &self.bones {
            let pose = poses
                .iter()
                .find(|p| p.bone.eq_ignore_ascii_case(&bone.name));
            let (rot, off, scale) = pose.map(|p| (p.rotation, p.offset, p.scale)).unwrap_or((
                Vec3::ZERO,
                Vec3::ZERO,
                1.0,
            ));
            let parent_hidden = bone.parent.is_some_and(|p| hidden[p]);
            hidden.push(parent_hidden || scale == 0.0);
            // Offsets are in geo axes: mirror X into model space.
            let off = Vec3::new(-off.x, off.y, off.z);
            let local = Mat4::from_translation(bone.pivot + off)
                * Mat4::from_mat3(geo_rotation(bone.rotation + rot))
                * Mat4::from_scale(Vec3::splat(if scale == 0.0 { 1.0 } else { scale }))
                * Mat4::from_translation(-bone.pivot);
            let m = match bone.parent {
                Some(p) => world[p] * local,
                None => local,
            };
            world.push(m);
        }
        (world, hidden)
    }

    /// Bone transforms in **block** units (same index as `bones`): maps a
    /// point of the rest-pose model (blocks) to its posed position. Useful
    /// for attaching held items to `rightItem` etc.
    pub fn bone_matrices(&self, poses: &[BonePose]) -> Vec<Mat4> {
        let to_px = Mat4::from_scale(Vec3::splat(16.0));
        let to_blocks = Mat4::from_scale(Vec3::splat(1.0 / 16.0));
        self.bone_transforms(poses)
            .0
            .into_iter()
            .map(|m| to_blocks * m * to_px)
            .collect()
    }

    /// Posed position (blocks) of a named locator, e.g. `"lead"`.
    pub fn locator(&self, name: &str, poses: &[BonePose]) -> Option<Vec3> {
        let (world, _) = self.bone_transforms(poses);
        for (i, b) in self.bones.iter().enumerate() {
            if let Some((_, p)) = b.locators.iter().find(|(n, _)| &**n == name) {
                return Some(world[i].transform_point3(*p) / 16.0);
            }
        }
        None
    }

    /// Triangles (3 vertices each) for the model with the given bone poses applied.
    pub fn mesh(&self, poses: &[BonePose]) -> Vec<ModelVertex> {
        let mut out = Vec::with_capacity(self.vertex_count());
        self.mesh_into(poses, &mut out);
        out
    }

    /// Like [`EntityModel::mesh`] but appends to an existing buffer.
    pub fn mesh_into(&self, poses: &[BonePose], out: &mut Vec<ModelVertex>) {
        let (world, hidden) = self.bone_transforms(poses);
        let tex = Vec2::new(
            self.texture_width.max(1) as f32,
            self.texture_height.max(1) as f32,
        );
        let to_blocks = Mat4::from_scale(Vec3::splat(1.0 / 16.0));
        for (i, bone) in self.bones.iter().enumerate() {
            if bone.never_render || hidden[i] || bone.cubes.is_empty() {
                continue;
            }
            let mut bone_m = to_blocks * world[i];
            if bone.bind_pose_rotation != Vec3::ZERO {
                bone_m = bone_m * about(bone.pivot, geo_rotation(bone.bind_pose_rotation));
            }
            for cube in &bone.cubes {
                let m = if cube.rotation != Vec3::ZERO {
                    bone_m * about(cube.pivot, geo_rotation(cube.rotation))
                } else {
                    bone_m
                };
                let nm = Mat3::from_mat4(m);
                let lo = cube.from.min(cube.from + cube.size) - Vec3::splat(cube.inflate);
                let hi = cube.from.max(cube.from + cube.size) + Vec3::splat(cube.inflate);
                for f in Face::ALL {
                    let Some(fuv) = cube.faces[f.index()] else {
                        continue;
                    };
                    let c = face_corners(f, lo, hi);
                    if c[0].distance_squared(c[1]) < 1e-10 || c[0].distance_squared(c[3]) < 1e-10 {
                        continue;
                    }
                    let [u1, v1, u2, v2] = fuv.rect;
                    let mut uv = [
                        Vec2::new(u1, v1),
                        Vec2::new(u2, v1),
                        Vec2::new(u2, v2),
                        Vec2::new(u1, v2),
                    ];
                    uv.rotate_left((fuv.rotation % 4) as usize);
                    let n = (nm * f.normal().as_vec3()).normalize_or_zero();
                    let vert = |k: usize| ModelVertex {
                        pos: m.transform_point3(c[k]),
                        normal: n,
                        uv: uv[k] / tex,
                    };
                    out.extend_from_slice(&[vert(0), vert(3), vert(2), vert(0), vert(2), vert(1)]);
                }
            }
        }
    }

    /// Number of vertices `mesh` returns in the rest pose.
    pub fn vertex_count(&self) -> usize {
        let mut n = 0;
        for bone in &self.bones {
            if bone.never_render {
                continue;
            }
            for cube in &bone.cubes {
                let lo = cube.from.min(cube.from + cube.size) - Vec3::splat(cube.inflate);
                let hi = cube.from.max(cube.from + cube.size) + Vec3::splat(cube.inflate);
                for f in Face::ALL {
                    if cube.faces[f.index()].is_none() {
                        continue;
                    }
                    let c = face_corners(f, lo, hi);
                    if c[0].distance_squared(c[1]) >= 1e-10 && c[0].distance_squared(c[3]) >= 1e-10
                    {
                        n += 6;
                    }
                }
            }
        }
        n
    }

    pub fn cube_count(&self) -> usize {
        self.bones.iter().map(|b| b.cubes.len()).sum()
    }

    /// Axis-aligned bounds (blocks) of the posed mesh.
    pub fn bounds(&self, poses: &[BonePose]) -> Option<(Vec3, Vec3)> {
        let mesh = self.mesh(poses);
        let first = mesh.first()?.pos;
        Some(
            mesh.iter()
                .fold((first, first), |(lo, hi), v| (lo.min(v.pos), hi.max(v.pos))),
        )
    }
}

/// A `minecraft:client_entity` definition (`entity/*.json`).
#[derive(Clone, Debug, Default)]
pub struct ClientEntity {
    /// e.g. `"minecraft:pig"`.
    pub identifier: Arc<str>,
    pub min_engine_version: (u32, u32, u32),
    /// Texture name → pack path without extension (`"default"` → `"textures/entity/pig/pig_v3"`).
    pub textures: Vec<(Arc<str>, Arc<str>)>,
    /// Geometry name → geometry identifier (`"default"` → `"geometry.pig.v3"`).
    pub geometry: Vec<(Arc<str>, Arc<str>)>,
    pub materials: Vec<(Arc<str>, Arc<str>)>,
    /// Animation name → animation / controller id.
    pub animations: Vec<(Arc<str>, Arc<str>)>,
    pub render_controllers: Vec<Arc<str>>,
    /// `scripts.animate` entries: (animation name, Molang condition if any).
    pub animate: Vec<(Arc<str>, Option<Arc<str>>)>,
    /// Legacy `animation_controllers` list: (name, controller id).
    pub animation_controllers: Vec<(Arc<str>, Arc<str>)>,
    /// Spawn egg texture key, if any.
    pub spawn_egg: Option<Arc<str>>,
    /// File it was read from.
    pub source: Arc<str>,
}

fn lookup<'a>(list: &'a [(Arc<str>, Arc<str>)], name: &str) -> Option<&'a str> {
    list.iter().find(|(k, _)| &**k == name).map(|(_, v)| &**v)
}

/// `"default"` if present, else the most "plain" entry: names mentioning
/// babies or add-on layers (armor, decor, saddle...) rank last.
fn default_entry(list: &[(Arc<str>, Arc<str>)]) -> Option<&str> {
    if let Some(v) = lookup(list, "default") {
        return Some(v);
    }
    let score = |k: &str| {
        let k = k.to_ascii_lowercase();
        let mut s = 0;
        if k.contains("baby") {
            s += 4;
        }
        for w in [
            "armor",
            "decor",
            "saddle",
            "overlay",
            "collar",
            "chest",
            "harness",
            "charged",
            "invisible",
            "_v1",
        ] {
            if k.contains(w) {
                s += 2;
            }
        }
        s
    };
    list.iter()
        .min_by(|a, b| score(&a.0).cmp(&score(&b.0)).then(a.0.cmp(&b.0)))
        .map(|(_, v)| &**v)
}

impl ClientEntity {
    pub fn texture(&self, name: &str) -> Option<&str> {
        lookup(&self.textures, name)
    }
    pub fn geometry(&self, name: &str) -> Option<&str> {
        lookup(&self.geometry, name)
    }
    pub fn material(&self, name: &str) -> Option<&str> {
        lookup(&self.materials, name)
    }
    pub fn animation(&self, name: &str) -> Option<&str> {
        lookup(&self.animations, name)
    }
    /// `"default"` texture, else the plainest one (not baby/armor/decor...).
    pub fn default_texture(&self) -> Option<&str> {
        default_entry(&self.textures)
    }
    /// `"default"` geometry, else the plainest one.
    pub fn default_geometry(&self) -> Option<&str> {
        default_entry(&self.geometry)
    }
}

/// All parsed geometries and client entity definitions.
#[derive(Clone, Debug, Default)]
pub struct EntityModels {
    pub by_id: HashMap<Arc<str>, Arc<EntityModel>>,
    /// Client entities by identifier (`"minecraft:pig"`).
    pub entities: HashMap<Arc<str>, Arc<ClientEntity>>,
    /// Animations from `animations/*.json` by id (static poses only).
    pub animations: HashMap<Arc<str>, Arc<Animation>>,
    /// Animation controller id → animation names its initial state always plays.
    pub controllers: HashMap<Arc<str>, Vec<Arc<str>>>,
    /// Files or geometries that could not be parsed/resolved.
    pub failures: Vec<String>,
}

impl EntityModels {
    /// Parse `models/mobs.json`, `models/entity/*.json`, `models/blocks/*.json`
    /// and `entity/*.json`.
    pub fn load(pack: &Pack) -> Self {
        let mut out = EntityModels::default();
        let mut files = vec!["models/mobs.json".to_string()];
        files.extend(pack.list_files("models/entity", ".json"));
        files.extend(pack.list_files("models/blocks", ".json"));
        let mut raws: HashMap<String, RawGeo> = HashMap::default();
        for f in &files {
            let Some(v) = pack.load_json(f) else {
                out.failures.push(format!("{f}: unreadable geometry file"));
                continue;
            };
            for g in parse_geometry_file(&v, f) {
                match raws.get(&g.id) {
                    Some(old) if old.format >= g.format => {}
                    _ => {
                        raws.insert(g.id.clone(), g);
                    }
                }
            }
        }
        let mut cache = HashMap::default();
        let mut ids: Vec<&String> = raws.keys().collect();
        ids.sort();
        for id in ids {
            match resolve(id, &raws, &mut cache, 0) {
                Some(raw) => {
                    let m = finalize(&raw);
                    out.by_id.insert(m.identifier.clone(), Arc::new(m));
                }
                None => out.failures.push(format!(
                    "{}: geometry '{id}' has an unresolvable parent '{}'",
                    raws[id].source,
                    raws[id].parent.as_deref().unwrap_or("?")
                )),
            }
        }

        out.animations = load_animations(pack);
        out.controllers = load_controllers(pack);

        for f in pack.list_files("entity", ".json") {
            let Some(v) = pack.load_json(&f) else {
                out.failures
                    .push(format!("{f}: unreadable client entity file"));
                continue;
            };
            let Some(ce) = parse_client_entity(&v, &f) else {
                continue;
            };
            match out.entities.get(&ce.identifier) {
                Some(old) if old.min_engine_version > ce.min_engine_version => {}
                Some(old)
                    if old.min_engine_version == ce.min_engine_version
                        && old.source.len() <= ce.source.len() => {}
                _ => {
                    out.entities.insert(ce.identifier.clone(), Arc::new(ce));
                }
            }
        }
        out
    }

    pub fn get(&self, id: &str) -> Option<&Arc<EntityModel>> {
        self.by_id.get(id)
    }

    /// Client entity by identifier; `"pig"` is shorthand for `"minecraft:pig"`.
    pub fn client_entity(&self, id: &str) -> Option<&Arc<ClientEntity>> {
        self.entities.get(id).or_else(|| {
            if id.contains(':') {
                None
            } else {
                self.entities.get(format!("minecraft:{id}").as_str())
            }
        })
    }

    /// The constant "setup" pose of an entity's default model: every
    /// unconditional `scripts.animate` animation evaluated where it doesn't
    /// depend on queries/time (e.g. the spider's leg spread). Combine with
    /// gameplay poses by adding rotations/offsets.
    pub fn rest_pose(&self, id: &str) -> Vec<BonePose> {
        let Some(ce) = self.client_entity(id) else {
            return Vec::new();
        };
        let Some(model) = ce.default_geometry().and_then(|g| self.get(g)) else {
            return Vec::new();
        };
        self.rest_pose_for(ce, model)
    }

    /// [`EntityModels::rest_pose`] for a specific model of the entity.
    pub fn rest_pose_for(&self, ce: &ClientEntity, model: &EntityModel) -> Vec<BonePose> {
        let mut poses: Vec<BonePose> = Vec::new();
        // Unconditional `scripts.animate` entries plus the initial state of
        // every animation controller (legacy `animation_controllers` list or
        // controllers named in `animate`).
        let mut ids: Vec<Arc<str>> = Vec::new();
        let mut stack: Vec<Arc<str>> = ce
            .animate
            .iter()
            .filter(|(_, cond)| cond.as_deref().is_none_or(condition_holds))
            .filter_map(|(k, _)| ce.animation(k).map(Arc::from))
            .collect();
        stack.extend(ce.animation_controllers.iter().map(|(_, c)| c.clone()));
        let mut guard = 0;
        while let Some(id) = stack.pop() {
            guard += 1;
            if guard > 256 {
                break;
            }
            if let Some(names) = self.controllers.get(&id) {
                for n in names {
                    if let Some(a) = ce.animation(n) {
                        stack.push(Arc::from(a));
                    }
                }
            } else if !ids.contains(&id) {
                ids.push(id);
            }
        }
        for id in ids.iter().rev() {
            let Some(anim) = self.animations.get(id) else {
                continue;
            };
            for p in anim.static_pose(model) {
                match poses.iter_mut().find(|q| q.bone == p.bone) {
                    Some(q) => {
                        q.rotation += p.rotation;
                        q.offset += p.offset;
                        q.scale *= p.scale;
                    }
                    None => poses.push(p),
                }
            }
        }
        poses
    }

    /// Default geometry and texture path of an entity (`"minecraft:pig"`).
    pub fn entity_model(&self, id: &str) -> Option<(&Arc<EntityModel>, &str)> {
        let ce = self.client_entity(id)?;
        let model = self.get(ce.default_geometry()?)?;
        Some((model, ce.default_texture()?))
    }
}

// ---------------------------------------------------------------------------
// Raw (file-convention) parsing and inheritance.

#[derive(Clone, Debug, Default)]
struct RawGeo {
    id: String,
    parent: Option<String>,
    texture_width: Option<f32>,
    texture_height: Option<f32>,
    bounds_w: Option<f32>,
    bounds_h: Option<f32>,
    bounds_offset: Option<[f32; 3]>,
    bones: Vec<RawBone>,
    format: (u32, u32, u32),
    source: Arc<str>,
}

#[derive(Clone, Debug, Default)]
struct RawBone {
    name: String,
    parent: Option<String>,
    pivot: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    bind_pose_rotation: Option<[f32; 3]>,
    mirror: Option<bool>,
    inflate: Option<f32>,
    never_render: Option<bool>,
    reset: bool,
    cubes: Option<Vec<RawCube>>,
    locators: Vec<(String, [f32; 3])>,
}

#[derive(Clone, Debug, PartialEq)]
struct RawCube {
    origin: [f32; 3],
    size: [f32; 3],
    uv: RawUv,
    inflate: Option<f32>,
    rotation: Option<[f32; 3]>,
    pivot: Option<[f32; 3]>,
    mirror: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
enum RawUv {
    Box([f32; 2]),
    /// Per face (`Face::index()` order): rect in model-space orientation + quarter turns.
    Faces([Option<FaceUv>; 6]),
}

fn num(v: &Value) -> Option<f32> {
    v.as_f64().map(|f| f as f32)
}

fn vec3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    if a.len() < 3 {
        return None;
    }
    Some([num(&a[0])?, num(&a[1])?, num(&a[2])?])
}

fn vec2(v: &Value) -> Option<[f32; 2]> {
    let a = v.as_array()?;
    if a.len() < 2 {
        return None;
    }
    Some([num(&a[0])?, num(&a[1])?])
}

fn parse_version(v: &Value) -> (u32, u32, u32) {
    let s = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return (0, 0, 0),
    };
    let mut it = s.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    (
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
        it.next().unwrap_or(0),
    )
}

fn parse_geometry_file(v: &Value, source: &str) -> Vec<RawGeo> {
    let format = parse_version(&v["format_version"]);
    let source: Arc<str> = Arc::from(source);
    let mut out = Vec::new();
    if let Some(list) = v["minecraft:geometry"].as_array() {
        for g in list {
            let d = &g["description"];
            let Some(ident) = d["identifier"].as_str() else {
                continue;
            };
            let (id, parent) = split_id(ident);
            out.push(RawGeo {
                id,
                parent,
                texture_width: num(&d["texture_width"]),
                texture_height: num(&d["texture_height"]),
                bounds_w: num(&d["visible_bounds_width"]),
                bounds_h: num(&d["visible_bounds_height"]),
                bounds_offset: vec3(&d["visible_bounds_offset"]),
                bones: parse_bones(&g["bones"]),
                format,
                source: source.clone(),
            });
        }
    }
    if let Some(obj) = v.as_object() {
        for (k, g) in obj {
            if !k.starts_with("geometry.") || !g.is_object() {
                continue;
            }
            let (id, parent) = split_id(k);
            out.push(RawGeo {
                id,
                parent,
                texture_width: num(&g["texturewidth"]).or_else(|| num(&g["texture_width"])),
                texture_height: num(&g["textureheight"]).or_else(|| num(&g["texture_height"])),
                bounds_w: num(&g["visible_bounds_width"]),
                bounds_h: num(&g["visible_bounds_height"]),
                bounds_offset: vec3(&g["visible_bounds_offset"]),
                bones: parse_bones(&g["bones"]),
                format,
                source: source.clone(),
            });
        }
    }
    out
}

/// Split `"geometry.child:geometry.parent"`. A colon that is a namespace
/// (`"minecraft:geometry.x"`) is part of the identifier.
fn split_id(s: &str) -> (String, Option<String>) {
    for (i, _) in s.match_indices(':') {
        let (a, b) = (&s[..i], &s[i + 1..]);
        if a.contains("geometry") && b.contains("geometry") {
            return (a.trim().to_string(), Some(b.trim().to_string()));
        }
    }
    (s.trim().to_string(), None)
}

fn parse_bones(v: &Value) -> Vec<RawBone> {
    let Some(list) = v.as_array() else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|b| {
            let name = b["name"].as_str()?.to_string();
            let mut locators = Vec::new();
            if let Some(m) = b["locators"].as_object() {
                for (k, l) in m {
                    if let Some(p) = vec3(l).or_else(|| vec3(&l["offset"])) {
                        locators.push((k.clone(), p));
                    }
                }
            }
            Some(RawBone {
                name,
                parent: b["parent"].as_str().map(str::to_string),
                pivot: vec3(&b["pivot"]),
                rotation: vec3(&b["rotation"]),
                bind_pose_rotation: vec3(&b["bind_pose_rotation"]),
                mirror: b["mirror"].as_bool(),
                inflate: num(&b["inflate"]),
                never_render: b["neverRender"].as_bool(),
                reset: b["reset"].as_bool().unwrap_or(false),
                cubes: b["cubes"]
                    .as_array()
                    .map(|cs| cs.iter().filter_map(parse_cube).collect()),
                locators,
            })
        })
        .collect()
}

fn parse_cube(c: &Value) -> Option<RawCube> {
    let origin = vec3(&c["origin"]).unwrap_or([0.0; 3]);
    let size = vec3(&c["size"])?;
    let uv = match &c["uv"] {
        Value::Object(m) => {
            let mut faces = [None; 6];
            for f in Face::ALL {
                let Some(fv) = m.get(f.pack_key()) else {
                    continue;
                };
                let [u, v] = vec2(&fv["uv"]).unwrap_or([0.0, 0.0]);
                let [w, h] = vec2(&fv["uv_size"]).unwrap_or([0.0, 0.0]);
                // Per-face UVs on the top/bottom faces are anchored at the
                // opposite corner (same layout as box UV).
                let rect = match f {
                    Face::Up | Face::Down => [u + w, v + h, u, v],
                    _ => [u, v, u + w, v + h],
                };
                let rot = num(&fv["uv_rotation"]).unwrap_or(0.0);
                let rotation = ((rot / 90.0).round() as i32).rem_euclid(4) as u8;
                faces[f.index()] = Some(FaceUv { rect, rotation });
            }
            RawUv::Faces(faces)
        }
        other => RawUv::Box(vec2(other).unwrap_or([0.0, 0.0])),
    };
    Some(RawCube {
        origin,
        size,
        uv,
        inflate: num(&c["inflate"]),
        rotation: vec3(&c["rotation"]),
        pivot: vec3(&c["pivot"]),
        mirror: c["mirror"].as_bool(),
    })
}

/// Resolve legacy inheritance (`child:parent`) into a flat geometry.
fn resolve(
    id: &str,
    raws: &HashMap<String, RawGeo>,
    cache: &mut HashMap<String, Option<RawGeo>>,
    depth: u32,
) -> Option<RawGeo> {
    if let Some(r) = cache.get(id) {
        return r.clone();
    }
    let raw = raws.get(id)?;
    let result = match &raw.parent {
        None => Some(raw.clone()),
        Some(_) if depth > 16 => None,
        Some(p) => resolve(p, raws, cache, depth + 1).map(|parent| merge(parent, raw)),
    };
    cache.insert(id.to_string(), result.clone());
    result
}

fn merge(parent: RawGeo, child: &RawGeo) -> RawGeo {
    let mut g = parent;
    g.id = child.id.clone();
    g.parent = None;
    g.source = child.source.clone();
    g.format = child.format;
    g.texture_width = child.texture_width.or(g.texture_width);
    g.texture_height = child.texture_height.or(g.texture_height);
    g.bounds_w = child.bounds_w.or(g.bounds_w);
    g.bounds_h = child.bounds_h.or(g.bounds_h);
    g.bounds_offset = child.bounds_offset.or(g.bounds_offset);
    for cb in &child.bones {
        let Some(pb) = g.bones.iter_mut().find(|b| b.name == cb.name) else {
            g.bones.push(cb.clone());
            continue;
        };
        if cb.reset {
            pb.cubes = None;
        }
        pb.parent = cb.parent.clone().or(pb.parent.take());
        pb.pivot = cb.pivot.or(pb.pivot);
        pb.rotation = cb.rotation.or(pb.rotation);
        pb.bind_pose_rotation = cb.bind_pose_rotation.or(pb.bind_pose_rotation);
        pb.mirror = cb.mirror.or(pb.mirror);
        pb.inflate = cb.inflate.or(pb.inflate);
        pb.never_render = cb.never_render.or(pb.never_render);
        for l in &cb.locators {
            pb.locators.retain(|(n, _)| *n != l.0);
            pb.locators.push(l.clone());
        }
        if let Some(cubes) = &cb.cubes {
            let list = pb.cubes.get_or_insert_with(Vec::new);
            for c in cubes {
                match list
                    .iter_mut()
                    .find(|o| o.origin == c.origin && o.size == c.size && o.inflate == c.inflate)
                {
                    Some(o) => *o = c.clone(),
                    None => list.push(c.clone()),
                }
            }
        }
    }
    g
}

fn mirror_x(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[0], p[1], p[2])
}

/// Box UV layout for a cube of `size` at texture offset (u, v), in
/// model-space face orientation.
fn box_uv(u: f32, v: f32, size: [f32; 3], mirror: bool) -> [Option<FaceUv>; 6] {
    // Box UV uses whole-pixel sizes.
    let (w, h, d) = (size[0].floor(), size[1].floor(), size[2].floor());
    let mut r = [[0.0f32; 4]; 6];
    r[Face::East.index()] = [u, v + d, u + d, v + d + h];
    r[Face::North.index()] = [u + d, v + d, u + d + w, v + d + h];
    r[Face::West.index()] = [u + d + w, v + d, u + 2.0 * d + w, v + d + h];
    r[Face::South.index()] = [u + 2.0 * d + w, v + d, u + 2.0 * d + 2.0 * w, v + d + h];
    r[Face::Up.index()] = [u + d + w, v + d, u + d, v];
    r[Face::Down.index()] = [u + d + 2.0 * w, v, u + d + w, v + d];
    if mirror {
        r.swap(Face::East.index(), Face::West.index());
        for rect in &mut r {
            rect.swap(0, 2);
        }
    }
    r.map(|rect| Some(FaceUv { rect, rotation: 0 }))
}

fn finalize(raw: &RawGeo) -> EntityModel {
    // Order bones so parents precede children.
    let n = raw.bones.len();
    let index_of: HashMap<&str, usize> = raw
        .bones
        .iter()
        .enumerate()
        .rev()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();
    let parent_of = |i: usize| -> Option<usize> {
        raw.bones[i]
            .parent
            .as_deref()
            .and_then(|p| index_of.get(p).copied())
            .filter(|&p| p != i)
    };
    let mut order = Vec::with_capacity(n);
    let mut state = vec![0u8; n]; // 0 = new, 1 = visiting, 2 = done
    fn visit(
        i: usize,
        parent_of: &dyn Fn(usize) -> Option<usize>,
        state: &mut [u8],
        order: &mut Vec<usize>,
    ) {
        if state[i] != 0 {
            return;
        }
        state[i] = 1;
        if let Some(p) = parent_of(i) {
            if state[p] == 0 {
                visit(p, parent_of, state, order);
            }
        }
        state[i] = 2;
        order.push(i);
    }
    for i in 0..n {
        visit(i, &parent_of, &mut state, &mut order);
    }
    let mut new_index = vec![usize::MAX; n];
    for (k, &i) in order.iter().enumerate() {
        new_index[i] = k;
    }

    let bones = order
        .iter()
        .map(|&i| {
            let b = &raw.bones[i];
            // Parent must already be placed; a cycle leaves it as a root.
            let parent = parent_of(i)
                .map(|p| new_index[p])
                .filter(|&p| p < new_index[i]);
            let mirror = b.mirror.unwrap_or(false);
            let cubes = b
                .cubes
                .as_deref()
                .unwrap_or(&[])
                .iter()
                .map(|c| {
                    let size = Vec3::from(c.size);
                    let from = Vec3::new(-(c.origin[0] + c.size[0]), c.origin[1], c.origin[2]);
                    Cube {
                        from,
                        size,
                        inflate: c.inflate.or(b.inflate).unwrap_or(0.0),
                        pivot: c.pivot.map(mirror_x).unwrap_or(from + size * 0.5),
                        rotation: c.rotation.map(Vec3::from).unwrap_or(Vec3::ZERO),
                        faces: match &c.uv {
                            RawUv::Box([u, v]) => {
                                box_uv(*u, *v, c.size, c.mirror.unwrap_or(mirror))
                            }
                            RawUv::Faces(f) => *f,
                        },
                    }
                })
                .collect();
            Bone {
                name: Arc::from(b.name.as_str()),
                parent,
                pivot: b.pivot.map(mirror_x).unwrap_or(Vec3::ZERO),
                rotation: b.rotation.map(Vec3::from).unwrap_or(Vec3::ZERO),
                bind_pose_rotation: b.bind_pose_rotation.map(Vec3::from).unwrap_or(Vec3::ZERO),
                never_render: b.never_render.unwrap_or(false),
                cubes,
                locators: b
                    .locators
                    .iter()
                    .map(|(k, p)| (Arc::from(k.as_str()), mirror_x(*p)))
                    .collect(),
            }
        })
        .collect();

    EntityModel {
        identifier: Arc::from(raw.id.as_str()),
        texture_width: raw.texture_width.unwrap_or(64.0).round().max(1.0) as u32,
        texture_height: raw.texture_height.unwrap_or(64.0).round().max(1.0) as u32,
        bones,
        visible_bounds: Vec2::new(raw.bounds_w.unwrap_or(1.0), raw.bounds_h.unwrap_or(1.0)),
        visible_bounds_offset: raw.bounds_offset.map(Vec3::from).unwrap_or(Vec3::ZERO),
        source: raw.source.clone(),
    }
}

fn string_pairs(v: &Value) -> Vec<(Arc<str>, Arc<str>)> {
    let mut out = Vec::new();
    if let Some(m) = v.as_object() {
        for (k, val) in m {
            if let Some(s) = val.as_str() {
                out.push((Arc::from(k.as_str()), Arc::from(s)));
            }
        }
    }
    out
}

fn parse_client_entity(v: &Value, source: &str) -> Option<ClientEntity> {
    let d = &v["minecraft:client_entity"]["description"];
    let identifier = d["identifier"].as_str()?;
    let render_controllers = d["render_controllers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| match r {
                    Value::String(s) => Some(Arc::from(s.as_str())),
                    Value::Object(m) => m.keys().next().map(|k| Arc::from(k.as_str())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ClientEntity {
        identifier: Arc::from(identifier),
        min_engine_version: parse_version(&d["min_engine_version"]),
        textures: string_pairs(&d["textures"]),
        geometry: string_pairs(&d["geometry"]),
        materials: string_pairs(&d["materials"]),
        animations: string_pairs(&d["animations"]),
        render_controllers,
        animation_controllers: d["animation_controllers"]
            .as_array()
            .map(|a| a.iter().flat_map(string_pairs).collect())
            .unwrap_or_default(),
        animate: d["scripts"]["animate"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|e| match e {
                        Value::String(s) => Some((Arc::from(s.as_str()), None)),
                        Value::Object(m) => m.iter().next().map(|(k, c)| {
                            (
                                Arc::from(k.as_str()),
                                Some(Arc::from(c.as_str().unwrap_or("?"))),
                            )
                        }),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        spawn_egg: d["spawn_egg"]["texture"].as_str().map(Arc::from),
        source: Arc::from(source),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> EntityModels {
        EntityModels::load(&Pack::find().expect("pack"))
    }

    fn check_uvs(m: &EntityModel) {
        for v in m.mesh(&[]) {
            assert!(
                (-1e-4..=1.0001).contains(&v.uv.x) && (-1e-4..=1.0001).contains(&v.uv.y),
                "{} uv out of range: {:?}",
                m.identifier,
                v.uv
            );
            assert!((v.normal.length() - 1.0).abs() < 1e-3);
        }
    }

    #[test]
    fn parses_every_geometry_file() {
        let ms = models();
        assert!(ms.failures.is_empty(), "{:?}", ms.failures);
        assert!(ms.by_id.len() > 200, "only {} models", ms.by_id.len());
        for id in [
            "geometry.pig.v3",
            "geometry.cow.v2",
            "geometry.sheep.v1.8",
            "geometry.chicken.v1.12",
            "geometry.zombie.v1.8",
            "geometry.skeleton.v1.8",
            "geometry.creeper.v1.8",
            "geometry.spider.v1.8",
            "geometry.humanoid.custom",
        ] {
            let m = ms.get(id).unwrap_or_else(|| panic!("{id} missing"));
            assert!(m.cube_count() > 0, "{id}");
            check_uvs(m);
        }
    }

    #[test]
    fn pig_mesh_layout() {
        let ms = models();
        let pig = ms.get("geometry.pig.v3").unwrap();
        assert_eq!((pig.texture_width, pig.texture_height), (64, 64));
        assert_eq!(pig.cube_count(), 8);
        let mesh = pig.mesh(&[]);
        assert_eq!(mesh.len(), 8 * 6 * 6);
        assert_eq!(mesh.len(), pig.vertex_count());
        let (lo, hi) = pig.bounds(&[]).unwrap();
        // Feet on the ground, about 1 block long, snout toward -Z.
        assert!(lo.y.abs() < 1e-4, "{lo}");
        assert!(hi.y < 1.2 && hi.y > 0.8, "{hi}");
        assert!(lo.z < -0.9 && hi.z < 0.6, "{lo} {hi}");
        // The body cube is rotated 90° into a horizontal box.
        let body = &pig.bones[pig.bone_index("body").unwrap()];
        assert_eq!(body.cubes[1].rotation, Vec3::new(90.0, 0.0, 0.0));
        // Right legs (leg0/leg2) end up on +X in model space.
        let leg0 = &pig.bones[pig.bone_index("leg0").unwrap()];
        assert!(leg0.pivot.x > 0.0);
        // Triangles wind counter-clockwise seen from outside.
        for t in mesh.chunks_exact(3) {
            let n = (t[1].pos - t[0].pos).cross(t[2].pos - t[0].pos);
            assert!(n.dot(t[0].normal) > 0.0);
        }
    }

    #[test]
    fn legacy_inheritance_merges_bones() {
        let ms = models();
        // Wool sheep = sheared sheep (skin) + wool cubes.
        let sheared = ms.get("geometry.sheep.sheared.v1.8").unwrap();
        let wool = ms.get("geometry.sheep.v1.8").unwrap();
        assert_eq!(sheared.cube_count(), 6);
        assert_eq!(wool.cube_count(), 12);
        // Helmet keeps only the head cubes.
        let helmet = ms.get("geometry.humanoid.armor.helmet").unwrap();
        for b in &helmet.bones {
            if b.cubes.is_empty() {
                continue;
            }
            assert!(&*b.name == "head" || &*b.name == "hat", "{}", b.name);
        }
        // Custom humanoid replaces (not duplicates) the inherited body cube.
        let custom = ms.get("geometry.humanoid.custom").unwrap();
        let body = &custom.bones[custom.bone_index("body").unwrap()];
        assert_eq!(body.cubes.len(), 1);
    }

    #[test]
    fn poses_move_and_hide_bones() {
        let ms = models();
        let zombie = ms.get("geometry.zombie.v1.8").unwrap();
        let rest = zombie.mesh(&[]);
        let hidden = zombie.mesh(&[BonePose::hidden("head")]);
        assert_eq!(rest.len() - hidden.len(), 36);
        // Arms straight forward (x = -90) point toward -Z.
        let arm = zombie.bone_index("rightArm").unwrap();
        let mats = zombie.bone_matrices(&[BonePose::rot("rightArm", Vec3::new(-90.0, 0.0, 0.0))]);
        let hand = mats[arm].transform_point3(Vec3::new(6.0, 12.0, 0.0) / 16.0);
        assert!(hand.z < -0.5, "{hand}");
        assert!((hand.y - 22.0 / 16.0).abs() < 0.1, "{hand}");
        // The right arm is on the model's right (+X).
        assert!(zombie.bones[arm].pivot.x > 0.0);
    }

    #[test]
    fn client_entities() {
        let ms = models();
        let pig = ms.client_entity("minecraft:pig").unwrap();
        assert_eq!(pig.default_geometry(), Some("geometry.pig.v3"));
        assert_eq!(pig.default_texture(), Some("textures/entity/pig/pig_v3"));
        for id in [
            "pig", "cow", "sheep", "chicken", "zombie", "skeleton", "creeper", "spider", "player",
        ] {
            let (m, tex) = ms
                .entity_model(id)
                .unwrap_or_else(|| panic!("no model for {id}"));
            assert!(m.cube_count() > 0);
            assert!(tex.starts_with("textures/entity/"), "{tex}");
        }
    }

    #[test]
    fn rest_poses_from_pack_animations() {
        let ms = models();
        // Spider legs spread down to the ground.
        let spider = ms.rest_pose("minecraft:spider");
        assert_eq!(spider.len(), 8, "{spider:?}");
        let (m, _) = ms.entity_model("spider").unwrap();
        let (lo, _) = m.bounds(&spider).unwrap();
        assert!(lo.y.abs() < 0.1, "spider feet at {}", lo.y);
        // Wolf body is turned horizontal; "keep in place" offsets are zero.
        let wolf = ms.rest_pose("minecraft:wolf");
        let body = wolf.iter().find(|p| &*p.bone == "body").unwrap();
        assert_eq!(body.rotation.x, 90.0);
        assert!(
            wolf.iter()
                .filter(|p| &*p.bone != "upperBody")
                .all(|p| p.offset.length() < 1e-4),
            "{wolf:?}"
        );
        // Sheep head stays where the geometry puts it.
        assert!(
            ms.rest_pose("minecraft:sheep")
                .iter()
                .all(|p| p.offset.length() < 1e-4)
        );
        // Mobs whose geometry is already posed get nothing surprising.
        for id in ["pig", "cow", "creeper"] {
            let (m, _) = ms.entity_model(id).unwrap();
            let rest = ms.rest_pose(id);
            assert_eq!(m.bounds(&rest), m.bounds(&[]), "{id}: {rest:?}");
        }
    }

    #[test]
    fn per_face_uv_matches_box_layout() {
        // ghast.geo.json spells out a box-UV layout per face.
        let ms = models();
        let ghast = ms.get("geometry.ghast").unwrap();
        let body = &ghast.bones[ghast.bone_index("body").unwrap()].cubes[0];
        let expect = box_uv(0.0, 0.0, [16.0, 16.0, 16.0], false);
        for f in Face::ALL {
            assert_eq!(body.faces[f.index()], expect[f.index()], "{f:?}");
        }
    }
}
