//! Workload generation for scheduling experiments.
//!
//! Implements UUniFast-Discard (Bini & Buttazzo, 2005) for
//! synthetic real-time task set generation.

pub mod dag_generator;
pub mod generator;
pub mod karami_profile_adapter;
pub mod openmp_adapter;
pub mod replay;
pub mod uunifast;

pub use dag_generator::{
    ErdosRenyiDagConfig, LayeredDagConfig, dag_from_json, dag_to_json, generate_erdos_renyi_dag,
    generate_layered_dag,
};
pub use generator::{WorkloadConfig, generate_taskset};
pub use karami_profile_adapter::{
    KaramiAssumptionSpec, KaramiPaperProfileWorkload, KaramiPaperProfileWorkloadError,
    KaramiScenarioSpec, adapt_karami_paper_profile_json, adapt_karami_paper_profile_json_str,
};
pub use openmp_adapter::{
    OpenMpSpecializedWorkload, OpenMpSpecializedWorkloadError, adapt_openmp_specialized_json,
    adapt_openmp_specialized_json_str,
};
pub use replay::{
    ReplayAssumption, ReplayExecHint, ReplayJobSpec, ReplayMetadata, ReplayTaskSpec,
    ReplayWorkload, ReplayWorkloadError, load_replay_csv, load_replay_json,
};
