//! DAG runtime tracking with edge-level dependency tokens.

use std::collections::HashMap;

use hprss_types::{
    DagInstanceId, DagProvenance, SubTaskIdx, TaskId,
    task::{ArrivalModel, DagTask, SubTask, Task},
};

#[derive(Debug, Clone)]
struct DagInstanceState {
    task: DagTask,
    node_to_task: HashMap<SubTaskIdx, TaskId>,
    task_to_node: HashMap<TaskId, SubTaskIdx>,
    unsatisfied_incoming: HashMap<SubTaskIdx, usize>,
    edge_satisfied: HashMap<(SubTaskIdx, SubTaskIdx), bool>,
    edge_bytes: HashMap<(SubTaskIdx, SubTaskIdx), u64>,
}

/// Registration result for a DAG instance.
#[derive(Debug, Clone)]
pub struct DagRegistration {
    pub instance_id: DagInstanceId,
    pub ready_task_ids: Vec<TaskId>,
}

/// Tracks DAG instances, node dependency state, and SubTask→Task proxy mapping.
#[derive(Debug, Default)]
pub struct DagTracker {
    next_instance_id: u64,
    instances: HashMap<DagInstanceId, DagInstanceState>,
}

impl DagTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one DAG instance and append proxy tasks to the global task registry.
    pub fn register_dag(
        &mut self,
        dag: DagTask,
        task_registry: &mut Vec<Task>,
    ) -> DagRegistration {
        let instance_id = DagInstanceId(self.next_instance_id);
        self.next_instance_id += 1;

        let mut node_to_task = HashMap::new();
        let mut task_to_node = HashMap::new();
        for node in &dag.nodes {
            let task_id = TaskId(task_registry.len() as u32);
            let proxy = subtask_to_proxy_task(&dag, node, task_id, instance_id);
            task_registry.push(proxy);
            node_to_task.insert(SubTaskIdx(node.index as u32), task_id);
            task_to_node.insert(task_id, SubTaskIdx(node.index as u32));
        }

        let mut unsatisfied_incoming = dag
            .nodes
            .iter()
            .map(|n| (SubTaskIdx(n.index as u32), 0usize))
            .collect::<HashMap<_, _>>();
        let mut edge_satisfied = HashMap::new();
        let mut edge_bytes = HashMap::new();
        for node in &dag.nodes {
            let to = SubTaskIdx(node.index as u32);
            for &(pred, bytes) in &node.data_deps {
                edge_bytes.insert((SubTaskIdx(pred as u32), to), bytes);
            }
        }
        for &(from, to) in &dag.edges {
            let from = SubTaskIdx(from as u32);
            let to = SubTaskIdx(to as u32);
            *unsatisfied_incoming.entry(to).or_default() += 1;
            edge_satisfied.insert((from, to), false);
            edge_bytes.entry((from, to)).or_insert(0);
        }

        let ready_task_ids = unsatisfied_incoming
            .iter()
            .filter_map(|(node, &need)| (need == 0).then_some(*node))
            .filter_map(|node| node_to_task.get(&node).copied())
            .collect();

        self.instances.insert(
            instance_id,
            DagInstanceState {
                task: dag,
                node_to_task,
                task_to_node,
                unsatisfied_incoming,
                edge_satisfied,
                edge_bytes,
            },
        );

        DagRegistration {
            instance_id,
            ready_task_ids,
        }
    }

    pub fn dag_provenance(&self, instance_id: DagInstanceId, task_id: TaskId) -> Option<DagProvenance> {
        let state = self.instances.get(&instance_id)?;
        let node = state.task_to_node.get(&task_id)?;
        Some(DagProvenance {
            dag_instance_id: instance_id,
            node: *node,
        })
    }

    pub fn provenance_for_task(&self, task_id: TaskId) -> Option<DagProvenance> {
        self.instances.iter().find_map(|(instance_id, state)| {
            state.task_to_node.get(&task_id).map(|node| DagProvenance {
                dag_instance_id: *instance_id,
                node: *node,
            })
        })
    }

    pub fn proxy_task_id(&self, instance_id: DagInstanceId, node: SubTaskIdx) -> Option<TaskId> {
        self.instances.get(&instance_id)?.node_to_task.get(&node).copied()
    }

    /// Mark one edge token satisfied; returns successor proxy task IDs newly released.
    pub fn mark_edge_satisfied(
        &mut self,
        instance_id: DagInstanceId,
        from: SubTaskIdx,
        to: SubTaskIdx,
    ) -> Vec<TaskId> {
        let Some(state) = self.instances.get_mut(&instance_id) else {
            return Vec::new();
        };

        let Some(satisfied) = state.edge_satisfied.get_mut(&(from, to)) else {
            return Vec::new();
        };
        if *satisfied {
            return Vec::new();
        }
        *satisfied = true;

        let Some(remaining) = state.unsatisfied_incoming.get_mut(&to) else {
            return Vec::new();
        };
        if *remaining > 0 {
            *remaining -= 1;
        }
        if *remaining == 0
            && let Some(task_id) = state.node_to_task.get(&to).copied()
        {
            return vec![task_id];
        }
        Vec::new()
    }

    pub fn edges(&self, instance_id: DagInstanceId) -> Option<&[(usize, usize)]> {
        self.instances.get(&instance_id).map(|s| s.task.edges.as_slice())
    }

    pub fn outgoing_edges(
        &self,
        instance_id: DagInstanceId,
        from: SubTaskIdx,
    ) -> Vec<(SubTaskIdx, u64)> {
        let Some(state) = self.instances.get(&instance_id) else {
            return Vec::new();
        };
        state
            .edge_bytes
            .iter()
            .filter_map(|(&(src, dst), &bytes)| (src == from).then_some((dst, bytes)))
            .collect()
    }
}

