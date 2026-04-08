//! Workload generation for scheduling experiments.
//!
//! Implements UUniFast-Discard (Bini & Buttazzo, 2005) for
//! synthetic real-time task set generation.

pub mod dag_generator;
pub mod generator;
pub mod uunifast;

pub use dag_generator::{
    ErdosRenyiDagConfig, LayeredDagConfig, dag_from_json, dag_to_json, generate_erdos_renyi_dag,
    generate_layered_dag,
};
pub use generator::{WorkloadConfig, generate_taskset};
