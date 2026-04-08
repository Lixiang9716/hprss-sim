//! Workload generation for scheduling experiments.
//!
//! Implements UUniFast-Discard (Bini & Buttazzo, 2005) for
//! synthetic real-time task set generation.

pub mod dag_generator;
pub mod generator;
pub mod replay;
pub mod uunifast;

pub use dag_generator::{
    ErdosRenyiDagConfig, LayeredDagConfig, dag_from_json, dag_to_json, generate_erdos_renyi_dag,
    generate_layered_dag,
};
pub use generator::{WorkloadConfig, generate_taskset};
pub use replay::{
    ReplayJobSpec, ReplayTaskSpec, ReplayWorkload, ReplayWorkloadError, load_replay_csv,
    load_replay_json,
};
