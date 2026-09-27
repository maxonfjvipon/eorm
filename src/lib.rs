//! The EO memory model on real memory.
//!
//! A small machine that runs hand-written EO programs on 4 KB blocks
//! taken from the operating system, exactly the way `MEMORY.md` says,
//! and counts what it costs.
#![deny(clippy::pedantic, missing_docs)]

pub mod frames;
pub mod pool;
pub mod region;
