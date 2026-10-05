// MCCLONE world shaders. All lighting is computed MC-style in gamma space
// (per-level brightness curve, face shading, ambient occlusion) and then
// converted to linear before multiplying the (sRGB-decoded) textures, so the
// image matches the familiar look while output and blending stay sRGB-correct.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz: camera world position, w: seconds since start (animation clock)
    cam: vec4<f32>,
    // xyz: sun direction, w: day factor 0..1
    sun: vec4<f32>,
    // rgb: zenith colour (linear), w: star visibility
    zenith: vec4<f32>,
    // rgb: horizon colour (linear), w: sunrise/sunset glow strength
    horizon: vec4<f32>,
    // rgb: sunrise/sunset glow colour, w: sky light dimming in levels (night)
    glow: vec4<f32>,
    // x: fog start, y: fog end, z: mode (0 air, 1 water, 2 lava), w: unused
    fog: vec4<f32>,
    // rgb: fog colour for water/lava (linear)
    fog_color: vec4<f32>,
    // rgb: sky light colour (gamma), w: game ticks (animation)
    sky_light: vec4<f32>,
    // rgb: block light colour (gamma)
    block_light: vec4<f32>,
    // width, height, 1/width, 1/height
    screen: vec4<f32>,
    // xyz: cloud mesh origin relative to the camera, w: cloud alpha
    cloud: vec4<f32>,
    // rgb: cloud colour (linear), w: cloud fade distance
    cloud_color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var samp_blocks: sampler;
//#BLOCK_ARRAYS#

@group(1) @binding(0) var tex2d: texture_2d<f32>;
@group(1) @binding(1) var samp2d: sampler;

//#SAMPLE_BLOCK#

// ---------------------------------------------------------------- helpers

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    return pow(max(c, vec3<f32>(0.0)), vec3<f32>(2.2));
}

// Brightness of a light level (gamma space): each level is ~20% dimmer,
// then lifted like a mid "brightness" setting so caves stay readable.
fn light_curve(level: f32) -> f32 {
    let b = pow(0.8, 15.0 - clamp(level, 0.0, 15.0));
    let lifted = 1.0 - pow(1.0 - b, 4.0);
    return mix(b, lifted, 0.45);
}

// Sky light (dimmed at night) and warm block light, combined per channel.
fn world_light(sky: f32, blk: f32) -> vec3<f32> {
    let s = light_curve(sky - g.glow.w) * g.sky_light.rgb;
    let b = light_curve(blk) * g.block_light.rgb * step(0.01, blk);
    return max(max(s, b), vec3<f32>(0.02));
}

fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let y = dir.y;
    let t = pow(1.0 - clamp(y, 0.0, 1.0), 3.0);
    var c = mix(g.zenith.rgb, g.horizon.rgb, t);
    if (y < 0.0) {
        c = g.horizon.rgb * mix(1.0, 0.7, clamp(-y * 2.0, 0.0, 1.0));
    }
    // Sunrise/sunset glow around the sun's azimuth, close to the horizon.
    let hl = length(dir.xz);
    let sh = vec2<f32>(g.sun.x, g.sun.z);
    let shl = length(sh);
    if (g.horizon.w > 0.0 && hl > 0.0001 && shl > 0.0001) {
        let facing = dot(dir.xz / hl, sh / shl) * 0.5 + 0.5;
        let band = pow(1.0 - min(abs(y), 1.0), 5.0);
        let k = g.horizon.w * pow(facing, 2.5) * band;
        c = mix(c, g.glow.rgb, clamp(k, 0.0, 1.0));
    }
    return c;
}

fn fog_amount(rel: vec3<f32>) -> f32 {
    if (g.fog.z > 0.5) {
        let d = length(rel);
        return clamp((d - g.fog.x) / max(g.fog.y - g.fog.x, 0.001), 0.0, 1.0);
    }
    let d = max(length(rel.xz), abs(rel.y));
    return smoothstep(g.fog.x, g.fog.y, d);
}

