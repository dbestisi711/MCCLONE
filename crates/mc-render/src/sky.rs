//! Time-of-day atmosphere: sun direction, sky gradient colours, sunrise and
//! sunset glow, star visibility, night dimming of sky light, cloud colour.
//! The shader turns these into the sky, the fog colour and the lighting.

use glam::Vec3;
use mc_core::DAY_LENGTH_TICKS;
use mc_core::render_types::SkyState;

#[derive(Clone, Copy, Debug)]
pub struct SkyParams {
    pub sun_dir: Vec3,
    /// 0 at night, 1 in full day.
    pub day: f32,
    pub zenith: Vec3,
    pub horizon: Vec3,
    pub glow: Vec3,
    pub glow_strength: f32,
    pub stars: f32,
    /// Light levels subtracted from sky light (0 by day, 10 at night).
    pub sky_dim: f32,
    /// Sky light colour (gamma space).
    pub sky_light: Vec3,
    pub block_light: Vec3,
    pub cloud: Vec3,
    pub moon_phase: u32,
    pub sun_alpha: f32,
    pub moon_alpha: f32,
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn rgb_linear(c: u32) -> Vec3 {
    Vec3::new(
        srgb_to_linear(((c >> 16) & 255) as f32 / 255.0),
        srgb_to_linear(((c >> 8) & 255) as f32 / 255.0),
        srgb_to_linear((c & 255) as f32 / 255.0),
    )
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sun direction for a time of day (0 = sunrise in the east (+X), 6000 =
/// noon overhead, 12000 = sunset in the west). The path is tilted a little
/// toward the south so the noon sun is not exactly at the zenith.
pub fn sun_direction(time_of_day: u64) -> Vec3 {
    let t = (time_of_day % DAY_LENGTH_TICKS) as f32 / DAY_LENGTH_TICKS as f32;
    let a = t * std::f32::consts::TAU;
    let tilt = 0.18f32;
    Vec3::new(a.cos(), a.sin() * tilt.cos(), a.sin() * tilt.sin()).normalize()
}

pub fn sky_params(sky: &SkyState, total_ticks: u64) -> SkyParams {
    let sun_dir = sun_direction(sky.time_of_day);
    let rain = sky.rain.clamp(0.0, 1.0);
    let e = sun_dir.y;
    // Visual day factor from the sun's elevation: full daylight until the
    // sun gets low, then a quick dusk while it crosses the horizon.
    // (`SkyState::daylight` is a plain cosine meant for game logic.)
    let day = smoothstep(-0.2, 0.12, e);

    let zenith_day = rgb_linear(sky.sky_color) * 0.9;
    let horizon_day = rgb_linear(sky.fog_color);
    let zenith_night = Vec3::new(0.0012, 0.0018, 0.0065);
    let horizon_night = Vec3::new(0.0045, 0.0065, 0.016);
    let k = day.powf(1.3);
    let grey = |v: Vec3| {
        let l = v.dot(Vec3::new(0.3, 0.59, 0.11));
        v.lerp(Vec3::splat(l), 0.7 * rain) * (1.0 - 0.45 * rain)
    };
    let zenith = grey(zenith_night.lerp(zenith_day, k));
    let horizon = grey(horizon_night.lerp(horizon_day, k));

    // Sunrise / sunset: strongest when the sun crosses the horizon.
    let glow_strength = (1.0 - (e.abs() / 0.42)).clamp(0.0, 1.0).powf(1.6) * 0.9 * (1.0 - rain);
    let warm = smoothstep(-0.25, 0.2, e);
    let glow = Vec3::new(0.95, 0.22, 0.05).lerp(Vec3::new(1.0, 0.45, 0.15), warm);

    let stars = (1.0 - day * 2.2).clamp(0.0, 1.0) * (1.0 - rain);
    let sky_dim = (1.0 - day) * 10.0;
    let sunset_tint = Vec3::new(1.0, 0.82, 0.68);
    let night_tint = Vec3::new(0.7, 0.76, 1.0);
    let sky_light = night_tint
        .lerp(Vec3::ONE, smoothstep(0.0, 0.6, day))
        .lerp(sunset_tint, glow_strength * 0.5);
    let block_light = Vec3::new(1.0, 0.88, 0.72);
    let cloud_day = Vec3::new(1.0, 1.0, 1.0) * (1.0 - 0.5 * rain);
    let cloud_night = Vec3::new(0.035, 0.04, 0.06);
    let cloud = cloud_night
        .lerp(cloud_day, k)
        .lerp(Vec3::new(1.0, 0.55, 0.35), glow_strength * 0.45);
    let day_index = total_ticks / DAY_LENGTH_TICKS;
    SkyParams {
        sun_dir,
        day,
        zenith,
        horizon,
        glow,
        glow_strength,
        stars,
        sky_dim,
        sky_light,
        block_light,
        cloud,
        moon_phase: (day_index % 8) as u32,
        sun_alpha: smoothstep(-0.12, 0.05, e) * (1.0 - rain),
        moon_alpha: smoothstep(-0.12, 0.05, -e) * (1.0 - rain),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_and_midnight_dark() {
        let noon = sky_params(
            &SkyState {
                time_of_day: 6000,
                daylight: 1.0,
                ..Default::default()
            },
            6000,
        );
        assert!(noon.sun_dir.y > 0.9);
        assert!(noon.stars == 0.0 && noon.sky_dim == 0.0);
        let night = sky_params(
            &SkyState {
                time_of_day: 18000,
                daylight: 0.2,
                ..Default::default()
            },
            18000,
        );
        assert!(night.sun_dir.y < -0.9);
        assert!(night.stars > 0.9);
        assert!(night.zenith.length() < noon.zenith.length() * 0.1);
        let sunset = sky_params(
            &SkyState {
                time_of_day: 12000,
                daylight: 0.6,
                ..Default::default()
            },
            12000,
        );
        assert!(sunset.glow_strength > 0.5);
    }
}
