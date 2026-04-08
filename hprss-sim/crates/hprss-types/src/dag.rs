//! DAG-related runtime identifiers.

use serde::{Deserialize, Serialize};

/// Runtime DAG instance identifier.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct DagInstanceId(pub u64);

/// Sub-task index within a DAG.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct SubTaskIdx(pub u32);

/// Provenance carried by a job that originates from a DAG node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DagProvenance {
    pub dag_instance_id: DagInstanceId,
    pub node: SubTaskIdx,
}

/// Token identifying data transfer on a DAG edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EdgeTransferId {
    pub dag_instance_id: DagInstanceId,
    pub from_node: SubTaskIdx,
    pub to_node: SubTaskIdx,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dag_identifiers_are_comparable() {
        assert!(DagInstanceId(1) < DagInstanceId(2));
        assert!(SubTaskIdx(3) > SubTaskIdx(1));
    }
}