fn fog_tint(rel: vec3<f32>) -> vec3<f32> {
    if (g.fog.z > 0.5) {
        return g.fog_color.rgb;
    }
    let h = vec3<f32>(rel.x, 0.0, rel.z);
    let l = length(h);
    if (l < 0.001) {
        return g.horizon.rgb;
    }
    return sky_color(h / l);
}


fn face_shade(n: u32) -> f32 {
    var shades = array<f32, 8>(1.0, 0.5, 0.8, 0.8, 0.6, 0.6, 0.9, 1.0);
    return shades[min(n, 7u)];
}

fn ao_factor(ao: u32) -> f32 {
    var f = array<f32, 4>(0.48, 0.62, 0.8, 1.0);
    return f[min(ao, 3u)];
}

// ---------------------------------------------------------------- sky

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
}

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let x = f32((i << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(i & 2u) * 2.0 - 1.0;
    var o: SkyOut;
    o.clip = vec4<f32>(x, y, 0.0, 1.0);
    o.ndc = vec2<f32>(x, y);
    return o;
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn stars(dir: vec3<f32>) -> f32 {
    // The star sphere turns with the sun (around the z axis).
    let ang = atan2(g.sun.y, g.sun.x);
    let ca = cos(-ang);
    let sa = sin(-ang);
    let d = vec3<f32>(dir.x * ca - dir.y * sa, dir.x * sa + dir.y * ca, dir.z);
    let a = abs(d);
    var uv: vec2<f32>;
    var face: f32;
    if (a.x >= a.y && a.x >= a.z) {
        uv = d.yz / a.x;
        face = select(0.0, 1.0, d.x > 0.0);
    } else if (a.y >= a.z) {
        uv = d.xz / a.y;
        face = select(2.0, 3.0, d.y > 0.0);
    } else {
        uv = d.xy / a.z;
        face = select(4.0, 5.0, d.z > 0.0);
    }
    let n = 80.0;
    let p = (uv * 0.5 + 0.5) * n;
    let cell = floor(p);
    let seed = cell + vec2<f32>(face * 131.0, face * 71.0);
    if (hash2(seed) > 0.045) {
        return 0.0;
    }
    let center = vec2<f32>(hash2(seed + 17.0), hash2(seed + 41.0)) * 0.6 + 0.2;
    let dist = length(p - cell - center);
    let size = 0.07 + 0.1 * hash2(seed + 99.0);
    return smoothstep(size, size * 0.3, dist) * (0.35 + 0.65 * hash2(seed + 7.0));
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    if (g.fog.z > 0.5) {
        return vec4<f32>(g.fog_color.rgb, 1.0);
    }
    let p = g.inv_view_proj * vec4<f32>(in.ndc, 0.5, 1.0);
    let dir = normalize(p.xyz / p.w);
    var c = sky_color(dir);
    // Soft halo around the sun.
    let sd = max(dot(dir, g.sun.xyz), 0.0);
    c += vec3<f32>(1.0, 0.85, 0.6) * pow(sd, 32.0) * 0.35 * g.sun.w;
    if (g.zenith.w > 0.0 && dir.y > -0.1) {
        let fade = smoothstep(-0.1, 0.15, dir.y);
        c += vec3<f32>(0.9, 0.92, 1.0) * stars(dir) * g.zenith.w * fade;
    }
    return vec4<f32>(c, 1.0);
}

// ---------------------------------------------------------------- chunks

struct ChunkOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) light: vec3<f32>,
    @location(3) tint: vec3<f32>,
    @location(4) rel: vec3<f32>,
    // normal (3 bits) | isotropic << 3
    @location(5) @interpolate(flat) info: u32,
    // fog colour (evaluated per vertex: the sky gradient is costly per pixel)
    @location(6) fog: vec3<f32>,
}

