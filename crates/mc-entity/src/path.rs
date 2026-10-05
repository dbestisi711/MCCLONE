//! A* pathfinding over the block grid.
//!
//! A node is the block position of a mob's feet. A node is *standable* when
//! the mob's column (`height` blocks) has no collision boxes and the block
//! below has one (or the feet are in water, for swimmers). Moves go to the 8
//! horizontal neighbours on the same level (diagonals only without corner
//! cutting), up one block (jump) or down at most `max_drop` blocks. Lava and
//! cactus are never entered or stood on; standing next to them is expensive.
//! Unloaded chunks are impassable. The search has a node budget; when it runs
//! out (or the goal is unreachable) the path to the closest node found is
//! returned with `reached == false`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use glam::IVec3;
use mc_core::{World, blocks};
use rustc_hash::FxHashMap;

use crate::physics::block_shape;

#[derive(Clone, Copy, Debug)]
pub struct PathConfig {
    /// Clearance the mob needs, in whole blocks (ceil of its height).
    pub height: i32,
    /// Largest drop the mob will take deliberately.
    pub max_drop: i32,
    /// Maximum number of nodes expanded before giving up.
    pub max_nodes: usize,
    /// Never path through water (passive wandering).
    pub avoid_water: bool,
    /// Accept nodes within this Chebyshev distance of the goal.
    pub tolerance: i32,
}

