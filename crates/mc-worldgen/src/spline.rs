//! Nested cubic splines.
//!
//! A [`Spline`] maps one climate coordinate (continentalness, erosion or
//! peaks/valleys) to a value with cubic Hermite interpolation between control
//! points. A control point's value can itself be another spline over a
//! different coordinate, which is how several climate parameters are combined
//! into one terrain height: "at this continentalness, the height follows this
//! erosion curve, whose points in turn follow these peaks/valleys curves".

/// Which climate parameter a spline is indexed by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coord {
    Continentalness,
    Erosion,
    PeaksValleys,
}

/// The climate parameters a spline can read.
#[derive(Clone, Copy, Debug, Default)]
pub struct SplineInput {
    pub c: f32,
    pub e: f32,
    pub pv: f32,
}

impl SplineInput {
    #[inline]
    fn get(&self, c: Coord) -> f32 {
        match c {
            Coord::Continentalness => self.c,
            Coord::Erosion => self.e,
            Coord::PeaksValleys => self.pv,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Const(f32),
    Nested(Box<Spline>),
}

impl Value {
    #[inline]
    fn eval(&self, p: &SplineInput) -> f32 {
        match self {
            Value::Const(v) => *v,
            Value::Nested(s) => s.eval(p),
        }
    }
}

impl From<f32> for Value {
    fn from(v: f32) -> Self {
        Value::Const(v)
    }
}

impl From<Spline> for Value {
    fn from(s: Spline) -> Self {
        Value::Nested(Box::new(s))
    }
}

#[derive(Clone, Debug)]
pub struct Spline {
    coord: Coord,
    locs: Vec<f32>,
    values: Vec<Value>,
    slopes: Vec<f32>,
}

impl Spline {
    pub fn new(coord: Coord) -> Self {
        Spline {
            coord,
            locs: Vec::new(),
            values: Vec::new(),
            slopes: Vec::new(),
        }
    }

    /// Add a control point (must be added in increasing `loc` order).
    pub fn point(mut self, loc: f32, value: impl Into<Value>, slope: f32) -> Self {
        debug_assert!(self.locs.last().is_none_or(|l| *l < loc));
        self.locs.push(loc);
        self.values.push(value.into());
        self.slopes.push(slope);
        self
    }

    /// Build a spline through constant points with slopes chosen
    /// automatically (Fritsch-Carlson style, so the curve never overshoots
    /// between monotone neighbours).
    pub fn smooth(coord: Coord, points: &[(f32, f32)]) -> Self {
        let n = points.len();
        let mut s = Spline::new(coord);
        for i in 0..n {
            let (x, y) = points[i];
            let slope = if n < 2 || i == 0 || i == n - 1 {
                0.0
            } else {
                let (x0, y0) = points[i - 1];
                let (x1, y1) = points[i + 1];
                let d0 = (y - y0) / (x - x0);
                let d1 = (y1 - y) / (x1 - x);
                if d0 * d1 <= 0.0 {
                    0.0
                } else {
                    // Harmonic mean keeps it monotone.
                    2.0 * d0 * d1 / (d0 + d1)
                }
            };
            s = s.point(x, y, slope);
        }
        s
    }

    pub fn eval(&self, p: &SplineInput) -> f32 {
        let x = p.get(self.coord);
        let n = self.locs.len();
        if n == 0 {
            return 0.0;
        }
        // Index of the first control point to the right of x.
        let i = self.locs.iter().position(|&l| l > x).unwrap_or(n);
        if i == 0 {
            return self.values[0].eval(p) + self.slopes[0] * (x - self.locs[0]);
        }
        if i == n {
            return self.values[n - 1].eval(p) + self.slopes[n - 1] * (x - self.locs[n - 1]);
        }
        let x0 = self.locs[i - 1];
        let x1 = self.locs[i];
        let h = x1 - x0;
        let t = (x - x0) / h;
        let y0 = self.values[i - 1].eval(p);
        let y1 = self.values[i].eval(p);
        let m0 = self.slopes[i - 1] * h;
        let m1 = self.slopes[i] * h;
        let t2 = t * t;
        let t3 = t2 * t;
        (2.0 * t3 - 3.0 * t2 + 1.0) * y0
            + (t3 - 2.0 * t2 + t) * m0
            + (-2.0 * t3 + 3.0 * t2) * y1
            + (t3 - t2) * m1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_control_points_and_nests() {
        let inner = Spline::smooth(Coord::Erosion, &[(-1.0, 10.0), (1.0, 20.0)]);
        let s = Spline::new(Coord::Continentalness)
            .point(0.0, 0.0, 0.0)
            .point(1.0, inner, 0.0);
        let p = |c, e| SplineInput { c, e, pv: 0.0 };
        assert!((s.eval(&p(0.0, 0.0)) - 0.0).abs() < 1e-5);
        assert!((s.eval(&p(1.0, -1.0)) - 10.0).abs() < 1e-5);
        assert!((s.eval(&p(1.0, 1.0)) - 20.0).abs() < 1e-5);
        let mid = s.eval(&p(0.5, 1.0));
        assert!(mid > 0.0 && mid < 20.0);
    }

    #[test]
    fn smooth_is_monotone() {
        let s = Spline::smooth(
            Coord::PeaksValleys,
            &[(-1.0, 0.0), (-0.2, 1.0), (0.3, 5.0), (1.0, 6.0)],
        );
        let mut prev = f32::MIN;
        for i in 0..=200 {
            let pv = -1.0 + i as f32 * 0.01;
            let v = s.eval(&SplineInput { c: 0.0, e: 0.0, pv });
            assert!(v >= prev - 1e-4, "not monotone at {pv}");
            prev = v;
        }
    }
}