@vertex
fn vs_chunk(
    @location(0) w0: u32,
    @location(1) w1: u32,
    @location(2) w2: u32,
    @location(3) origin: vec4<f32>,
) -> ChunkOut {
    let p = vec3<f32>(f32(w0 & 511u), f32((w0 >> 9u) & 511u), f32((w0 >> 18u) & 511u)) / 16.0 - vec3<f32>(1.0);
    let normal = (w0 >> 27u) & 7u;
    let flags = w0 >> 30u;
    var rel = origin.xyz + p;
    let world = rel + g.cam.xyz;
    if ((w2 >> 29u) & 1u) != 0u {
        // Seal T-junctions: grow the face a hair in its plane, more with
        // distance (sub-pixel everywhere).
        let corner = (w2 >> 27u) & 3u;
        let sr = select(-1.0, 1.0, corner == 1u || corner == 2u);
        let su = select(-1.0, 1.0, corner >= 2u);
        let e = max(0.0008, length(rel) * 0.00012);
        rel += (face_right(normal) * sr + face_up(normal) * su) * e;
    }
    let uv = vec2<f32>(f32(w1 & 511u), f32((w1 >> 9u) & 511u)) / 16.0;
    if ((flags & 1u) != 0u) {
        // Leaves and plants sway a little (plants only at the top).
        let t = g.cam.w;
        let ph = world.x * 0.7 + world.z * 0.43 + world.y * 0.2;
        var amp = 0.02;
        if (normal == 6u) {
            amp = 0.07 * (1.0 - uv.y);
        }
        rel.x += sin(t * 1.7 + ph) * amp;
        rel.z += cos(t * 1.3 + ph * 1.1) * amp;
    }
    let tile = (w1 >> 18u) & 4095u;
    let ao = w1 >> 30u;
    let sky = f32((w2 >> 16u) & 31u) / 2.0;
    let blk = f32((w2 >> 21u) & 31u) / 2.0;
    let tint = vec3<f32>(
        f32(w2 & 31u) / 31.0,
        f32((w2 >> 5u) & 63u) / 63.0,
        f32((w2 >> 11u) & 31u) / 31.0,
    );
    let l = world_light(sky, blk) * face_shade(normal) * ao_factor(ao);
    var o: ChunkOut;
    o.clip = g.view_proj * vec4<f32>(rel, 1.0);
    o.uv = uv;
    o.layer = tile;
    o.light = to_linear(l);
    o.tint = to_linear(tint);
    o.rel = rel;
    o.info = normal | (((w2 >> 26u) & 1u) << 3u);
    o.fog = fog_tint(rel);
    return o;
}

fn face_right(n: u32) -> vec3<f32> {
    var r = array<vec3<f32>, 8>(
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 0.0),
    );
    return r[min(n, 7u)];
}

fn face_up(n: u32) -> vec3<f32> {
    var u = array<vec3<f32>, 8>(
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 0.0),
    );
    return u[min(n, 7u)];
}

fn face_normal(n: u32) -> vec3<f32> {
    var normals = array<vec3<f32>, 8>(
        vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, -1.0, 0.0),
        vec3<f32>(0.0, 0.0, -1.0),
        vec3<f32>(0.0, 0.0, 1.0),
        vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(-1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 0.0),
    );
    return normals[min(n, 7u)];
}

// Sample a chunk fragment's tile. Isotropic faces (grass top, sand...) get
// a random quarter turn per block, computed from the world position so it
// also works on greedy-merged quads.
fn chunk_sample(in: ChunkOut) -> vec4<f32> {
    let ddx = dpdx(in.uv);
    let ddy = dpdy(in.uv);
    var uv = in.uv;
    if ((in.info & 8u) != 0u) {
        let n = in.info & 7u;
        let wp = in.rel + g.cam.xyz;
        let b = floor(wp - face_normal(n) * 0.001);
        let lp = clamp(wp - b, vec3<f32>(0.0), vec3<f32>(0.9999));
        var f: vec2<f32>;
        switch n {
            case 0u: { f = vec2<f32>(lp.x, lp.z); }
            case 1u: { f = vec2<f32>(lp.x, 1.0 - lp.z); }
            case 2u: { f = vec2<f32>(1.0 - lp.x, 1.0 - lp.y); }
            case 3u: { f = vec2<f32>(lp.x, 1.0 - lp.y); }
            case 4u: { f = vec2<f32>(1.0 - lp.z, 1.0 - lp.y); }
            default: { f = vec2<f32>(lp.z, 1.0 - lp.y); }
        }
        let bi = vec3<i32>(b);
        let h = (u32(bi.x) * 0x9E3779B1u) ^ (u32(bi.z) * 0x85EBCA77u) ^ (u32(bi.y) * 0xC2B2AE3Du);
        let r = ((h * 0x27D4EB2Fu) >> 13u) & 3u;
        if (r == 1u) {
            f = vec2<f32>(1.0 - f.y, f.x);
        } else if (r == 2u) {
            f = vec2<f32>(1.0) - f;
        } else if (r == 3u) {
            f = vec2<f32>(f.y, 1.0 - f.x);
        }
        uv = f;
    }
    return sample_block_grad(uv, in.layer, ddx, ddy);
}

