//! Static poses from the pack's `animations/*.json`.
//!
//! Only channels that are constant (numbers, or Molang arithmetic using
//! `this` and `math.*`) are evaluated; anything depending on queries,
//! variables or time is skipped. That covers "setup"/"default pose"
//! animations (e.g. how a spider holds its legs), which is what a model needs
//! to look right at rest; gameplay animation is procedural (`mc-entity`).

use std::sync::Arc;

use glam::Vec3;
use mc_core::render_types::BonePose;
use rustc_hash::FxHashMap as HashMap;
use serde_json::Value;

use crate::{EntityModel, Pack};

/// One animation's bone channels (raw JSON values, evaluated on demand).
#[derive(Clone, Debug, Default)]
pub struct Animation {
    pub id: Arc<str>,
    /// (bone, rotation, position, scale) channel values as in the file.
    pub bones: Vec<(Arc<str>, Value, Value, Value)>,
}

impl Animation {
    /// The constant part of this animation as bone poses for `model`.
    /// Channels that aren't constant are left at rest.
    pub fn static_pose(&self, model: &EntityModel) -> Vec<BonePose> {
        let mut out = Vec::new();
        for (bone, rot, pos, scale) in &self.bones {
            let Some(b) = model.bones.iter().find(|b| b.name == *bone) else {
                continue;
            };
            let rest = b.rotation;
            let rotation = eval_vec3(rot, rest).unwrap_or(Vec3::ZERO);
            let offset = eval_vec3(pos, Vec3::ZERO).unwrap_or(Vec3::ZERO);
            let scale = match scale {
                Value::Null => 1.0,
                v => eval_value(v, 1.0)
                    .or_else(|| eval_vec3(v, Vec3::ONE).map(|s| (s.x + s.y + s.z) / 3.0))
                    .unwrap_or(1.0),
            };
            if rotation != Vec3::ZERO || offset != Vec3::ZERO || scale != 1.0 {
                out.push(BonePose {
                    bone: bone.clone(),
                    rotation,
                    offset,
                    scale,
                });
            }
        }
        out
    }
}

/// Parse every animation file in `animations/`.
pub fn load_animations(pack: &Pack) -> HashMap<Arc<str>, Arc<Animation>> {
    let mut map = HashMap::default();
    for f in pack.list_files("animations", ".json") {
        let Some(v) = pack.load_json(&f) else {
            continue;
        };
        let Some(anims) = v["animations"].as_object() else {
            continue;
        };
        for (id, a) in anims {
            let mut bones = Vec::new();
            if let Some(bs) = a["bones"].as_object() {
                for (name, ch) in bs {
                    bones.push((
                        Arc::from(name.as_str()),
                        ch["rotation"].clone(),
                        ch["position"].clone(),
                        ch["scale"].clone(),
                    ));
                }
            }
            map.insert(
                Arc::from(id.as_str()),
                Arc::new(Animation {
                    id: Arc::from(id.as_str()),
                    bones,
                }),
            );
        }
    }
    map
}

fn eval_value(v: &Value, this: f32) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => eval_molang(s, this),
        _ => None,
    }
}

fn eval_vec3(v: &Value, this: Vec3) -> Option<Vec3> {
    match v {
        Value::Array(a) if a.len() == 3 => Some(Vec3::new(
            eval_value(&a[0], this.x)?,
            eval_value(&a[1], this.y)?,
            eval_value(&a[2], this.z)?,
        )),
        Value::Number(_) | Value::String(_) => {
            let s = eval_value(v, this.x)?;
            Some(Vec3::splat(s))
        }
        _ => None,
    }
}

