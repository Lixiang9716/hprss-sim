//! Discrete Event Simulation engine.
//!
//! The core simulation loop: processes events from a priority queue,
//! dispatches to device simulators and scheduler, collects metrics.

pub mod engine;

pub use engine::SimEngine;