@fragment
fn fs_chunk_opaque(in: ChunkOut) -> @location(0) vec4<f32> {
    let t = chunk_sample(in);
    // Opaque Bedrock textures carry a tint mask in alpha (grass sides).
    let c = t.rgb * mix(vec3<f32>(1.0), in.tint, t.a) * in.light;
    return vec4<f32>(mix(c, in.fog, fog_amount(in.rel)), 1.0);
}

@fragment
fn fs_chunk_cutout(in: ChunkOut) -> @location(0) vec4<f32> {
    let t = chunk_sample(in);
    if (t.a < 0.5) {
        discard;
    }
    let c = t.rgb * in.tint * in.light;
    return vec4<f32>(mix(c, in.fog, fog_amount(in.rel)), 1.0);
}

@fragment
fn fs_chunk_translucent(in: ChunkOut) -> @location(0) vec4<f32> {
    let t = chunk_sample(in);
    let c = t.rgb * in.tint * in.light;
    let f = fog_amount(in.rel);
    let a = mix(t.a, 1.0, f * f);
    return vec4<f32>(mix(c, in.fog, f), a);
}

// ---------------------------------------------------------------- dynamic geometry
// Entities, dropped items, falling blocks, crack overlay, sun and moon.

struct DynIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    // rgba tint (sRGB)
    @location(2) color: vec4<f32>,
    // sky/15, block/15, face shade, hurt flash
    @location(3) light: vec4<f32>,
    @location(4) layer: u32,
}

struct DynOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // tile layer; bit 31 set = opaque tile whose alpha is a tint mask
    @location(1) @interpolate(flat) layer: u32,
    @location(2) light: vec3<f32>,
    @location(3) tint: vec4<f32>,
    @location(4) rel: vec3<f32>,
    @location(5) hurt: f32,
    @location(6) fog: vec3<f32>,
}

@vertex
fn vs_dyn(v: DynIn) -> DynOut {
    return dyn_vertex(v);
}

fn dyn_vertex(v: DynIn) -> DynOut {
    let l = world_light(v.light.x * 15.0, v.light.y * 15.0) * v.light.z;
    var o: DynOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    o.uv = v.uv;
    o.layer = v.layer;
    o.light = to_linear(l);
    o.tint = vec4<f32>(to_linear(v.color.rgb), v.color.a);
    o.rel = v.pos;
    o.hurt = v.light.w;
    o.fog = fog_tint(v.pos);
    return o;
}

fn finish_dyn(rgb: vec3<f32>, a: f32, in: DynOut) -> vec4<f32> {
    let c = mix(rgb * in.light, vec3<f32>(0.6, 0.0, 0.0), in.hurt * 0.55);
    return vec4<f32>(mix(c, in.fog, fog_amount(in.rel)), a);
}

@fragment
fn fs_dyn_tex(in: DynOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex2d, samp2d, in.uv);
    if (t.a * in.tint.a < 0.5) {
        discard;
    }
    return finish_dyn(t.rgb * in.tint.rgb, 1.0, in);
}

@fragment
fn fs_dyn_block(in: DynOut) -> @location(0) vec4<f32> {
    let t = sample_block(in.uv, in.layer & 0x7fffffffu);
    if ((in.layer >> 31u) != 0u) {
        return finish_dyn(t.rgb * mix(vec3<f32>(1.0), in.tint.rgb, t.a), 1.0, in);
    }
    if (t.a < 0.5) {
        discard;
    }
    return finish_dyn(t.rgb * in.tint.rgb, 1.0, in);
}