fn subtask_to_proxy_task(
    dag: &DagTask,
    node: &SubTask,
    task_id: TaskId,
    instance_id: DagInstanceId,
) -> Task {
    Task {
        id: task_id,
        name: format!("dag-{}-n{}", instance_id.0, node.index),
        priority: 1,
        arrival: ArrivalModel::Aperiodic,
        deadline: dag.deadline,
        criticality: dag.criticality,
        exec_times: node.exec_times.clone(),
        affinity: node.affinity.clone(),
        data_size: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::task::{CriticalityLevel, DeviceType, ExecutionTimeModel};

    fn sample_dag() -> DagTask {
        DagTask {
            id: TaskId(7),
            name: "dag".into(),
            arrival: ArrivalModel::Aperiodic,
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            nodes: vec![
                SubTask {
                    index: 0,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 10_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![],
                },
                SubTask {
                    index: 1,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 20_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![(0, 128)],
                },
                SubTask {
                    index: 2,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 20_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![(0, 128)],
                },
                SubTask {
                    index: 3,
                    exec_times: vec![(
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic { wcet: 30_000 },
                    )],
                    affinity: vec![DeviceType::Cpu],
                    data_deps: vec![(1, 256), (2, 256)],
                },
            ],
            edges: vec![(0, 1), (0, 2), (1, 3), (2, 3)],
        }
    }

    #[test]
    fn dag_registration_releases_zero_indegree_nodes() {
        let mut tracker = DagTracker::new();
        let mut tasks = Vec::new();
        let reg = tracker.register_dag(sample_dag(), &mut tasks);
        assert_eq!(reg.ready_task_ids.len(), 1);
        let node0 = tracker
            .proxy_task_id(reg.instance_id, SubTaskIdx(0))
            .expect("node 0 should have proxy task");
        assert_eq!(reg.ready_task_ids, vec![node0]);
    }

    #[test]
    fn fan_in_node_released_only_after_all_incoming_edges_complete() {
        let mut tracker = DagTracker::new();
        let mut tasks = Vec::new();
        let reg = tracker.register_dag(sample_dag(), &mut tasks);
        let node3_task = tracker
            .proxy_task_id(reg.instance_id, SubTaskIdx(3))
            .expect("node 3 should have proxy task");

        let released = tracker.mark_edge_satisfied(reg.instance_id, SubTaskIdx(1), SubTaskIdx(3));
        assert!(released.is_empty(), "fan-in node must stay blocked");
        let released = tracker.mark_edge_satisfied(reg.instance_id, SubTaskIdx(2), SubTaskIdx(3));
        assert_eq!(released, vec![node3_task]);
    }
}
