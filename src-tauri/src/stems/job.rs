//! Serialising separation jobs.
//!
//! One inference peaks around 4.9 GB of RSS (measured — the model card's ~1.1 GB
//! claim did not hold). Two at once would mean ~10 GB, so exactly one job may run
//! app-wide. This mutex is the load-bearing memory mitigation: the clip-length cap
//! bounds the accumulation buffers but does NOT reduce the peak, because the model
//! chunk is a fixed size.

use std::sync::atomic::{AtomicBool, Ordering};

static RUNNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Longest clip we will separate. Bounds the accumulation buffers, not the peak.
pub const MAX_CLIP_MS: i64 = 60_000;

/// Held for the duration of a job; releases on drop even if the job panics.
pub struct JobGuard;

impl JobGuard {
    /// `None` when a job is already running.
    pub fn acquire() -> Option<JobGuard> {
        if RUNNING.swap(true, Ordering::SeqCst) {
            None
        } else {
            CANCEL.store(false, Ordering::SeqCst);
            Some(JobGuard)
        }
    }
}

impl Drop for JobGuard {
    fn drop(&mut self) {
        RUNNING.store(false, Ordering::SeqCst);
    }
}

pub fn request_cancel() {
    CANCEL.store(true, Ordering::SeqCst);
}

pub fn is_cancelled() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

pub fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_one_job_may_run_at_a_time() {
        let first = JobGuard::acquire().expect("first job should start");
        assert!(is_running());
        assert!(JobGuard::acquire().is_none(), "a second concurrent job must be refused");
        drop(first);
        assert!(!is_running(), "the guard must release on drop");
        let again = JobGuard::acquire().expect("a new job can start once the first finished");
        drop(again);
    }

    #[test]
    fn acquiring_clears_a_stale_cancel_flag() {
        // A cancelled job must not poison the next one.
        let g = JobGuard::acquire().unwrap();
        request_cancel();
        assert!(is_cancelled());
        drop(g);
        let g2 = JobGuard::acquire().unwrap();
        assert!(!is_cancelled(), "a fresh job must start uncancelled");
        drop(g2);
    }
}