// Crack overlay: multiplied with what's behind (dark cracks on a
// transparent tile).
@fragment
fn fs_crack(in: DynOut) -> @location(0) vec4<f32> {
    let t = sample_block(in.uv, in.layer & 0x7fffffffu);
    if (t.a < 0.1) {
        discard;
    }
    return vec4<f32>(mix(vec3<f32>(1.0), t.rgb, t.a), 1.0);
}

// First-person hand / held item: like `vs_dyn`, but depth is squeezed
// next to the near plane so it is never hidden inside walls.
@vertex
fn vs_dyn_hand(v: DynIn) -> DynOut {
    var o = dyn_vertex(v);
    o.clip.z = o.clip.w * 0.92 + o.clip.z * 0.08;
    return o;
}

// Sun and moon: additive, unlit.
@vertex
fn vs_sprite(v: DynIn) -> DynOut {
    var o: DynOut;
    o.clip = g.view_proj * vec4<f32>(v.pos, 1.0);
    // At infinity: only drawn where nothing else is (depth still 0).
    o.clip.z = 0.0;
    o.uv = v.uv;
    o.layer = v.layer;
    o.light = vec3<f32>(1.0);
    o.tint = v.color;
    o.rel = v.pos;
    o.hurt = 0.0;
    o.fog = vec3<f32>(0.0);
    return o;
}

@fragment
fn fs_sprite(in: DynOut) -> @location(0) vec4<f32> {
    let t = textureSample(tex2d, samp2d, in.uv);
    return vec4<f32>(t.rgb * t.a * in.tint.rgb * in.tint.a, 1.0);
}

// ---------------------------------------------------------------- clouds

struct CloudOut {
    @builtin(position) @invariant clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) rel: vec3<f32>,
    @location(2) sky: vec3<f32>,
}

@vertex
fn vs_cloud(@location(0) pos: vec3<f32>, @location(1) shade: f32) -> CloudOut {
    let rel = pos + g.cloud.xyz;
    var c = g.cloud_color.rgb * shade;
    let h = vec3<f32>(rel.x, 0.0, rel.z);
    var sky = c;
    if (length(h) > 0.001) {
        sky = sky_color(normalize(h));
    }
    var o: CloudOut;
    o.clip = g.view_proj * vec4<f32>(rel, 1.0);
    o.color = vec4<f32>(c, 1.0);
    o.rel = rel;
    o.sky = sky;
    return o;
}

@fragment
fn fs_cloud(in: CloudOut) -> @location(0) vec4<f32> {
    let fade = 1.0 - smoothstep(g.cloud_color.w * 0.55, g.cloud_color.w, length(in.rel.xz));
    let c = mix(in.color.rgb, in.sky, (1.0 - fade) * 0.8);
    return vec4<f32>(c, g.cloud.w * fade);
}

// ---------------------------------------------------------------- lines

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
}

@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var o: LineOut;
    o.clip = g.view_proj * vec4<f32>(pos, 1.0);
    // Pull slightly toward the camera to avoid z-fighting with the block.
    o.clip.z = o.clip.z * 1.0005;
    o.color = vec4<f32>(to_linear(color.rgb), color.a);
    return o;
}

@fragment
fn fs_line(in: LineOut) -> @location(0) vec4<f32> {
    return in.color;
}

// ---------------------------------------------------------------- UI

struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) layer: u32,
}

@vertex
fn vs_ui(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
    @location(3) layer: u32,
) -> UiOut {
    var o: UiOut;
    let ndc = pos * g.screen.zw * 2.0 - vec2<f32>(1.0);
    o.clip = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.uv = uv;
    o.color = color;
    o.layer = layer;
    return o;
}

@fragment
fn fs_ui_tex(in: UiOut) -> @location(0) vec4<f32> {
    return textureSample(tex2d, samp2d, in.uv) * in.color;
}

@fragment
fn fs_ui_block(in: UiOut) -> @location(0) vec4<f32> {
    return sample_block(in.uv, in.layer) * in.color;
}
