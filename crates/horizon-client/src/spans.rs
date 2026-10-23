//! Named timings within a frame, for finding what makes a frame slow:
//! `HORIZON_SPIKES=MS` prints the spans of every frame whose CPU time
//! exceeds MS milliseconds, and of every server tick (dedicated or
//! integrated) longer than MS. Each span keeps its wall time and the
//! thread's CPU time inside it, so a span that waited (for a lock, the GPU
//! or a core on a busy machine) shows apart from one that worked. Costs two
//! clock reads per span when enabled and a relaxed load otherwise.
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

static ENABLED: AtomicBool = AtomicBool::new(false);
thread_local! {
    static SPANS: RefCell<Vec<Span>> = const { RefCell::new(Vec::new()) };
}

/// A named timing: wall milliseconds and the thread's CPU milliseconds in it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub name: &'static str,
    pub ms: f32,
    pub cpu_ms: f32,
}

pub fn enable(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}
/// Run `f`, recording its duration under `name` when spans are enabled.
#[inline]
pub fn time<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    if !ENABLED.load(Ordering::Relaxed) {
        return f();
    }
    let (started, cpu) = (Instant::now(), thread_cpu());
    let out = f();
    let ms = started.elapsed().as_secs_f32() * 1000.0;
    let cpu_ms = thread_cpu()
        .zip(cpu)
        .map_or(ms, |(end, start)| (end - start).as_secs_f32() * 1000.0);
    SPANS.with(|s| s.borrow_mut().push(Span { name, ms, cpu_ms }));
    out
}
/// A non-borrowing named timing scope. Unlike wrapping a large block in a
/// closure, it can be dropped at a chosen point without changing local
/// variable lifetimes, `?` propagation or frame-render execution order.
/// Disabled scopes make no clock reads and do not allocate.
#[must_use = "keep the scope alive until the operation ends"]
pub struct Scope {
    name: &'static str,
    started: Option<Instant>,
    started_cpu: Option<Duration>,
}

#[inline]
pub fn scope(name: &'static str) -> Scope {
    let started = ENABLED.load(Ordering::Relaxed).then(Instant::now);
    Scope {
        name,
        started,
        started_cpu: started.and_then(|_| thread_cpu()),
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if let Some(started) = self.started {
            let ms = started.elapsed().as_secs_f32() * 1000.0;
            let cpu_ms = thread_cpu()
                .zip(self.started_cpu)
                .map_or(ms, |(end, start)| (end - start).as_secs_f32() * 1000.0);
            SPANS.with(|spans| {
                spans.borrow_mut().push(Span {
                    name: self.name,
                    ms,
                    cpu_ms,
                })
            });
        }
    }
}

/// The CPU time this thread has used, where the platform tells it: a tick
/// with more wall time than CPU time waited (for a lock, or for a core on a
/// busy machine).
pub fn thread_cpu() -> Option<Duration> {
    #[cfg(unix)]
    {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        Some(Duration::new(t.tv_sec as u64, t.tv_nsec as u32))
    }
    #[cfg(not(unix))]
    None
}
/// The frame's spans, summed by name, slowest first; clears them.
pub fn take() -> Vec<Span> {
    let spans = SPANS.with(|s| std::mem::take(&mut *s.borrow_mut()));
    let mut sums: Vec<Span> = Vec::new();
    for span in spans {
        match sums.iter_mut().find(|s| s.name == span.name) {
            Some(sum) => {
                sum.ms += span.ms;
                sum.cpu_ms += span.cpu_ms;
            }
            None => sums.push(span),
        }
    }
    sums.sort_by(|a, b| b.ms.total_cmp(&a.ms));
    sums
}
/// `name wall` for spans of at least 0.5 ms, with `/cpu` when the thread
/// worked for less than nine tenths of it: the slow-frame and slow-tick lines.
pub fn describe(spans: &[Span]) -> String {
    let list: Vec<String> = spans
        .iter()
        .filter(|s| s.ms >= 0.5)
        .map(|s| {
            if s.cpu_ms < s.ms * 0.9 {
                format!("{} {:.1}/{:.1}", s.name, s.ms, s.cpu_ms)
            } else {
                format!("{} {:.1}", s.name, s.ms)
            }
        })
        .collect();
    list.join(", ")
}

/// HORIZON_SPIKES=0: include every span, including submillisecond draw-list
/// preparation costs. Keep upstream's optional wall/CPU attribution.
pub fn describe_all(spans: &[Span]) -> String {
    spans
        .iter()
        .map(|s| {
            if s.cpu_ms < s.ms * 0.9 {
                format!("{} {:.3}/{:.3}", s.name, s.ms, s.cpu_ms)
            } else {
                format!("{} {:.3}", s.name, s.ms)
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::Span;

    #[test]
    fn detailed_log_preserves_submillisecond_measurements() {
        let spans = [
            Span {
                name: "draw_list",
                ms: 0.0034,
                cpu_ms: 0.0034,
            },
            Span {
                name: "tick",
                ms: 3.4,
                cpu_ms: 3.4,
            },
        ];
        assert_eq!(super::describe(&spans), "tick 3.4");
        assert_eq!(super::describe_all(&spans), "draw_list 0.003, tick 3.400");
    }

    #[test]
    fn scope_records_on_drop_in_its_thread() {
        let _ = super::take();
        let span = super::Scope {
            name: "scope_regression",
            started: Some(std::time::Instant::now()),
            started_cpu: super::thread_cpu(),
        };
        assert!(super::take().is_empty());
        drop(span);
        let values = super::take();
        assert_eq!(values.len(), 1);
        assert_eq!(values[0].name, "scope_regression");
        assert!(values[0].ms >= 0.0);
        assert!(values[0].cpu_ms >= 0.0);
    }

    /// Upstream CPU/wall summation remains exact across repeated scopes.
    #[test]
    fn spans_sum_by_name_and_show_waits() {
        super::SPANS.with(|s| {
            s.borrow_mut().extend([
                Span {
                    name: "tick",
                    ms: 2.0,
                    cpu_ms: 2.0,
                },
                Span {
                    name: "submit",
                    ms: 30.0,
                    cpu_ms: 0.2,
                },
                Span {
                    name: "tick",
                    ms: 3.0,
                    cpu_ms: 2.9,
                },
                Span {
                    name: "hud",
                    ms: 0.3,
                    cpu_ms: 0.3,
                },
            ]);
        });
        let spans = super::take();
        assert_eq!(
            spans[0],
            Span {
                name: "submit",
                ms: 30.0,
                cpu_ms: 0.2
            }
        );
        assert_eq!((spans[1].name, spans[1].ms), ("tick", 5.0));
        assert_eq!(super::describe(&spans), "submit 30.0/0.2, tick 5.0");
        assert!(super::take().is_empty());
    }

    /// The thread's CPU clock moves with work done on it, not with time
    /// spent asleep.
    #[cfg(unix)]
    #[test]
    fn thread_cpu_counts_work_not_sleep() {
        let start = super::thread_cpu().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let slept = super::thread_cpu().unwrap() - start;
        // Busy until the clock shows 10 ms of work (a loaded machine gives
        // the thread less than its wall time).
        let mut x = 0u64;
        let busy = std::time::Instant::now();
        let worked = loop {
            x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(7));
            let worked = super::thread_cpu().unwrap() - start - slept;
            if worked.as_millis() >= 10 || busy.elapsed().as_secs() >= 10 {
                break worked;
            }
        };
        assert!(slept.as_millis() < 25, "{slept:?} while asleep");
        assert!(worked.as_millis() >= 10, "{worked:?} while busy");
    }
}
