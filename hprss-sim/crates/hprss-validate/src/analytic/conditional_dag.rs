use std::collections::{BTreeMap, BTreeSet, VecDeque};

use thiserror::Error;

/// Scope of conditional DAG analytic support.
pub const CONDITIONAL_DAG_SCOPE: &str =
    "precedence-only conditional DAG bounds with explicit boolean branch assumptions";

const MAX_CONDITIONS_SUPPORTED: usize = usize::BITS as usize - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalDagModelAssumptions {
    /// Conditions are global binary predicates with fixed truth assignment per job instance.
    pub condition_domain: &'static str,
    /// At each branching node, exactly one outgoing edge is selected via one condition.
    pub branch_semantics: &'static str,
    /// Node execution and edge transfer costs are deterministic; no contention interference is modeled.
    pub timing_model: &'static str,
    /// Underlying graph must be acyclic.
    pub graph_constraints: &'static str,
}

impl Default for ConditionalDagModelAssumptions {
    fn default() -> Self {
        Self {
            condition_domain: "boolean conditions (true/false), consistent per scenario",
            branch_semantics: "if a node has conditional outgoing edges, they must be a complete exclusive binary split",
            timing_model: "response bound is precedence-only critical-path latency",
            graph_constraints: "directed acyclic graph with valid node indices",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionalDagNode {
    pub wcet_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionLiteral {
    pub condition_id: u32,
    pub expected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionalDagEdge {
    pub from: usize,
    pub to: usize,
    pub transfer_ns: u64,
    pub condition: Option<ConditionLiteral>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConditionalDagAnalysisConfig {
    pub deadline_ns: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConditionAssignment {
    pub condition_id: u32,
    pub value: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalDagScenarioReport {
    pub assignment: Vec<ConditionAssignment>,
    pub response_time_ns: u64,
    pub critical_path_nodes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalDagAnalysisReport {
    pub model_assumptions: ConditionalDagModelAssumptions,
    pub deadline_ns: u64,
    pub response_time_lower_bound_ns: u64,
    pub response_time_upper_bound_ns: u64,
    pub schedulable: bool,
    pub scenario_reports: Vec<ConditionalDagScenarioReport>,
}

impl ConditionalDagAnalysisReport {
    pub fn is_schedulable(&self) -> bool {
        self.schedulable
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ConditionalDagAnalysisError {
    #[error(
        "edge {edge_index} has invalid endpoint(s): from={from}, to={to}, node_count={node_count}"
    )]
    InvalidEdgeEndpoint {
        edge_index: usize,
        from: usize,
        to: usize,
        node_count: usize,
    },
    #[error("conditional DAG must be acyclic")]
    CyclicGraph,
    #[error("node {node} mixes conditional and unconditional outgoing edges")]
    MixedConditionalAndUnconditional { node: usize },
    #[error("node {node} uses multiple condition IDs for one branch point: {condition_ids:?}")]
    MultipleConditionsOnNode {
        node: usize,
        condition_ids: Vec<u32>,
    },
    #[error(
        "node {node} condition {condition_id} must provide both true/false branches (has_true={has_true}, has_false={has_false})"
    )]
    IncompleteConditionCoverage {
        node: usize,
        condition_id: u32,
        has_true: bool,
        has_false: bool,
    },
    #[error(
        "node {node} condition {condition_id} repeats outcome={outcome}; branch outcomes must be unique"
    )]
    DuplicateConditionOutcome {
        node: usize,
        condition_id: u32,
        outcome: bool,
    },
    #[error("condition space too large ({condition_count}); maximum supported is {max_supported}")]
    ScenarioSpaceTooLarge {
        condition_count: usize,
        max_supported: usize,
    },
    #[error("scenario has no reachable sink: {assignment:?}")]
    NoReachableSink {
        assignment: Vec<ConditionAssignment>,
    },
}

pub fn analyze_conditional_dag(
    nodes: &[ConditionalDagNode],
    edges: &[ConditionalDagEdge],
    config: ConditionalDagAnalysisConfig,
) -> Result<ConditionalDagAnalysisReport, ConditionalDagAnalysisError> {
    if nodes.is_empty() {
        return Ok(ConditionalDagAnalysisReport {
            model_assumptions: ConditionalDagModelAssumptions::default(),
            deadline_ns: config.deadline_ns,
            response_time_lower_bound_ns: 0,
            response_time_upper_bound_ns: 0,
            schedulable: true,
            scenario_reports: vec![ConditionalDagScenarioReport {
                assignment: Vec::new(),
                response_time_ns: 0,
                critical_path_nodes: Vec::new(),
            }],
        });
    }

    let mut edges_by_from = vec![Vec::<usize>::new(); nodes.len()];
    let mut indegree = vec![0usize; nodes.len()];
    for (edge_index, edge) in edges.iter().enumerate() {
        if edge.from >= nodes.len() || edge.to >= nodes.len() {
            return Err(ConditionalDagAnalysisError::InvalidEdgeEndpoint {
                edge_index,
                from: edge.from,
                to: edge.to,
                node_count: nodes.len(),
            });
        }
        edges_by_from[edge.from].push(edge_index);
        indegree[edge.to] = indegree[edge.to].saturating_add(1);
    }

    let topological_order = topological_sort(nodes.len(), edges, &indegree)?;
    let condition_ids = validate_branch_model(edges, &edges_by_from)?;

    if condition_ids.len() > MAX_CONDITIONS_SUPPORTED {
        return Err(ConditionalDagAnalysisError::ScenarioSpaceTooLarge {
            condition_count: condition_ids.len(),
            max_supported: MAX_CONDITIONS_SUPPORTED,
        });
    }

    let roots: Vec<usize> = indegree
        .iter()
        .enumerate()
        .filter_map(|(idx, deg)| (*deg == 0).then_some(idx))
        .collect();

    let scenario_count = 1usize << condition_ids.len();
    let mut scenario_reports = Vec::with_capacity(scenario_count);

    for mask in 0..scenario_count {
        let assignment = condition_ids
            .iter()
            .enumerate()
            .map(|(bit, condition_id)| ConditionAssignment {
                condition_id: *condition_id,
                value: (mask & (1usize << bit)) != 0,
            })
            .collect::<Vec<_>>();
        let assignment_map = assignment
            .iter()
            .map(|a| (a.condition_id, a.value))
            .collect::<BTreeMap<_, _>>();

        let (response_time_ns, critical_path_nodes) = evaluate_scenario(
            nodes,
            edges,
            &edges_by_from,
            &topological_order,
            &roots,
            &assignment,
            &assignment_map,
        )?;
        scenario_reports.push(ConditionalDagScenarioReport {
            assignment,
            response_time_ns,
            critical_path_nodes,
        });
    }

    let response_time_lower_bound_ns = scenario_reports
        .iter()
        .map(|s| s.response_time_ns)
        .min()
        .unwrap_or(0);
    let response_time_upper_bound_ns = scenario_reports
        .iter()
        .map(|s| s.response_time_ns)
        .max()
        .unwrap_or(0);

    Ok(ConditionalDagAnalysisReport {
        model_assumptions: ConditionalDagModelAssumptions::default(),
        deadline_ns: config.deadline_ns,
        response_time_lower_bound_ns,
        response_time_upper_bound_ns,
        schedulable: response_time_upper_bound_ns <= config.deadline_ns,
        scenario_reports,
    })
}

fn topological_sort(
    node_count: usize,
    edges: &[ConditionalDagEdge],
    indegree: &[usize],
) -> Result<Vec<usize>, ConditionalDagAnalysisError> {
    let mut indegree_work = indegree.to_vec();
    let mut queue = VecDeque::new();
    for (idx, value) in indegree_work.iter().enumerate() {
        if *value == 0 {
            queue.push_back(idx);
        }
    }

    let mut order = Vec::with_capacity(node_count);
    while let Some(node) = queue.pop_front() {
        order.push(node);
        for edge in edges.iter().filter(|edge| edge.from == node) {
            indegree_work[edge.to] = indegree_work[edge.to].saturating_sub(1);
            if indegree_work[edge.to] == 0 {
                queue.push_back(edge.to);
            }
        }
    }

    if order.len() != node_count {
        return Err(ConditionalDagAnalysisError::CyclicGraph);
    }
    Ok(order)
}

fn validate_branch_model(
    edges: &[ConditionalDagEdge],
    edges_by_from: &[Vec<usize>],
) -> Result<Vec<u32>, ConditionalDagAnalysisError> {
    let mut condition_ids = BTreeSet::new();

    for (node, outgoing) in edges_by_from.iter().enumerate() {
        let mut unconditional_count = 0usize;
        let mut condition_map = BTreeMap::<u32, (usize, usize)>::new();

        for edge_index in outgoing {
            let edge = &edges[*edge_index];
            if let Some(condition) = edge.condition {
                condition_ids.insert(condition.condition_id);
                let entry = condition_map
                    .entry(condition.condition_id)
                    .or_insert((0, 0));
                if condition.expected {
                    entry.0 = entry.0.saturating_add(1);
                } else {
                    entry.1 = entry.1.saturating_add(1);
                }
            } else {
                unconditional_count = unconditional_count.saturating_add(1);
            }
        }

        if condition_map.is_empty() {
            continue;
        }

        if unconditional_count > 0 {
            return Err(ConditionalDagAnalysisError::MixedConditionalAndUnconditional { node });
        }

        if condition_map.len() > 1 {
            return Err(ConditionalDagAnalysisError::MultipleConditionsOnNode {
                node,
                condition_ids: condition_map.keys().copied().collect(),
            });
        }

        let (&condition_id, &(true_count, false_count)) =
            condition_map.iter().next().expect("map is not empty");

        if true_count > 1 {
            return Err(ConditionalDagAnalysisError::DuplicateConditionOutcome {
                node,
                condition_id,
                outcome: true,
            });
        }

        if false_count > 1 {
            return Err(ConditionalDagAnalysisError::DuplicateConditionOutcome {
                node,
                condition_id,
                outcome: false,
            });
        }

        if true_count == 0 || false_count == 0 {
            return Err(ConditionalDagAnalysisError::IncompleteConditionCoverage {
                node,
                condition_id,
                has_true: true_count > 0,
                has_false: false_count > 0,
            });
        }
    }

    Ok(condition_ids.into_iter().collect())
}

fn evaluate_scenario(
    nodes: &[ConditionalDagNode],
    edges: &[ConditionalDagEdge],
    edges_by_from: &[Vec<usize>],
    topological_order: &[usize],
    roots: &[usize],
    assignment: &[ConditionAssignment],
    assignment_map: &BTreeMap<u32, bool>,
) -> Result<(u64, Vec<usize>), ConditionalDagAnalysisError> {
    let mut active_edge = vec![false; edges.len()];
    for (index, edge) in edges.iter().enumerate() {
        active_edge[index] = match edge.condition {
            Some(condition) => assignment_map
                .get(&condition.condition_id)
                .is_some_and(|value| *value == condition.expected),
            None => true,
        };
    }

    let mut distance = vec![None::<u64>; nodes.len()];
    let mut predecessor = vec![None::<usize>; nodes.len()];

    for root in roots {
        distance[*root] = Some(nodes[*root].wcet_ns);
    }

    for node in topological_order {
        let Some(node_finish) = distance[*node] else {
            continue;
        };

        for edge_index in &edges_by_from[*node] {
            if !active_edge[*edge_index] {
                continue;
            }
            let edge = edges[*edge_index];
            let candidate = node_finish
                .saturating_add(edge.transfer_ns)
                .saturating_add(nodes[edge.to].wcet_ns);

            if distance[edge.to].is_none_or(|current| candidate > current) {
                distance[edge.to] = Some(candidate);
                predecessor[edge.to] = Some(*node);
            }
        }
    }

    let mut chosen_sink = None::<usize>;
    let mut chosen_response = 0_u64;

    for node in 0..nodes.len() {
        let Some(node_finish) = distance[node] else {
            continue;
        };
        let has_active_outgoing = edges_by_from[node]
            .iter()
            .any(|edge_index| active_edge[*edge_index]);
        if !has_active_outgoing && node_finish >= chosen_response {
            chosen_response = node_finish;
            chosen_sink = Some(node);
        }
    }

    let Some(sink) = chosen_sink else {
        return Err(ConditionalDagAnalysisError::NoReachableSink {
            assignment: assignment.to_vec(),
        });
    };

    let mut critical_path_nodes = vec![sink];
    let mut cursor = sink;
    while let Some(prev) = predecessor[cursor] {
        critical_path_nodes.push(prev);
        cursor = prev;
    }
    critical_path_nodes.reverse();

    Ok((chosen_response, critical_path_nodes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_node_with_mixed_outgoing_types() {
        let nodes = vec![
            ConditionalDagNode { wcet_ns: 1 },
            ConditionalDagNode { wcet_ns: 1 },
        ];
        let edges = vec![
            ConditionalDagEdge {
                from: 0,
                to: 1,
                transfer_ns: 0,
                condition: None,
            },
            ConditionalDagEdge {
                from: 0,
                to: 1,
                transfer_ns: 0,
                condition: Some(ConditionLiteral {
                    condition_id: 1,
                    expected: true,
                }),
            },
        ];

        let err = analyze_conditional_dag(
            &nodes,
            &edges,
            ConditionalDagAnalysisConfig { deadline_ns: 10 },
        )
        .expect_err("mixed branch model must be rejected");
        assert_eq!(
            err,
            ConditionalDagAnalysisError::MixedConditionalAndUnconditional { node: 0 }
        );
    }
}
