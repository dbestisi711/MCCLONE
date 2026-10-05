//! Static poses from the pack's `animations/*.json`.
//!
//! Only channels that are constant for an adult mob standing still are
//! evaluated (numbers, Molang arithmetic/logic using `this`, `math.*` and a
//! few queries with default values, see [`eval_molang`]); anything depending
//! on variables, targets or time is skipped. That covers "setup"/"default
//! pose" animations (a wolf's horizontal body, a spider's leg spread), which
//! is what a model needs to look right at rest; gameplay animation is
//! procedural (`mc-entity`).

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
            // Bone names are case-insensitive in the pack ("upperbody").
            let Some(b) = model
                .bones
                .iter()
                .find(|b| b.name.eq_ignore_ascii_case(bone))
            else {
                continue;
            };
            let rest = b.rotation;
            let rotation = eval_vec3(rot, rest).unwrap_or(Vec3::ZERO);
            // In position channels `this` is the bone's pivot in geo axes with
            // Y measured from 24 px (the legacy "Java" origin): setup
            // animations write `<pivot> - this` to mean "keep it in place".
            let pos_this = Vec3::new(-b.pivot.x, b.pivot.y - 24.0, b.pivot.z);
            let offset = eval_vec3(pos, pos_this).unwrap_or(Vec3::ZERO);
            let scale = match scale {
                Value::Null => 1.0,
                v => eval_value(v, 1.0)
                    .or_else(|| eval_vec3(v, Vec3::ONE).map(|s| (s.x + s.y + s.z) / 3.0))
                    .unwrap_or(1.0),
            };
            if rotation != Vec3::ZERO || offset != Vec3::ZERO || scale != 1.0 {
                out.push(BonePose {
                    bone: b.name.clone(),
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

/// Parse `animation_controllers/*.json`: controller id → names of the
/// animations its initial state plays unconditionally.
pub fn load_controllers(pack: &Pack) -> HashMap<Arc<str>, Vec<Arc<str>>> {
    let mut map = HashMap::default();
    for f in pack.list_files("animation_controllers", ".json") {
        let Some(v) = pack.load_json(&f) else {
            continue;
        };
        let Some(ctrls) = v["animation_controllers"].as_object() else {
            continue;
        };
        for (id, c) in ctrls {
            let initial = c["initial_state"].as_str().unwrap_or("default");
            let names = c["states"][initial]["animations"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|e| match e {
                            Value::String(s) => Some(Arc::from(s.as_str())),
                            Value::Object(m) => m.iter().next().and_then(|(k, cond)| {
                                let on = match cond {
                                    Value::String(s) => condition_holds(s),
                                    v => eval_value(v, 0.0).is_some_and(|x| x != 0.0),
                                };
                                on.then(|| Arc::from(k.as_str()))
                            }),
                            _ => None,
                        })
                        .collect()
                })
                .unwrap_or_default();
            map.insert(Arc::from(id.as_str()), names);
        }
    }
    map
}

fn eval_value(v: &Value, this: f32) -> Option<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|f| f as f32),
        Value::String(s) => eval_molang(s, this),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
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

/// Is a Molang condition (from `scripts.animate` or a controller state)
/// true for an adult mob standing still? Unknown → false.
pub fn condition_holds(src: &str) -> bool {
    eval_molang(src, 0.0).is_some_and(|v| v != 0.0)
}

/// Values assumed for queries when evaluating a static rest pose: an adult,
/// alive, standing still on the ground, not doing anything special. Other
/// queries and all variables are unknown.
fn default_query(name: &str) -> Option<f32> {
    let q = name
        .strip_prefix("query.")
        .or_else(|| name.strip_prefix("q."))?;
    Some(match q {
        "is_alive" | "is_on_ground" => 1.0,
        "is_baby"
        | "is_sitting"
        | "is_sheared"
        | "is_angry"
        | "is_charged"
        | "is_powered"
        | "is_tamed"
        | "is_saddled"
        | "is_chested"
        | "is_sneaking"
        | "is_sleeping"
        | "is_riding"
        | "is_swimming"
        | "is_in_water"
        | "is_in_water_or_rain"
        | "is_sprinting"
        | "is_moving"
        | "is_gliding"
        | "is_jumping"
        | "is_eating"
        | "is_grazing"
        | "is_shaking_wetness"
        | "is_lying_down"
        | "is_interested"
        | "is_roaring"
        | "is_stunned"
        | "is_casting"
        | "is_celebrating"
        | "is_charging"
        | "is_using_item"
        | "is_admiring"
        | "has_target"
        | "is_onfire"
        | "is_on_fire"
        | "modified_move_speed"
        | "ground_speed"
        | "walk_distance"
        | "modified_distance_moved"
        | "anim_time"
        | "life_time"
        | "time_stamp"
        | "hurt_time"
        | "variant"
        | "mark_variant"
        | "skin_id"
        | "standing_scale"
        | "sitting_scale"
        | "lying_scale" => 0.0,
        _ => return None,
    })
}