/// Evaluate a constant Molang expression (numbers, `this`, `+ - * /`,
/// parentheses, `math.*` functions in degrees). Returns `None` for anything
/// else (queries, variables, conditionals).
pub fn eval_molang(src: &str, this: f32) -> Option<f32> {
    let s = src.trim().trim_end_matches(';').to_ascii_lowercase();
    let mut p = Parser {
        s: s.as_bytes(),
        i: 0,
        this,
    };
    let v = p.expr()?;
    p.ws();
    (p.i == p.s.len() && v.is_finite()).then_some(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    this: f32,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.s.get(self.i) == Some(&c) {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Option<f32> {
        let mut v = self.term()?;
        loop {
            if self.eat(b'+') {
                v += self.term()?;
            } else if self.eat(b'-') {
                v -= self.term()?;
            } else {
                return Some(v);
            }
        }
    }
    fn term(&mut self) -> Option<f32> {
        let mut v = self.unary()?;
        loop {
            if self.eat(b'*') {
                v *= self.unary()?;
            } else if self.eat(b'/') {
                let d = self.unary()?;
                v = if d == 0.0 { 0.0 } else { v / d };
            } else {
                return Some(v);
            }
        }
    }
    fn unary(&mut self) -> Option<f32> {
        if self.eat(b'-') {
            return Some(-self.unary()?);
        }
        if self.eat(b'+') {
            return self.unary();
        }
        self.primary()
    }
    fn primary(&mut self) -> Option<f32> {
        self.ws();
        if self.eat(b'(') {
            let v = self.expr()?;
            return self.eat(b')').then_some(v);
        }
        let start = self.i;
        let c = *self.s.get(self.i)?;
        if c.is_ascii_digit() || c == b'.' {
            while self.i < self.s.len()
                && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.')
            {
                self.i += 1;
            }
            let txt = std::str::from_utf8(&self.s[start..self.i]).ok()?;
            // Optional float suffix.
            if self.s.get(self.i) == Some(&b'f') {
                self.i += 1;
            }
            return txt.parse().ok();
        }
        if c.is_ascii_alphabetic() || c == b'_' {
            while self.i < self.s.len()
                && (self.s[self.i].is_ascii_alphanumeric()
                    || self.s[self.i] == b'_'
                    || self.s[self.i] == b'.')
            {
                self.i += 1;
            }
            let name = std::str::from_utf8(&self.s[start..self.i])
                .ok()?
                .to_string();
            match name.as_str() {
                "this" => return Some(self.this),
                "math.pi" => return Some(std::f32::consts::PI),
                _ => {}
            }
            let f = name.strip_prefix("math.")?;
            if !self.eat(b'(') {
                return None;
            }
            let mut args = Vec::new();
            if !self.eat(b')') {
                loop {
                    args.push(self.expr()?);
                    if self.eat(b')') {
                        break;
                    }
                    if !self.eat(b',') {
                        return None;
                    }
                }
            }
            let a = |k: usize| args.get(k).copied();
            return match f {
                "sin" => Some(a(0)?.to_radians().sin()),
                "cos" => Some(a(0)?.to_radians().cos()),
                "abs" => Some(a(0)?.abs()),
                "sqrt" => Some(a(0)?.max(0.0).sqrt()),
                "floor" => Some(a(0)?.floor()),
                "ceil" => Some(a(0)?.ceil()),
                "round" => Some(a(0)?.round()),
                "trunc" => Some(a(0)?.trunc()),
                "min" => Some(a(0)?.min(a(1)?)),
                "max" => Some(a(0)?.max(a(1)?)),
                "clamp" => Some(a(0)?.clamp(a(1)?, a(2)?.max(a(1)?))),
                "lerp" => Some(a(0)? + (a(1)? - a(0)?) * a(2)?),
                _ => None,
            };
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn molang_constants() {
        assert_eq!(eval_molang("45.0 - this", 5.0), Some(40.0));
        assert_eq!(eval_molang("-this", 90.0), Some(-90.0));
        assert_eq!(eval_molang("(1 + 2) * 3", 0.0), Some(9.0));
        assert!((eval_molang("math.sin(90)", 0.0).unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(eval_molang("query.is_baby ? 0 : 1", 0.0), None);
        assert_eq!(eval_molang("variable.x", 0.0), None);
    }
}