impl Default for PathConfig {
    fn default() -> Self {
        PathConfig {
            height: 2,
            max_drop: 3,
            max_nodes: 500,
            avoid_water: false,
            tolerance: 0,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Path {
    /// Feet block positions to walk through, excluding the start.
    pub nodes: Vec<IVec3>,
    /// The last node satisfies the goal (otherwise it is the closest found).
    pub reached: bool,
}

impl Path {
    pub fn goal(&self) -> Option<IVec3> {
        self.nodes.last().copied()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Cell {
    Open,
    Water,
    Solid,
    Danger,
}

fn cell(world: &World, p: IVec3) -> Cell {
    match world.block_loaded(p) {
        None => Cell::Solid,
        Some(id) if id == blocks::LAVA || id == blocks::CACTUS => Cell::Danger,
        Some(id) if block_shape(id).is_some() => Cell::Solid,
        Some(id) if id == blocks::WATER => Cell::Water,
        Some(_) => Cell::Open,
    }
}

fn passable(c: Cell) -> bool {
    matches!(c, Cell::Open | Cell::Water)
}

/// The mob's whole column at `p` is free of collision boxes and hazards.
fn clear(world: &World, p: IVec3, cfg: &PathConfig) -> bool {
    (0..cfg.height).all(|i| passable(cell(world, p + IVec3::Y * i)))
}

/// Extra cost of standing at `p`, or `None` if the mob cannot stand there.
fn standable(world: &World, p: IVec3, cfg: &PathConfig) -> Option<f32> {
    if !clear(world, p, cfg) {
        return None;
    }
    let feet = cell(world, p);
    let below = cell(world, p - IVec3::Y);
    let mut cost = 0.0;
    match (feet, below) {
        (Cell::Water, _) => {
            if cfg.avoid_water {
                return None;
            }
            cost += 2.0;
        }
        (_, Cell::Solid) => {}
        _ => return None,
    }
    for d in [IVec3::X, IVec3::NEG_X, IVec3::Z, IVec3::NEG_Z] {
        if cell(world, p + d) == Cell::Danger || cell(world, p + d - IVec3::Y) == Cell::Danger {
            cost += 8.0;
            break;
        }
    }
    Some(cost)
}

/// Extra path cost of standing at `p`, or `None` if a mob with this config
/// cannot stand there.
pub fn standable_cost(world: &World, p: IVec3, cfg: &PathConfig) -> Option<f32> {
    standable(world, p, cfg)
}

const DIRS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

fn neighbours(world: &World, p: IVec3, cfg: &PathConfig, out: &mut Vec<(IVec3, f32)>) {
    out.clear();
    for (dx, dz) in DIRS {
        let t = p + IVec3::new(dx, 0, dz);
        if dx != 0 && dz != 0 {
            // Diagonal: only on the same level and without cutting corners.
            if standable(world, p + IVec3::new(dx, 0, 0), cfg).is_none()
                || standable(world, p + IVec3::new(0, 0, dz), cfg).is_none()
            {
                continue;
            }
            if let Some(c) = standable(world, t, cfg) {
                out.push((t, std::f32::consts::SQRT_2 + c));
            }
            continue;
        }
        if let Some(c) = standable(world, t, cfg) {
            out.push((t, 1.0 + c));
            continue;
        }
        if !clear(world, t, cfg) {
            // Jump up one block: needs headroom above the current node.
            let up = t + IVec3::Y;
            if passable(cell(world, p + IVec3::Y * cfg.height))
                && let Some(c) = standable(world, up, cfg)
            {
                out.push((up, 1.5 + c));
            }
            continue;
        }
        // Walk off the edge and drop.
        for k in 1..=cfg.max_drop {
            let d = t - IVec3::Y * k;
            if let Some(c) = standable(world, d, cfg) {
                out.push((d, 1.0 + 0.5 * k as f32 + c));
                break;
            }
            if !passable(cell(world, d)) {
                break;
            }
        }
    }
    // Swimming straight up or down.
    if cell(world, p) == Cell::Water {
        for d in [IVec3::Y, IVec3::NEG_Y] {
            if let Some(c) = standable(world, p + d, cfg) {
                out.push((p + d, 1.0 + c));
            }
        }
    }
}

fn heuristic(a: IVec3, b: IVec3) -> f32 {
    let dx = (a.x - b.x).abs() as f32;
    let dz = (a.z - b.z).abs() as f32;
    let dy = (a.y - b.y).abs() as f32;
    let (lo, hi) = if dx < dz { (dx, dz) } else { (dz, dx) };
    hi + (std::f32::consts::SQRT_2 - 1.0) * lo + dy
}

fn is_goal(p: IVec3, goal: IVec3, tol: i32) -> bool {
    let d = (p - goal).abs();
    d.x.max(d.z) <= tol && d.y <= tol.max(0)
}

struct NodeInfo {
    g: f32,
    parent: Option<IVec3>,
    closed: bool,
}

/// Find a path for a mob standing at `start` (feet block) toward `goal`.
/// Returns `None` if no step at all could be made toward the goal.
pub fn find_path(world: &World, start: IVec3, goal: IVec3, cfg: &PathConfig) -> Option<Path> {
    if is_goal(start, goal, cfg.tolerance) {
        return Some(Path {
            nodes: Vec::new(),
            reached: true,
        });
    }
    let mut nodes: FxHashMap<IVec3, NodeInfo> = FxHashMap::default();
    // (f, insertion order) so ties resolve deterministically.
    let mut open: BinaryHeap<Reverse<(u32, u32, [i32; 3])>> = BinaryHeap::new();
    let mut seq = 0u32;
    nodes.insert(
        start,
        NodeInfo {
            g: 0.0,
            parent: None,
            closed: false,
        },
    );
    open.push(Reverse((
        heuristic(start, goal).to_bits(),
        seq,
        start.to_array(),
    )));
    let mut best = start;
    let mut best_h = heuristic(start, goal);
    let mut expanded = 0usize;
    let mut nbrs = Vec::with_capacity(12);
    let mut found = None;

    while let Some(Reverse((_, _, pa))) = open.pop() {
        let p = IVec3::from_array(pa);
        let g = {
            let n = nodes.get_mut(&p).expect("queued node exists");
            if n.closed {
                continue;
            }
            n.closed = true;
            n.g
        };
        if is_goal(p, goal, cfg.tolerance) {
            found = Some(p);
            break;
        }
        let h = heuristic(p, goal);
        if h < best_h {
            best_h = h;
            best = p;
        }
        expanded += 1;
        if expanded > cfg.max_nodes {
            break;
        }
        neighbours(world, p, cfg, &mut nbrs);
        for &(q, cost) in &nbrs {
            let ng = g + cost;
            let better = match nodes.get(&q) {
                Some(n) => !n.closed && ng < n.g,
                None => true,
            };
            if better {
                nodes.insert(
                    q,
                    NodeInfo {
                        g: ng,
                        parent: Some(p),
                        closed: false,
                    },
                );
                seq = seq.wrapping_add(1);
                open.push(Reverse((
                    (ng + heuristic(q, goal)).to_bits(),
                    seq,
                    q.to_array(),
                )));
            }
        }
    }

    let (end, reached) = match found {
        Some(p) => (p, true),
        None if best != start => (best, false),
        None => return None,
    };
    let mut path = vec![end];
    let mut cur = end;
    while let Some(parent) = nodes.get(&cur).and_then(|n| n.parent) {
        if parent == start {
            break;
        }
        path.push(parent);
        cur = parent;
    }
    path.reverse();
    Some(Path {
        nodes: path,
        reached,
    })
}
