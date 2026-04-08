//! DAG workload generation utilities.

use hprss_types::{
    TaskId,
    task::{ArrivalModel, CriticalityLevel, DagTask, DeviceType, ExecutionTimeModel, SubTask},
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

#[derive(Debug, Clone)]
pub struct ErdosRenyiDagConfig {
    pub task_id: u32,
    pub node_count: usize,
    pub edge_probability: f64,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub struct LayeredDagConfig {
    pub task_id: u32,
    pub layer_widths: Vec<usize>,
    pub edge_probability: f64,
    pub seed: u64,
}

pub fn generate_erdos_renyi_dag(config: &ErdosRenyiDagConfig) -> DagTask {
    let node_count = config.node_count.max(1);
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let mut edges = Vec::new();
    let p = config.edge_probability.clamp(0.0, 1.0);

    for from in 0..node_count {
        for to in (from + 1)..node_count {
            if rng.gen_bool(p) {
                edges.push((from, to));
            }
        }
    }

    let nodes = build_nodes(node_count, &edges, &mut rng);
    DagTask {
        id: TaskId(config.task_id),
        name: format!("dag-er-{}", config.task_id),
        arrival: ArrivalModel::Periodic { period: 1_000_000 },
        deadline: (node_count as u64).saturating_mul(200_000),
        criticality: CriticalityLevel::Lo,
        nodes,
        edges,
    }
}

pub fn generate_layered_dag(config: &LayeredDagConfig) -> DagTask {
    let widths = if config.layer_widths.is_empty() {
        vec![1usize]
    } else {
        config.layer_widths.clone()
    };
    let mut rng = ChaCha8Rng::seed_from_u64(config.seed);
    let p = config.edge_probability.clamp(0.0, 1.0);

    let mut layer_nodes = Vec::with_capacity(widths.len());
    let mut next_index = 0usize;
    for width in widths {
        let mut layer = Vec::with_capacity(width);
        for _ in 0..width {
            layer.push(next_index);
            next_index += 1;
        }
        layer_nodes.push(layer);
    }

    let mut edges = Vec::new();
    for pair in layer_nodes.windows(2) {
        let current = &pair[0];
        let next = &pair[1];
        for &from in current {
            for &to in next {
                if rng.gen_bool(p) {
                    edges.push((from, to));
                }
            }
        }
    }

    let node_count = next_index.max(1);
    let nodes = build_nodes(node_count, &edges, &mut rng);
    DagTask {
        id: TaskId(config.task_id),
        name: format!("dag-layered-{}", config.task_id),
        arrival: ArrivalModel::Periodic { period: 1_000_000 },
        deadline: (node_count as u64).saturating_mul(200_000),
        criticality: CriticalityLevel::Lo,
        nodes,
        edges,
    }
}

pub fn dag_to_json(dag: &DagTask) -> String {
    serde_json::to_string_pretty(dag).expect("dag serialization should succeed")
}

pub fn dag_from_json(json: &str) -> Result<DagTask, serde_json::Error> {
    serde_json::from_str(json)
}

fn build_nodes(node_count: usize, edges: &[(usize, usize)], rng: &mut ChaCha8Rng) -> Vec<SubTask> {
    let mut incoming: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    for &(from, to) in edges {
        incoming[to].push(from);
    }

    (0..node_count)
        .map(|idx| {
            let data_deps = incoming[idx]
                .iter()
                .map(|&pred| (pred, rng.gen_range(256..=8192)))
                .collect();
            SubTask {
                index: idx,
                exec_times: vec![
                    (
                        DeviceType::Cpu,
                        ExecutionTimeModel::Deterministic {
                            wcet: rng.gen_range(10_000..=60_000),
                        },
                    ),
                    (
                        DeviceType::Gpu,
                        ExecutionTimeModel::Deterministic {
                            wcet: rng.gen_range(4_000..=24_000),
                        },
                    ),
                ],
                affinity: vec![DeviceType::Cpu, DeviceType::Gpu],
                data_deps,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn is_acyclic(node_count: usize, edges: &[(usize, usize)]) -> bool {
        let mut indegree = vec![0usize; node_count];
        let mut adj = vec![Vec::new(); node_count];
        for &(u, v) in edges {
            indegree[v] += 1;
            adj[u].push(v);
        }
        let mut queue: std::collections::VecDeque<usize> = indegree
            .iter()
            .enumerate()
            .filter_map(|(idx, &deg)| (deg == 0).then_some(idx))
            .collect();
        let mut visited = 0usize;
        while let Some(node) = queue.pop_front() {
            visited += 1;
            for &next in &adj[node] {
                indegree[next] -= 1;
                if indegree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }
        visited == node_count
    }

    #[test]
    fn erdos_renyi_generator_produces_acyclic_graph() {
        let dag = generate_erdos_renyi_dag(&ErdosRenyiDagConfig {
            task_id: 1,
            node_count: 12,
            edge_probability: 0.4,
            seed: 7,
        });
        assert_eq!(dag.nodes.len(), 12);
        assert!(is_acyclic(12, &dag.edges));
    }

    #[test]
    fn layered_generator_produces_forward_only_edges() {
        let dag = generate_layered_dag(&LayeredDagConfig {
            task_id: 2,
            layer_widths: vec![2, 3, 2],
            edge_probability: 0.8,
            seed: 11,
        });
        for &(u, v) in &dag.edges {
            assert!(u < v, "layered DAG edge must be forward");
        }
    }

    #[test]
    fn dag_json_roundtrip_preserves_structure() {
        let dag = generate_erdos_renyi_dag(&ErdosRenyiDagConfig {
            task_id: 3,
            node_count: 8,
            edge_probability: 0.3,
            seed: 19,
        });
        let json = dag_to_json(&dag);
        let parsed = dag_from_json(&json).expect("roundtrip parse should succeed");
        assert_eq!(parsed.nodes.len(), dag.nodes.len());
        assert_eq!(parsed.edges, dag.edges);
    }
}
