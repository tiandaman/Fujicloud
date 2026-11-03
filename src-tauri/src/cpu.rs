//! Sharing the processor between the background analyses: smart search,
//! faces and pets.
//!
//! On a PC with up to 8 processors they take turns, a photo (or a batch) at
//! a time, each using all but one processor, which is left for the window.
//! Side by side with a share each, the one with most to do (faces, in a
//! library of people) ran on a single processor long after the others had
//! finished and left theirs idle. Bigger PCs run them side by side.

use parking_lot::{Mutex, MutexGuard};

#[derive(Debug, Clone, Copy)]
pub enum Analysis {
    SmartSearch,
    Faces,
    Pets,
}

fn processors() -> usize {
    std::thread::available_parallelism().map_or(2, |n| n.get())
}

fn taking_turns() -> bool {
    processors() <= 8
}

/// How many threads an analysis's models run on.
pub fn threads(analysis: Analysis) -> usize {
    let n = processors();
    if taking_turns() {
        return (n - 1).clamp(1, 6);
    }
    match analysis {
        Analysis::SmartSearch => n / 2,
        Analysis::Faces | Analysis::Pets => (n / 4).max(1),
    }
}

static TURN: Mutex<()> = Mutex::new(());

/// Runs `work` in its turn: straight away on a bigger PC, otherwise once no
/// other analysis is working, handing over fairly when done.
pub fn in_turn<T>(work: impl FnOnce() -> T) -> T {
    if !taking_turns() {
        return work();
    }
    let turn = TURN.lock();
    let out = work();
    MutexGuard::unlock_fair(turn);
    out
}
