//! Workload generation for scheduling experiments.
//!
//! Implements UUniFast-Discard (Bini & Buttazzo, 2005) for
//! synthetic real-time task set generation.

pub mod generator;
pub mod uunifast;

pub use generator::{WorkloadConfig, generate_taskset};
