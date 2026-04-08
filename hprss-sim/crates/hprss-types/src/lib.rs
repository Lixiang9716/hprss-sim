//! Core types for the HPRSS heterogeneous real-time scheduling simulator.
//!
//! This crate defines all shared data structures used across the simulator:
//! - Task and job models (periodic, sporadic, DAG)
//! - Device and platform types
//! - Event system with version-based invalidation
//! - Scheduler interface (trait)
//! - Simulation actions

pub mod dag;
pub mod device;
pub mod event;
pub mod job;
pub mod policy;
pub mod scheduler;
pub mod task;
pub mod time;

pub use dag::*;
pub use device::*;
pub use event::*;
pub use job::*;
pub use policy::*;
pub use scheduler::*;
pub use task::*;
pub use time::*;

/// Newtype IDs for type safety
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct TaskId(pub u32);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct JobId(pub u64);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct DeviceId(pub u32);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct BusId(pub u32);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct ChainId(pub u32);
