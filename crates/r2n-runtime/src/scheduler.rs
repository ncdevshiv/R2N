//! Scheduler — deterministic FIFO render queue with dedup (M0.2-T04).
//!
//! When a state setter marks a component instance dirty, the scheduler
//! enqueues that instance's path — but only if it isn't already queued
//! (dedup: N state updates inside one handler produce ONE re-render pass,
//! matching React's batched-update semantics).
//!
//! The queue is drained in FIFO order by `Runtime::flush`: each render pass
//! pops the next dirty instance and re-renders the whole tree (component
//! bodies are re-evaluated top-down, so a popped instance sees its updated
//! state). A bound prevents infinite update loops (same guard the old
//! dirty-flag loop had).
//!
//! Determinism: the queue is a `VecDeque` keyed by instance path; equal
//! sequences of setter calls always drain in the same order, so the emitted
//! patch stream is identical across runs (test-verified).

use std::collections::HashSet;
use std::collections::VecDeque;

/// FIFO queue of dirty component instance paths, with dedup.
#[derive(Debug, Default)]
pub struct Scheduler {
    queue: VecDeque<Vec<String>>,
    queued: HashSet<Vec<String>>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue a dirty instance path (no-op if already queued).
    pub fn schedule(&mut self, path: Vec<String>) {
        if self.queued.insert(path.clone()) {
            self.queue.push_back(path);
        }
    }

    /// Pop the next dirty instance path (FIFO), or `None` when drained.
    pub fn pop_front(&mut self) -> Option<Vec<String>> {
        let path = self.queue.pop_front()?;
        self.queued.remove(&path);
        Some(path)
    }

    /// Whether anything is queued.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Number of queued (deduped) instances.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Clear the queue (used by the render guard when aborting a runaway loop).
    pub fn clear(&mut self) {
        self.queue.clear();
        self.queued.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Vec<String> {
        vec![s.to_string()]
    }

    #[test]
    fn fifo_order() {
        let mut s = Scheduler::new();
        s.schedule(p("a"));
        s.schedule(p("b"));
        s.schedule(p("c"));
        assert_eq!(s.pop_front(), Some(p("a")));
        assert_eq!(s.pop_front(), Some(p("b")));
        assert_eq!(s.pop_front(), Some(p("c")));
        assert_eq!(s.pop_front(), None);
    }

    #[test]
    fn dedup_on_enqueue() {
        let mut s = Scheduler::new();
        s.schedule(p("a"));
        s.schedule(p("a"));
        s.schedule(p("a"));
        assert_eq!(s.len(), 1, "re-scheduling a queued instance is a no-op");
        s.schedule(p("b"));
        assert_eq!(s.len(), 2);
        assert_eq!(s.pop_front(), Some(p("a")));
    }

    #[test]
    fn requeue_after_pop() {
        // A popped instance that goes dirty AGAIN during its render must be
        // schedulable once more (dedup only applies while still queued).
        let mut s = Scheduler::new();
        s.schedule(p("a"));
        assert_eq!(s.pop_front(), Some(p("a")));
        s.schedule(p("a"));
        assert_eq!(s.len(), 1, "same instance can be re-enqueued after pop");
    }

    #[test]
    fn batched_updates_collapse() {
        // The core React-batching property: many setters on one instance in a
        // single handler → one queue entry → one render pass.
        let mut s = Scheduler::new();
        for _ in 0..10 {
            s.schedule(p("root"));
        }
        assert_eq!(s.len(), 1);
    }

    #[test]
    fn clear_empties() {
        let mut s = Scheduler::new();
        s.schedule(p("a"));
        s.schedule(p("b"));
        s.clear();
        assert!(s.is_empty());
        assert_eq!(s.pop_front(), None);
    }
}
