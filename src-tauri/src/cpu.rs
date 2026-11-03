//! Sharing the processor between the background analyses: smart search,
//! faces and pets run side by side, each on its own share.
//!
//! Faces get half: with a model to find faces and one to recognise each, a
//! library of people gives them the most to do. Measured on 4 cores with
//! 175 photos (`examples/library_bench`): 352 ms a photo for all three this
//! way, 386 ms with smart search on half; taking turns on all but one
//! processor each was slower still (493 ms), as one model gains little from
//! a second and third thread. Opening a photo meanwhile took 20–40 ms.

#[derive(Debug, Clone, Copy)]
pub enum Analysis {
    SmartSearch,
    Faces,
    Pets,
}

/// How many threads an analysis's models run on.
pub fn threads(analysis: Analysis) -> usize {
    let n = std::thread::available_parallelism().map_or(2, |n| n.get());
    match analysis {
        Analysis::Faces => (n / 2).max(1),
        Analysis::SmartSearch | Analysis::Pets => (n / 4).max(1),
    }
}
