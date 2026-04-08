//! Discrete Event Simulation engine.
//!
//! The core simulation loop: processes events from a priority queue,
//! dispatches to device simulators and scheduler, collects metrics.

pub mod device_manager;
pub mod engine;
pub mod transfer_manager;

pub use engine::SimEngine;
