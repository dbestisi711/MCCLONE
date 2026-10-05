//! Background work: the only module in the renderer that knows about threads.
//!
//! Everything that runs off the main thread (world generation, lighting,
//! chunk meshing) goes through [`TaskPool`] / [`Jobs`], so the web build can
//! swap this file's internals for a single-threaded version without touching
//! the callers:
//!
//! - native: a small fixed pool of worker threads pulling closures from three
//!   priority queues (mesh rebuilds for edits jump ahead of world generation);
//! - `wasm32`: jobs run inline when submitted and `par_map` is sequential.
//!   Callers already bound how many jobs they submit per frame, so the frame
//!   cost stays bounded.

use crossbeam_channel::{Receiver, Sender, unbounded};

#[cfg(not(target_arch = "wasm32"))]
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex, OnceLock},
};

/// Scheduling class of a job. Lower runs first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Priority {
    /// Latency sensitive (remeshing after a block edit, nearby sections).
    High = 0,
    /// Regular streaming work (meshing, lighting).
    Normal = 1,
    /// Bulk work (world generation).
    Low = 2,
}

type Job = Box<dyn FnOnce() + Send + 'static>;

#[cfg(not(target_arch = "wasm32"))]
struct Shared {
    queues: Mutex<[VecDeque<Job>; 3]>,
    cv: Condvar,
}

/// Process-wide worker pool.
pub struct TaskPool {
    #[cfg(not(target_arch = "wasm32"))]
    shared: Arc<Shared>,
    threads: usize,
}

#[cfg(not(target_arch = "wasm32"))]
static POOL: OnceLock<TaskPool> = OnceLock::new();

/// The shared pool (created on first use).
#[cfg(not(target_arch = "wasm32"))]
pub fn pool() -> &'static TaskPool {
    POOL.get_or_init(TaskPool::new)
}

#[cfg(target_arch = "wasm32")]
pub fn pool() -> &'static TaskPool {
    static P: TaskPool = TaskPool { threads: 1 };
    &P
}

/// Logical CPU count (at least 1).
pub fn cpu_count() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
    #[cfg(target_arch = "wasm32")]
    {
        1
    }
}

impl TaskPool {
    #[cfg(not(target_arch = "wasm32"))]
    fn new() -> Self {
        // Leave one core for the main (render) thread.
        let threads = cpu_count().saturating_sub(1).max(1);
        let shared = Arc::new(Shared {
            queues: Mutex::new([VecDeque::new(), VecDeque::new(), VecDeque::new()]),
            cv: Condvar::new(),
        });
        for i in 0..threads {
            let shared = shared.clone();
            std::thread::Builder::new()
                .name(format!("mc-worker-{i}"))
                .spawn(move || worker_loop(&shared))
                .expect("spawn worker thread");
        }
        TaskPool { shared, threads }
    }

    /// Number of background workers.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Run `f` in the background.
    pub fn spawn(&self, priority: Priority, f: impl FnOnce() + Send + 'static) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut q = self.shared.queues.lock().unwrap();
            q[priority as usize].push_back(Box::new(f));
            drop(q);
            self.shared.cv.notify_one();
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = priority;
            f();
        }
    }

    /// Apply `f` to every item in parallel and wait for all results (in input
    /// order). For blocking phases (startup, screenshots); the caller's thread
    /// takes part in the work. `f` may borrow from the caller.
    pub fn par_map<T, R, F>(&self, items: Vec<T>, f: F) -> Vec<R>
    where
        T: Send,
        R: Send,
        F: Fn(T) -> R + Sync,
    {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::sync::atomic::{AtomicUsize, Ordering};
            let n = items.len();
            if n <= 1 {
                return items.into_iter().map(f).collect();
            }
            let slots: Vec<Mutex<Option<T>>> =
                items.into_iter().map(|t| Mutex::new(Some(t))).collect();
            let results: Vec<Mutex<Option<R>>> = (0..n).map(|_| Mutex::new(None)).collect();
            let next = AtomicUsize::new(0);
            let work = || {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n {
                        break;
                    }
                    let item = slots[i].lock().unwrap().take().unwrap();
                    let r = f(item);
                    *results[i].lock().unwrap() = Some(r);
                }
            };
            let helpers = cpu_count().min(n).saturating_sub(1);
            std::thread::scope(|s| {
                for _ in 0..helpers {
                    s.spawn(work);
                }
                work();
            });
            results
                .into_iter()
                .map(|m| m.into_inner().unwrap().unwrap())
                .collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            items.into_iter().map(f).collect()
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn worker_loop(shared: &Shared) {
    loop {
        let job = {
            let mut q = shared.queues.lock().unwrap();
            loop {
                if let Some(j) = q.iter_mut().find_map(|d| d.pop_front()) {
                    break j;
                }
                q = shared.cv.wait(q).unwrap();
            }
        };
        // A panicking job must not take the worker down with it.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
    }
}

/// A set of background jobs producing results of type `R`, with an in-flight
/// counter so callers can bound how much work they queue.
pub struct Jobs<R> {
    tx: Sender<R>,
    rx: Receiver<R>,
    in_flight: usize,
}

impl<R: Send + 'static> Default for Jobs<R> {
    fn default() -> Self {
        Self::new()
    }
}

impl<R: Send + 'static> Jobs<R> {
    pub fn new() -> Self {
        let (tx, rx) = unbounded();
        Jobs {
            tx,
            rx,
            in_flight: 0,
        }
    }

    pub fn submit(&mut self, priority: Priority, job: impl FnOnce() -> R + Send + 'static) {
        self.in_flight += 1;
        let tx = self.tx.clone();
        pool().spawn(priority, move || {
            let _ = tx.send(job());
        });
    }

    /// A finished result, if any (never blocks).
    pub fn try_recv(&mut self) -> Option<R> {
        let r = self.rx.try_recv().ok();
        if r.is_some() {
            self.in_flight -= 1;
        }
        r
    }

    /// Wait for the next result; `None` when nothing is in flight.
    pub fn recv_blocking(&mut self) -> Option<R> {
        if self.in_flight == 0 {
            return None;
        }
        let r = self.rx.recv().ok();
        if r.is_some() {
            self.in_flight -= 1;
        }
        r
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn par_map_keeps_order() {
        let v: Vec<u32> = (0..100).collect();
        let base = 7u32;
        let out = pool().par_map(v, |x| x * 2 + base);
        assert_eq!(out, (0..100).map(|x| x * 2 + 7).collect::<Vec<_>>());
    }

    #[test]
    fn jobs_complete() {
        let mut jobs = Jobs::new();
        for i in 0..20u32 {
            jobs.submit(Priority::Normal, move || i * i);
        }
        let mut sum = 0;
        while let Some(r) = jobs.recv_blocking() {
            sum += r;
        }
        assert_eq!(sum, (0..20u32).map(|i| i * i).sum::<u32>());
        assert_eq!(jobs.in_flight(), 0);
    }
}
