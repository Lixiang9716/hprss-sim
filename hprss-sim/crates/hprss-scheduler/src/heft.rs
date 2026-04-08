use std::collections::HashMap;

use hprss_types::{
    Action, CriticalityLevel, DagInstanceId, DeviceId, Job, Scheduler, SchedulerView, TaskId,
    device::DeviceConfig,
    task::{DagTask, DeviceType, SubTask, Task},
};

#[derive(Debug, Clone)]
pub struct HeftPlan {
    pub rank_u: Vec<f64>,
    pub device_mapping: Vec<DeviceId>,
}

#[derive(Debug, Default)]
pub struct HeftPlanner {
    plans: HashMap<TaskId, HeftPlan>,
}

impl HeftPlanner {
    pub fn build_plan(&mut self, dag: &DagTask, devices: &[DeviceConfig]) -> &HeftPlan {
        let rank_u = compute_rank_u(dag, devices);
        let device_mapping = map_nodes(dag, devices, &rank_u);
        self.plans.entry(dag.id).or_insert(HeftPlan {
            rank_u,
            device_mapping,
        })
    }

    pub fn plan_for(&self, dag_id: TaskId) -> Option<&HeftPlan> {
        self.plans.get(&dag_id)
    }
}

/// Runtime HEFT scheduler (single-DAG baseline).
#[derive(Debug, Default)]
pub struct HeftScheduler {
    pub planner: HeftPlanner,
    instance_plans: HashMap<DagInstanceId, HeftPlan>,
}

impl HeftScheduler {
    pub fn set_instance_plan(&mut self, instance_id: DagInstanceId, plan: HeftPlan) {
        self.instance_plans.insert(instance_id, plan);
    }

    fn planned_device_for(&self, job: &Job) -> Option<DeviceId> {
        let prov = job.dag_provenance?;
        let plan = self.instance_plans.get(&prov.dag_instance_id)?;
        plan.device_mapping.get(prov.node.0 as usize).copied()
    }

    fn fallback_device(task: &Task, view: &SchedulerView<'_>) -> Option<DeviceId> {
        let candidates: Vec<DeviceId> = task
            .affinity
            .iter()
            .flat_map(|dt| {
                view.devices
                    .iter()
                    .filter(move |d| d.device_type == *dt)
                    .map(|d| d.id)
            })
            .collect();
        candidates.into_iter().min_by_key(|device_id| {
            let is_running = view
                .running_jobs
                .iter()
                .find(|(did, _)| did == device_id)
                .and_then(|(_, info)| info.as_ref())
                .is_some();
            let queue_len = view
                .ready_queues
                .iter()
                .find(|(did, _)| did == device_id)
                .map_or(0, |(_, q)| q.len());
            (is_running as u8, queue_len)
        })
    }
}