/// Evaluate a Molang expression for a static pose: numbers, `this`,
/// arithmetic, comparisons, `! && ||`, `? :`, `math.*` (degrees) and the
/// queries in [`default_query`]. Returns `None` if the result depends on
/// anything else (variables, targets, time-varying queries).
pub fn eval_molang(src: &str, this: f32) -> Option<f32> {
    let s = src.trim().trim_end_matches(';').to_ascii_lowercase();
    let mut p = Parser {
        s: s.as_bytes(),
        i: 0,
        this,
    };
    let v = p.expr().ok()?;
    p.ws();
    if p.i != p.s.len() {
        return None;
    }
    v.filter(|v| v.is_finite())
}

/// Parse result: `Err` = syntax error, `Ok(None)` = unknown value.
type Ev = Result<Option<f32>, ()>;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    this: f32,
}

fn bin(a: Option<f32>, b: Option<f32>, f: impl Fn(f32, f32) -> f32) -> Option<f32> {
    Some(f(a?, b?))
}

fn truth(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek_str(&mut self, t: &str) -> bool {
        self.ws();
        self.s[self.i..].starts_with(t.as_bytes())
    }
    fn eat_str(&mut self, t: &str) -> bool {
        if self.peek_str(t) {
            self.i += t.len();
            true
        } else {
            false
        }
    }
    fn expr(&mut self) -> Ev {
        let c = self.or()?;
        if self.peek_str("??") {
            return Err(());
        }
        if self.eat_str("?") {
            let a = self.expr()?;
            if !self.eat_str(":") {
                return Err(());
            }
            let b = self.expr()?;
            return Ok(match c {
                Some(c) if c != 0.0 => a,
                Some(_) => b,
                None => None,
            });
        }
        Ok(c)
    }
    fn or(&mut self) -> Ev {
        let mut v = self.and()?;
        while self.eat_str("||") {
            let r = self.and()?;
            v = match (v, r) {
                (Some(a), _) if a != 0.0 => Some(1.0),
                (_, Some(b)) if b != 0.0 => Some(1.0),
                (Some(_), Some(_)) => Some(0.0),
                _ => None,
            };
        }
        Ok(v)
    }
    fn and(&mut self) -> Ev {
        let mut v = self.cmp()?;
        while self.eat_str("&&") {
            let r = self.cmp()?;
            v = match (v, r) {
                (Some(a), _) if a == 0.0 => Some(0.0),
                (_, Some(b)) if b == 0.0 => Some(0.0),
                (Some(_), Some(_)) => Some(1.0),
                _ => None,
            };
        }
        Ok(v)
    }
    fn cmp(&mut self) -> Ev {
        let mut v = self.add()?;
        loop {
            let op = ["==", "!=", "<=", ">=", "<", ">"]
                .into_iter()
                .find(|op| self.peek_str(op));
            let Some(op) = op else {
                return Ok(v);
            };
            self.i += op.len();
            let r = self.add()?;
            v = bin(v, r, |a, b| {
                truth(match op {
                    "==" => a == b,
                    "!=" => a != b,
                    "<=" => a <= b,
                    ">=" => a >= b,
                    "<" => a < b,
                    _ => a > b,
                })
            });
        }
    }
    fn add(&mut self) -> Ev {
        let mut v = self.mul()?;
        loop {
            if self.eat_str("+") {
                let r = self.mul()?;
                v = bin(v, r, |a, b| a + b);
            } else if self.peek_str("-") {
                self.i += 1;
                let r = self.mul()?;
                v = bin(v, r, |a, b| a - b);
            } else {
                return Ok(v);
            }
        }
    }
    fn mul(&mut self) -> Ev {
        let mut v = self.unary()?;
        loop {
            if self.eat_str("*") {
                let r = self.unary()?;
                v = bin(v, r, |a, b| a * b);
            } else if self.eat_str("/") {
                let r = self.unary()?;
                v = bin(v, r, |a, b| if b == 0.0 { 0.0 } else { a / b });
            } else {
                return Ok(v);
            }
        }
    }
    fn unary(&mut self) -> Ev {
        if self.peek_str("!=") {
            return Err(());
        }
        if self.eat_str("!") {
            return Ok(self.unary()?.map(|v| truth(v == 0.0)));
        }
        if self.eat_str("-") {
            return Ok(self.unary()?.map(|v| -v));
        }
        if self.eat_str("+") {
            return self.unary();
        }
        self.primary()
    }
    fn primary(&mut self) -> Ev {
        self.ws();
        if self.eat_str("(") {
            let v = self.expr()?;
            return if self.eat_str(")") { Ok(v) } else { Err(()) };
        }
        let start = self.i;
        let c = *self.s.get(self.i).ok_or(())?;
        if c.is_ascii_digit() || c == b'.' {
            while self.i < self.s.len()
                && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.')
            {
                self.i += 1;
            }
            let txt = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| ())?;
            if self.s.get(self.i) == Some(&b'f') {
                self.i += 1;
            }
            return txt.parse::<f32>().map(Some).map_err(|_| ());
        }
        if !(c.is_ascii_alphabetic() || c == b'_') {
            return Err(());
        }
        while self.i < self.s.len()
            && (self.s[self.i].is_ascii_alphanumeric()
                || self.s[self.i] == b'_'
                || self.s[self.i] == b'.')
        {
            self.i += 1;
        }
        let name = std::str::from_utf8(&self.s[start..self.i])
            .map_err(|_| ())?
            .to_string();
        // Arguments (for functions and parameterised queries).
        let mut args: Vec<Option<f32>> = Vec::new();
        let has_args = self.peek_str("(");
        if has_args {
            self.i += 1;
            if !self.eat_str(")") {
                loop {
                    if self.peek_str("'") {
                        // String argument (e.g. query.property('x')): skip it.
                        self.i += 1;
                        while self.i < self.s.len() && self.s[self.i] != b'\'' {
                            self.i += 1;
                        }
                        self.i += 1;
                        args.push(None);
                    } else {
                        args.push(self.expr()?);
                    }
                    if self.eat_str(")") {
                        break;
                    }
                    if !self.eat_str(",") {
                        return Err(());
                    }
                }
            }
        }
        match name.as_str() {
            "this" => return Ok(Some(self.this)),
            "true" => return Ok(Some(1.0)),
            "false" => return Ok(Some(0.0)),
            "math.pi" => return Ok(Some(std::f32::consts::PI)),
            _ => {}
        }
        if let Some(f) = name.strip_prefix("math.") {
            let a = |k: usize| args.get(k).copied().flatten();
            let r = (|| {
                Some(match f {
                    "sin" => a(0)?.to_radians().sin(),
                    "cos" => a(0)?.to_radians().cos(),
                    "abs" => a(0)?.abs(),
                    "sqrt" => a(0)?.max(0.0).sqrt(),
                    "floor" => a(0)?.floor(),
                    "ceil" => a(0)?.ceil(),
                    "round" => a(0)?.round(),
                    "trunc" => a(0)?.trunc(),
                    "min" => a(0)?.min(a(1)?),
                    "max" => a(0)?.max(a(1)?),
                    "clamp" => a(0)?.clamp(a(1)?, a(2)?.max(a(1)?)),
                    "lerp" => a(0)? + (a(1)? - a(0)?) * a(2)?,
                    _ => return None,
                })
            })();
            return Ok(r);
        }
        if has_args {
            return Ok(None);
        }
        Ok(default_query(&name))
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
        assert_eq!(eval_molang("query.is_baby ? 0 : 1", 0.0), Some(1.0));
        assert_eq!(eval_molang("!query.is_baby && 2 > 1", 0.0), Some(1.0));
        assert_eq!(
            eval_molang("q.is_baby ? 0.0 : (-6.0 - this)", 1.0),
            Some(-7.0)
        );
        assert_eq!(eval_molang("variable.x", 0.0), None);
        assert_eq!(eval_molang("query.target_x_rotation", 0.0), None);
        assert_eq!(
            eval_molang(
                "math.cos(query.anim_time * 38.17) * 80.0 * query.modified_move_speed",
                0.0
            ),
            Some(0.0)
        );
        assert!(condition_holds("!query.is_baby"));
        assert!(!condition_holds("query.is_sitting"));
        assert!(!condition_holds("variable.foo"));
    }
}