impl Scheduler for HeftScheduler {
    fn name(&self) -> &str {
        "HEFT"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let target_device = self
            .planned_device_for(job)
            .filter(|did| view.devices.iter().any(|d| d.id == *did))
            .or_else(|| Self::fallback_device(task, view));

        let Some(device_id) = target_device else {
            return vec![Action::NoOp];
        };
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, info)| info.as_ref());
        if running.is_none() {
            vec![Action::Dispatch {
                job_id: job.id,
                device_id,
            }]
        } else {
            vec![Action::Enqueue {
                job_id: job.id,
                device_id,
            }]
        }
    }

    fn on_job_complete(
        &mut self,
        _job: &Job,
        device_id: DeviceId,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| q.first());
        match next {
            Some(job) => vec![Action::Dispatch {
                job_id: job.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }

    fn on_preemption_point(
        &mut self,
        _device_id: DeviceId,
        _running_job: &Job,
        _view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        vec![Action::NoOp]
    }

    fn on_criticality_change(
        &mut self,
        new_level: CriticalityLevel,
        _trigger_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        if new_level == CriticalityLevel::Hi {
            let mut actions = Vec::new();
            for (_, queue) in view.ready_queues {
                for job in queue {
                    if job.criticality == CriticalityLevel::Lo {
                        actions.push(Action::DropJob { job_id: job.job_id });
                    }
                }
            }
            actions
        } else {
            vec![Action::NoOp]
        }
    }
}

fn compute_rank_u(dag: &DagTask, devices: &[DeviceConfig]) -> Vec<f64> {
    let mut succ = vec![Vec::<usize>::new(); dag.nodes.len()];
    for &(u, v) in &dag.edges {
        if u < dag.nodes.len() && v < dag.nodes.len() {
            succ[u].push(v);
        }
    }

    let mut memo = vec![None::<f64>; dag.nodes.len()];
    fn rank(
        idx: usize,
        dag: &DagTask,
        devices: &[DeviceConfig],
        succ: &[Vec<usize>],
        memo: &mut [Option<f64>],
    ) -> f64 {
        if let Some(v) = memo[idx] {
            return v;
        }
        let comp = avg_compute_cost(&dag.nodes[idx], devices);
        let mut downstream = 0.0f64;
        for &s in &succ[idx] {
            let comm = edge_bytes(dag, idx, s) as f64;
            downstream = downstream.max(comm + rank(s, dag, devices, succ, memo));
        }
        let v = comp + downstream;
        memo[idx] = Some(v);
        v
    }

    for i in 0..dag.nodes.len() {
        let _ = rank(i, dag, devices, &succ, &mut memo);
    }
    memo.into_iter().map(|v| v.unwrap_or(0.0)).collect()
}

fn map_nodes(dag: &DagTask, devices: &[DeviceConfig], rank_u: &[f64]) -> Vec<DeviceId> {
    if devices.is_empty() {
        return vec![];
    }

    let mut order: Vec<usize> = (0..dag.nodes.len()).collect();
    order.sort_by(|&a, &b| rank_u[b].total_cmp(&rank_u[a]));

    let mut predecessors = vec![Vec::<usize>::new(); dag.nodes.len()];
    for &(u, v) in &dag.edges {
        if u < dag.nodes.len() && v < dag.nodes.len() {
            predecessors[v].push(u);
        }
    }

    let mut device_available: HashMap<DeviceId, f64> = devices.iter().map(|d| (d.id, 0.0)).collect();
    let mut mapping = vec![devices[0].id; dag.nodes.len()];
    let mut finish_time = vec![0.0f64; dag.nodes.len()];
    let mut scheduled = vec![false; dag.nodes.len()];

    for idx in order {
        let node = &dag.nodes[idx];
        let candidates = candidate_devices(node, devices);
        let mut best = (f64::INFINITY, devices[0].id, 0.0f64);
        for dev in candidates {
            let ready_from_preds = predecessors[idx]
                .iter()
                .map(|&p| {
                    let comm = if scheduled[p] && mapping[p] != dev.id {
                        edge_bytes(dag, p, idx) as f64
                    } else {
                        0.0
                    };
                    finish_time[p] + comm
                })
                .fold(0.0, f64::max);
            let est = ready_from_preds.max(*device_available.get(&dev.id).unwrap_or(&0.0));
            let eft = est + exec_cost_on(node, dev.device_type);
            if eft < best.0 {
                best = (eft, dev.id, est);
            }
        }
        mapping[idx] = best.1;
        finish_time[idx] = best.0;
        scheduled[idx] = true;
        device_available.insert(best.1, best.0);
    }

    mapping
}

fn candidate_devices<'a>(node: &SubTask, devices: &'a [DeviceConfig]) -> Vec<&'a DeviceConfig> {
    let mut candidates: Vec<&DeviceConfig> = node
        .affinity
        .iter()
        .flat_map(|dt| devices.iter().filter(move |d| d.device_type == *dt))
        .collect();
    if candidates.is_empty() {
        candidates = devices.iter().collect();
    }
    candidates
}

fn avg_compute_cost(node: &SubTask, devices: &[DeviceConfig]) -> f64 {
    let candidates = candidate_devices(node, devices);
    if candidates.is_empty() {
        return 1.0;
    }
    let sum: f64 = candidates
        .iter()
        .map(|d| exec_cost_on(node, d.device_type))
        .sum();
    sum / candidates.len() as f64
}

fn exec_cost_on(node: &SubTask, device_type: DeviceType) -> f64 {
    node.exec_times
        .iter()
        .find(|(dt, _)| *dt == device_type)
        .or_else(|| node.exec_times.first())
        .map(|(_, m)| m.wcet() as f64)
        .unwrap_or(1.0)
}

fn edge_bytes(dag: &DagTask, from: usize, to: usize) -> u64 {
    dag.nodes
        .get(to)
        .and_then(|n| {
            n.data_deps
                .iter()
                .find(|(pred, _)| *pred == from)
                .map(|(_, bytes)| *bytes)
        })
        .unwrap_or(0)
}
