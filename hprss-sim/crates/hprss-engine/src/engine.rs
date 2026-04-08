//! The DES simulation engine.

use std::collections::BinaryHeap;
use std::path::Path;

use hprss_metrics::{BlockingBreakdown, DeviceUtilization, MetricsCollector};
use hprss_types::{
    Action, CriticalityLevel, DeviceId, Event, EventKind, InterconnectConfig, Job, JobId, JobState,
    Nanos, Scheduler, SharedBusConfig, Task, TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DagTask, DeviceType, ExecutionTimeModel},
};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

use crate::{
    dag_tracker::DagTracker,
    device_manager::DeviceManager,
    transfer_manager::{JobTransferKind, ScheduledEvent, TransferManager},
};

/// Simulation engine configuration
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Total simulation duration in nanoseconds
    pub duration_ns: Nanos,
    /// Random seed for reproducibility
    pub seed: u64,
}

/// The core simulation engine
pub struct SimEngine {
    /// Configuration
    config: SimConfig,
    /// Current simulation time
    now: Nanos,
    /// Event queue (min-heap via reversed Ord)
    event_queue: BinaryHeap<Event>,
    /// Monotonic sequence counter for tie-breaking
    seq_counter: u64,
    /// All active jobs indexed by JobId
    jobs: Vec<Option<Job>>,
    /// Next job ID to allocate
    next_job_id: u64,
    /// Task registry indexed by TaskId
    task_registry: Vec<Task>,
    /// Device state manager
    device_mgr: DeviceManager,
    /// Transfer and bus manager
    transfer_mgr: TransferManager,
    /// DAG dependency tracker and SubTask proxy mapping
    dag_tracker: DagTracker,
    /// Current mixed-criticality level
    criticality_level: CriticalityLevel,
    /// Random source for stochastic execution models
    rng: ChaCha8Rng,
    /// Metrics collector
    metrics: MetricsCollector,
    /// Events processed counter
    events_processed: u64,
    /// Cumulative running (busy) wall-time per device.
    device_busy_time_ns: Vec<Nanos>,
    /// Number of successful preemptions performed by the engine.
    preemption_count: u64,
    /// Number of migrations performed by the engine.
    migration_count: u64,
}

impl SimEngine {
    pub fn new(
        config: SimConfig,
        devices: Vec<DeviceConfig>,
        interconnects: Vec<InterconnectConfig>,
        buses: Vec<SharedBusConfig>,
    ) -> Self {
        let seed = config.seed;
        Self {
            device_busy_time_ns: vec![0; devices.len()],
            config,
            now: 0,
            event_queue: BinaryHeap::new(),
            seq_counter: 0,
            jobs: Vec::new(),
            next_job_id: 0,
            task_registry: Vec::new(),
            device_mgr: DeviceManager::new(devices),
            transfer_mgr: TransferManager::new(interconnects, buses),
            dag_tracker: DagTracker::new(),
            criticality_level: CriticalityLevel::Lo,
            rng: ChaCha8Rng::seed_from_u64(seed),
            metrics: MetricsCollector::new(),
            events_processed: 0,
            preemption_count: 0,
            migration_count: 0,
        }
    }

    /// Register all tasks used by the simulation.
    pub fn register_tasks(&mut self, mut tasks: Vec<Task>) {
        tasks.sort_by_key(|t| t.id.0);
        self.task_registry = tasks;
    }

    /// Register DAG workloads and schedule initial source-node releases at t=0.
    pub fn register_dags(&mut self, dags: Vec<DagTask>) {
        for dag in dags {
            let reg = self.dag_tracker.register_dag(dag, &mut self.task_registry);
            for task_id in reg.ready_task_ids {
                self.schedule_job_release(task_id, 0);
            }
        }
    }

    /// Schedule the first release for all periodic/sporadic tasks.
    pub fn schedule_initial_arrivals(&mut self) {
        let task_ids: Vec<TaskId> = self
            .task_registry
            .iter()
            .filter_map(|t| match t.arrival {
                ArrivalModel::Periodic { .. } | ArrivalModel::Sporadic { .. } => Some(t.id),
                ArrivalModel::Aperiodic => None,
            })
            .collect();

        for task_id in task_ids {
            self.schedule_job_release(task_id, 0);
        }
    }

    /// Schedule a new event
    pub fn schedule_event(&mut self, time: Nanos, kind: EventKind) {
        let seq = self.seq_counter;
        self.seq_counter += 1;
        self.event_queue.push(Event { time, seq, kind });
    }

    /// Allocate a new job
    pub fn create_job(
        &mut self,
        task_id: TaskId,
        release_time: Nanos,
        absolute_deadline: Nanos,
        actual_exec_ns: Option<Nanos>,
        priority: u32,
    ) -> JobId {
        let id = JobId(self.next_job_id);
        self.next_job_id += 1;

        let job = Job::new(
            id,
            task_id,
            release_time,
            absolute_deadline,
            actual_exec_ns,
            priority,
        );

        // Grow vector if needed
        let idx = id.0 as usize;
        if idx >= self.jobs.len() {
            self.jobs.resize_with(idx + 1, || None);
        }
        self.jobs[idx] = Some(job);
        id
    }

    /// Look up a job by ID
    pub fn get_job(&self, id: JobId) -> Option<&Job> {
        self.jobs.get(id.0 as usize).and_then(|opt| opt.as_ref())
    }

    /// Look up a job mutably
    pub fn get_job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs
            .get_mut(id.0 as usize)
            .and_then(|opt| opt.as_mut())
    }

    /// Run the main simulation loop
    pub fn run(&mut self, scheduler: &mut dyn Scheduler) {
        // Schedule simulation end
        self.schedule_event(self.config.duration_ns, EventKind::SimulationEnd);

        tracing::info!(
            "Simulation starting: duration={}ns, seed={}",
            self.config.duration_ns,
            self.config.seed
        );

        while let Some(event) = self.event_queue.pop() {
            // Advance simulation clock
            debug_assert!(
                event.time >= self.now,
                "Time went backwards: {} < {}",
                event.time,
                self.now
            );
            self.now = event.time;

            // Check for stale events (version-based invalidation)
            if let Some(job_id) = Self::event_job_id(&event.kind)
                && let Some(job) = self.get_job(job_id)
                && event.kind.is_stale(job.version)
            {
                tracing::trace!(
                    "Discarding stale event {:?} for job {:?}",
                    event.kind,
                    job_id
                );
                continue;
            }

            self.events_processed += 1;

            match event.kind {
                EventKind::SimulationEnd => {
                    tracing::info!(
                        "Simulation ended at t={}ns, events_processed={}",
                        self.now,
                        self.events_processed
                    );
                    break;
                }
                EventKind::TaskArrival { task_id, job_id } => {
                    self.handle_task_arrival(task_id, job_id, scheduler);
                }
                EventKind::JobComplete {
                    job_id, device_id, ..
                } => {
                    self.handle_job_complete(job_id, device_id, scheduler);
                }
                EventKind::PreemptionPoint {
                    device_id, job_id, ..
                } => {
                    self.handle_preemption_point(device_id, job_id, scheduler);
                }
                EventKind::TransferComplete {
                    job_id, device_id, ..
                } => {
                    self.handle_transfer_complete(job_id, device_id, scheduler);
                }
                EventKind::EdgeTransferComplete {
                    edge_id, device_id, ..
                } => {
                    self.handle_edge_transfer_complete(edge_id, device_id);
                }
                EventKind::BusArbitration { bus_id } => {
                    let events = self.transfer_mgr.on_bus_arbitration(bus_id, self.now);
                    self.schedule_transfer_events(events);
                }
                EventKind::DeadlineCheck { job_id, .. } => {
                    if let Some(job) = self.get_job(job_id)
                        && job.has_missed_deadline(self.now)
                    {
                        tracing::warn!(
                            "t={}: DEADLINE MISS job={:?} deadline={}",
                            self.now,
                            job_id,
                            job.absolute_deadline
                        );
                        self.metrics
                            .record_deadline_miss(job_id, job.task_id, self.now);
                        let running_dev = self.device_mgr.devices().iter().find_map(|d| {
                            if self.device_mgr.running_job(d.id) == Some(job_id) {
                                Some(d.id)
                            } else {
                                None
                            }
                        });
                        if let Some(dev) = running_dev {
                            self.record_running_elapsed(job_id, dev);
                        }
                        if let Some(job_mut) = self.get_job_mut(job_id) {
                            job_mut.transition(JobState::DeadlineMissed);
                            job_mut.exec_start_time = None;
                        }
                        self.device_mgr.remove_job_from_all_queues(job_id);
                        if let Some(dev) = running_dev {
                            self.device_mgr.clear_running(dev);
                        }
                    }
                }
                EventKind::BudgetOverrun { job_id, .. } => {
                    self.handle_budget_overrun(job_id, scheduler);
                }
            }
        }
    }

    fn handle_task_arrival(
        &mut self,
        task_id: TaskId,
        job_id: JobId,
        scheduler: &mut dyn Scheduler,
    ) {
        tracing::trace!(
            "t={}: TaskArrival task={:?} job={:?}",
            self.now,
            task_id,
            job_id
        );

        if self.get_job(job_id).is_none() {
            return;
        }
        self.metrics.record_job_release();

        let next_release = self.task(task_id).and_then(|task| match task.arrival {
            ArrivalModel::Periodic { period } => Some(self.now.saturating_add(period)),
            ArrivalModel::Sporadic { min_inter_arrival } => {
                Some(self.now.saturating_add(min_inter_arrival))
            }
            ArrivalModel::Aperiodic => None,
        });

        let actions = {
            self.device_mgr
                .rebuild_view_data(self.now, &self.jobs, &self.task_registry);
            let job = match self.get_job(job_id) {
                Some(job) => job,
                None => return,
            };
            let task = &self.task_registry[task_id.0 as usize];
            let view = self
                .device_mgr
                .scheduler_view(self.now, self.criticality_level);
            scheduler.on_job_arrival(job, task, &view)
        };
        self.execute_actions(actions, false);

        if let Some(release_time) = next_release {
            self.schedule_job_release(task_id, release_time);
        }
    }

    fn handle_job_complete(
        &mut self,
        job_id: JobId,
        device_id: DeviceId,
        scheduler: &mut dyn Scheduler,
    ) {
        tracing::trace!(
            "t={}: JobComplete job={:?} dev={:?}",
            self.now,
            job_id,
            device_id
        );

        if self.device_mgr.running_job(device_id) != Some(job_id) {
            return;
        }
        self.device_mgr.clear_running(device_id);

        let speed_factor = self.device_mgr.device(device_id).speed_factor;
        let now = self.now;
        let mut busy_elapsed = 0;
        if let Some(job) = self.get_job_mut(job_id) {
            if job.state != JobState::Running {
                return;
            }
            if let Some(start) = job.exec_start_time {
                let wall_elapsed = now.saturating_sub(start);
                let work_done = wall_to_work(wall_elapsed, speed_factor).min(job.remaining_ns());
                job.record_progress(work_done);
                busy_elapsed = wall_elapsed;
            }
            if let Some(actual_exec_ns) = job.actual_exec_ns {
                job.executed_ns = actual_exec_ns;
            }
            let task_id = job.task_id;
            let release_time = job.release_time;
            job.exec_start_time = None;
            job.transition(JobState::Completed);
            self.metrics
                .record_completion(job_id, task_id, release_time, now);
        } else {
            return;
        }
        self.device_busy_time_ns[device_id.0 as usize] =
            self.device_busy_time_ns[device_id.0 as usize].saturating_add(busy_elapsed);

        self.schedule_dag_outgoing_transfers(job_id, device_id);

        let actions = {
            self.device_mgr
                .rebuild_view_data(self.now, &self.jobs, &self.task_registry);
            let job = match self.get_job(job_id) {
                Some(job) => job,
                None => return,
            };
            let view = self
                .device_mgr
                .scheduler_view(self.now, self.criticality_level);
            scheduler.on_job_complete(job, device_id, &view)
        };
        self.execute_actions(actions, false);
    }

    fn handle_preemption_point(
        &mut self,
        device_id: DeviceId,
        job_id: JobId,
        scheduler: &mut dyn Scheduler,
    ) {
        tracing::trace!(
            "t={}: PreemptionPoint dev={:?} job={:?}",
            self.now,
            device_id,
            job_id
        );

        if self.device_mgr.running_job(device_id) != Some(job_id) {
            return;
        }

        let actions = {
            self.device_mgr
                .rebuild_view_data(self.now, &self.jobs, &self.task_registry);
            let running_job = match self.get_job(job_id) {
                Some(job) => job,
                None => return,
            };
            let view = self
                .device_mgr
                .scheduler_view(self.now, self.criticality_level);
            scheduler.on_preemption_point(device_id, running_job, &view)
        };

        self.execute_actions(actions, true);

        // If no preemption happened, reschedule next preemption point for the same running job.
        if self.device_mgr.running_job(device_id) == Some(job_id) {
            let interval = match self.device_mgr.device(device_id).preemption {
                PreemptionModel::LimitedPreemptive { granularity_ns } => Some(granularity_ns),
                PreemptionModel::InterruptLevel {
                    dma_non_preemptive_ns,
                    ..
                } => Some(dma_non_preemptive_ns),
                PreemptionModel::FullyPreemptive | PreemptionModel::NonPreemptive { .. } => None,
            };
            if let Some(interval_ns) = interval
                && let Some(job) = self.get_job(job_id)
            {
                self.schedule_event(
                    self.now.saturating_add(interval_ns),
                    EventKind::PreemptionPoint {
                        device_id,
                        job_id,
                        expected_version: job.version,
                    },
                );
            }
        }
    }

    fn handle_transfer_complete(
        &mut self,
        job_id: JobId,
        device_id: DeviceId,
        scheduler: &mut dyn Scheduler,
    ) {
        tracing::trace!(
            "t={}: TransferComplete job={:?} dev={:?}",
            self.now,
            job_id,
            device_id
        );

        let events = self.transfer_mgr.on_transfer_complete(job_id, self.now);
        self.schedule_transfer_events(events);

        let task_id = match self.get_job(job_id) {
            Some(job) if !job.state.is_terminal() => job.task_id,
            _ => return,
        };

        if let Some(job) = self.get_job_mut(job_id) {
            job.assigned_device = Some(device_id);
            job.exec_start_time = None;
            if job.state != JobState::Ready {
                job.transition(JobState::Ready);
            }
        }

        // Transfer completion re-enters the scheduling loop (arrival-like callback).
        let actions = {
            self.device_mgr
                .rebuild_view_data(self.now, &self.jobs, &self.task_registry);
            let job = match self.get_job(job_id) {
                Some(job) => job,
                None => return,
            };
            let task = &self.task_registry[task_id.0 as usize];
            let view = self
                .device_mgr
                .scheduler_view(self.now, self.criticality_level);
            scheduler.on_job_arrival(job, task, &view)
        };
        self.execute_actions(actions, false);
    }

    fn handle_edge_transfer_complete(
        &mut self,
        edge_id: hprss_types::EdgeTransferId,
        device_id: DeviceId,
    ) {
        tracing::trace!(
            "t={}: EdgeTransferComplete edge={:?} dev={:?}",
            self.now,
            edge_id,
            device_id
        );

        let events = self
            .transfer_mgr
            .on_edge_transfer_complete(edge_id, self.now);
        self.schedule_transfer_events(events);

        let released = self.dag_tracker.mark_edge_satisfied(
            edge_id.dag_instance_id,
            edge_id.from_node,
            edge_id.to_node,
        );
        for task_id in released {
            self.schedule_job_release(task_id, self.now);
        }
    }

    fn handle_budget_overrun(&mut self, job_id: JobId, scheduler: &mut dyn Scheduler) {
        tracing::warn!("t={}: BudgetOverrun job={:?}", self.now, job_id);

        if self.criticality_level == CriticalityLevel::Hi {
            return;
        }
        self.criticality_level = CriticalityLevel::Hi;

        let actions = {
            self.device_mgr
                .rebuild_view_data(self.now, &self.jobs, &self.task_registry);
            let trigger_job = match self.get_job(job_id) {
                Some(job) => job,
                None => return,
            };
            let view = self
                .device_mgr
                .scheduler_view(self.now, self.criticality_level);
            scheduler.on_criticality_change(CriticalityLevel::Hi, trigger_job, &view)
        };
        self.execute_actions(actions, false);
        self.drop_lo_critical_jobs();
    }

    fn execute_actions(&mut self, actions: Vec<Action>, at_preemption_point: bool) {
        for action in actions {
            match action {
                Action::Dispatch { job_id, device_id } => self.dispatch_job(job_id, device_id),
                Action::Preempt {
                    victim,
                    by,
                    device_id,
                } => {
                    let allow_now = match self.device_mgr.device(device_id).preemption {
                        PreemptionModel::FullyPreemptive => true,
                        PreemptionModel::LimitedPreemptive { .. }
                        | PreemptionModel::InterruptLevel { .. } => at_preemption_point,
                        PreemptionModel::NonPreemptive { .. } => false,
                    };
                    if allow_now {
                        self.preempt_job(victim, by, device_id);
                    } else {
                        self.enqueue_job(by, device_id);
                    }
                }
                Action::Migrate { job_id, from, to } => self.migrate_job(job_id, from, to),
                Action::Enqueue { job_id, device_id } => self.enqueue_job(job_id, device_id),
                Action::DropJob { job_id } => self.drop_job(job_id),
                Action::NoOp => {}
            }
        }
    }

    fn dispatch_job(&mut self, job_id: JobId, device_id: DeviceId) {
        let Some(job_view) = self.get_job(job_id) else {
            return;
        };
        let task_id = job_view.task_id;
        let assigned_device = job_view.assigned_device;
        if job_view.state.is_terminal() {
            return;
        }
        if let Some(running) = self.device_mgr.running_job(device_id) {
            if running != job_id {
                self.enqueue_job(job_id, device_id);
            }
            return;
        }

        self.device_mgr.remove_from_queue(device_id, job_id);

        let Some(task) = self.task(task_id).cloned() else {
            return;
        };
        let target_device = self.device_mgr.device(device_id);
        let target_type = target_device.device_type;
        let speed_factor = target_device.speed_factor;
        let context_switch_ns = target_device.context_switch_ns;
        let preemption = target_device.preemption.clone();
        let Some(resolved_exec_ns) =
            sample_exec_time_for_device(&task, target_type, self.criticality_level, &mut self.rng)
        else {
            tracing::warn!(
                "No execution-time model for task={:?} on device={:?}",
                task_id,
                target_type
            );
            return;
        };

        // Accelerator dispatch needs data transfer first (single source: CPU or previous device).
        if task.data_size > 0
            && target_type != DeviceType::Cpu
            && assigned_device != Some(device_id)
        {
            let source = assigned_device
                .or_else(|| {
                    self.device_mgr
                        .device_for_type(DeviceType::Cpu)
                        .map(|d| d.id)
                })
                .unwrap_or(device_id);

            let (priority, expected_version) = {
                let Some(job) = self.get_job_mut(job_id) else {
                    return;
                };
                job.assigned_device = Some(device_id);
                if job.actual_exec_ns.is_none() {
                    job.actual_exec_ns = Some(resolved_exec_ns);
                }
                job.exec_start_time = None;
                if job.state != JobState::Transferring {
                    job.transition(JobState::Transferring);
                }
                (job.effective_priority, job.version)
            };

            let events = self.transfer_mgr.initiate_transfer(
                job_id,
                source,
                device_id,
                task.data_size,
                JobTransferKind::Dispatch,
                priority,
                self.now,
                expected_version,
            );
            self.schedule_transfer_events(events);
            return;
        }

        let now = self.now;
        let (expected_version, remaining_work, task_id, executed_work) = {
            let Some(job) = self.get_job_mut(job_id) else {
                return;
            };
            if job.state.is_terminal() {
                return;
            }
            job.assigned_device = Some(device_id);
            if job.actual_exec_ns.is_none() {
                job.actual_exec_ns = Some(resolved_exec_ns);
            }
            job.exec_start_time = Some(now);
            job.transition(JobState::Running);
            (
                job.version,
                job.remaining_ns(),
                job.task_id,
                job.executed_ns,
            )
        };

        self.device_mgr.set_running(device_id, job_id);

        let wall_exec_ns = work_to_wall(remaining_work, speed_factor);
        self.schedule_event(
            now.saturating_add(context_switch_ns)
                .saturating_add(wall_exec_ns),
            EventKind::JobComplete {
                job_id,
                expected_version,
                device_id,
            },
        );

        if self.criticality_level == CriticalityLevel::Lo
            && let Some(wcet_lo) = self.mixed_criticality_lo_wcet(task_id)
        {
            let remaining_lo = wcet_lo.saturating_sub(executed_work);
            let delay_ns = work_to_wall(remaining_lo, speed_factor);
            self.schedule_event(
                now.saturating_add(delay_ns),
                EventKind::BudgetOverrun {
                    job_id,
                    expected_version,
                },
            );
        }

        match preemption {
            PreemptionModel::LimitedPreemptive { granularity_ns } => {
                self.schedule_event(
                    now.saturating_add(granularity_ns),
                    EventKind::PreemptionPoint {
                        device_id,
                        job_id,
                        expected_version,
                    },
                );
            }
            PreemptionModel::InterruptLevel {
                dma_non_preemptive_ns,
                ..
            } => {
                self.schedule_event(
                    now.saturating_add(dma_non_preemptive_ns),
                    EventKind::PreemptionPoint {
                        device_id,
                        job_id,
                        expected_version,
                    },
                );
            }
            PreemptionModel::FullyPreemptive | PreemptionModel::NonPreemptive { .. } => {}
        }
    }

    fn preempt_job(&mut self, victim: JobId, by: JobId, device_id: DeviceId) {
        if self.device_mgr.running_job(device_id) != Some(victim) {
            self.enqueue_job(by, device_id);
            return;
        }

        let speed_factor = self.device_mgr.device(device_id).speed_factor;
        let now = self.now;
        let mut busy_elapsed = 0;
        let victim_priority = {
            let Some(victim_job) = self.get_job_mut(victim) else {
                return;
            };
            if victim_job.state != JobState::Running {
                return;
            }
            if let Some(start) = victim_job.exec_start_time {
                let wall_elapsed = now.saturating_sub(start);
                let work_done =
                    wall_to_work(wall_elapsed, speed_factor).min(victim_job.remaining_ns());
                victim_job.record_progress(work_done);
                busy_elapsed = wall_elapsed;
            }
            victim_job.exec_start_time = None;
            victim_job.transition(JobState::Suspended);
            victim_job.effective_priority
        };
        self.device_busy_time_ns[device_id.0 as usize] =
            self.device_busy_time_ns[device_id.0 as usize].saturating_add(busy_elapsed);

        self.device_mgr.clear_running(device_id);
        self.device_mgr.enqueue(device_id, victim, victim_priority);
        self.preemption_count = self.preemption_count.saturating_add(1);
        self.dispatch_job(by, device_id);
    }

    fn enqueue_job(&mut self, job_id: JobId, device_id: DeviceId) {
        if let Some(job) = self.get_job_mut(job_id) {
            if job.state.is_terminal() {
                return;
            }
            job.assigned_device = Some(device_id);
            job.exec_start_time = None;
            if job.state != JobState::Ready {
                job.transition(JobState::Ready);
            }
            let priority = job.effective_priority;
            let _ = job;
            self.device_mgr.enqueue(device_id, job_id, priority);
        }
    }

    fn drop_job(&mut self, job_id: JobId) {
        self.device_mgr.remove_job_from_all_queues(job_id);

        let running_device = self
            .device_mgr
            .devices()
            .iter()
            .find_map(|d| (self.device_mgr.running_job(d.id) == Some(job_id)).then_some(d.id));
        if let Some(dev) = running_device {
            self.record_running_elapsed(job_id, dev);
            self.device_mgr.clear_running(dev);
        }

        let events = self.transfer_mgr.cancel_job(job_id, self.now);
        self.schedule_transfer_events(events);

        if let Some(job) = self.get_job_mut(job_id)
            && !job.state.is_terminal()
        {
            job.exec_start_time = None;
            job.transition(JobState::Dropped);
        }
    }

    fn migrate_job(&mut self, job_id: JobId, from: DeviceId, to: DeviceId) {
        let was_running = self.device_mgr.running_job(from) == Some(job_id);
        let speed_factor = self.device_mgr.device(from).speed_factor;
        let now = self.now;
        let data_size = self
            .get_job(job_id)
            .and_then(|job| self.task(job.task_id).map(|t| t.data_size))
            .unwrap_or(0);

        let mut busy_elapsed = 0;
        let (priority, expected_version, data_size) = {
            let Some(job) = self.get_job_mut(job_id) else {
                return;
            };
            if job.state.is_terminal() {
                return;
            }
            if was_running && let Some(start) = job.exec_start_time {
                let wall_elapsed = now.saturating_sub(start);
                let work_done = wall_to_work(wall_elapsed, speed_factor).min(job.remaining_ns());
                job.record_progress(work_done);
                busy_elapsed = wall_elapsed;
            }
            job.exec_start_time = None;
            job.assigned_device = Some(to);
            job.transition(JobState::Migrating);
            (job.effective_priority, job.version, data_size)
        };
        self.device_busy_time_ns[from.0 as usize] =
            self.device_busy_time_ns[from.0 as usize].saturating_add(busy_elapsed);
        if was_running {
            self.device_mgr.clear_running(from);
        }

        let events = self.transfer_mgr.initiate_transfer(
            job_id,
            from,
            to,
            data_size,
            JobTransferKind::Migration,
            priority,
            self.now,
            expected_version,
        );
        self.schedule_transfer_events(events);
        self.migration_count = self.migration_count.saturating_add(1);
    }

    fn drop_lo_critical_jobs(&mut self) {
        let to_drop: Vec<JobId> = self
            .jobs
            .iter()
            .flatten()
            .filter(|job| {
                !job.state.is_terminal()
                    && self
                        .task_criticality_for_job(job.id)
                        .is_some_and(|c| c == CriticalityLevel::Lo)
            })
            .map(|job| job.id)
            .collect();

        for job_id in to_drop {
            self.drop_job(job_id);
        }
    }

    fn schedule_job_release(&mut self, task_id: TaskId, release_time: Nanos) {
        if release_time > self.config.duration_ns {
            return;
        }

        let Some(task) = self.task(task_id).cloned() else {
            return;
        };
        let absolute_deadline = release_time.saturating_add(task.deadline);
        let priority = task.priority;

        let job_id = self.create_job(task_id, release_time, absolute_deadline, None, priority);
        if let Some(prov) = self.dag_tracker.provenance_for_task(task_id)
            && let Some(job) = self.get_job_mut(job_id)
        {
            job.dag_provenance = Some(prov);
        }
        let expected_version = self.get_job(job_id).map_or(0, |j| j.version);

        self.schedule_event(release_time, EventKind::TaskArrival { task_id, job_id });
        self.schedule_event(
            absolute_deadline,
            EventKind::DeadlineCheck {
                job_id,
                expected_version,
            },
        );
    }

    fn schedule_transfer_events(&mut self, events: Vec<ScheduledEvent>) {
        for ev in events {
            self.schedule_event(ev.time, ev.kind);
        }
    }

    fn record_running_elapsed(&mut self, job_id: JobId, device_id: DeviceId) {
        let speed_factor = self.device_mgr.device(device_id).speed_factor;
        let now = self.now;
        if let Some(job) = self.get_job_mut(job_id)
            && job.state == JobState::Running
            && let Some(start) = job.exec_start_time
        {
            let wall_elapsed = now.saturating_sub(start);
            let work_done = wall_to_work(wall_elapsed, speed_factor).min(job.remaining_ns());
            job.record_progress(work_done);
            self.device_busy_time_ns[device_id.0 as usize] =
                self.device_busy_time_ns[device_id.0 as usize].saturating_add(wall_elapsed);
        }
    }

    fn schedule_dag_outgoing_transfers(&mut self, job_id: JobId, from_device: DeviceId) {
        let Some(job) = self.get_job(job_id).cloned() else {
            return;
        };
        let Some(prov) = job.dag_provenance else {
            return;
        };

        for (succ_node, bytes) in self
            .dag_tracker
            .outgoing_edges(prov.dag_instance_id, prov.node)
        {
            let Some(successor_task_id) = self
                .dag_tracker
                .proxy_task_id(prov.dag_instance_id, succ_node)
            else {
                continue;
            };
            let Some(successor_task) = self.task(successor_task_id).cloned() else {
                continue;
            };
            let Some(target_type) = successor_task.affinity.first().copied() else {
                continue;
            };
            let Some(target_device) = self.device_mgr.device_for_type(target_type).map(|d| d.id)
            else {
                continue;
            };

            let edge_id = hprss_types::EdgeTransferId {
                dag_instance_id: prov.dag_instance_id,
                from_node: prov.node,
                to_node: succ_node,
            };
            let events = self.transfer_mgr.initiate_edge_transfer(
                edge_id,
                job_id,
                from_device,
                target_device,
                bytes,
                job.effective_priority,
                self.now,
                job.version,
            );
            self.schedule_transfer_events(events);
        }
    }

    /// Extract the job ID from an event kind (if applicable)
    fn event_job_id(kind: &EventKind) -> Option<JobId> {
        match kind {
            EventKind::JobComplete { job_id, .. }
            | EventKind::PreemptionPoint { job_id, .. }
            | EventKind::TransferComplete { job_id, .. }
            | EventKind::EdgeTransferComplete { job_id, .. }
            | EventKind::DeadlineCheck { job_id, .. }
            | EventKind::BudgetOverrun { job_id, .. } => Some(*job_id),
            EventKind::TaskArrival { .. }
            | EventKind::BusArbitration { .. }
            | EventKind::SimulationEnd => None,
        }
    }

    fn task(&self, task_id: TaskId) -> Option<&Task> {
        self.task_registry.get(task_id.0 as usize)
    }

    fn mixed_criticality_lo_wcet(&self, task_id: TaskId) -> Option<Nanos> {
        let task = self.task(task_id)?;
        task.exec_times
            .iter()
            .find(|(dt, _)| *dt == DeviceType::Cpu)
            .or_else(|| task.exec_times.first())
            .and_then(|(_, model)| match model {
                ExecutionTimeModel::MixedCriticality { wcet_lo, .. } => Some(*wcet_lo),
                ExecutionTimeModel::Deterministic { .. }
                | ExecutionTimeModel::Stochastic { .. } => None,
            })
    }

    fn task_criticality_for_job(&self, job_id: JobId) -> Option<CriticalityLevel> {
        let job = self.get_job(job_id)?;
        self.task(job.task_id).map(|task| task.criticality)
    }

    /// Get current simulation time
    pub fn now(&self) -> Nanos {
        self.now
    }

    /// Get total events processed
    pub fn events_processed(&self) -> u64 {
        self.events_processed
    }

    /// Get metrics collector reference
    pub fn metrics(&self) -> &MetricsCollector {
        &self.metrics
    }

    /// Produce a summary of simulation results.
    pub fn summary(&self) -> SimResult {
        let m = &self.metrics;
        let per_device_busy: Vec<(DeviceId, Nanos)> = self
            .device_mgr
            .devices()
            .iter()
            .map(|device| (device.id, self.device_busy_time_ns[device.id.0 as usize]))
            .collect();
        let transfer_stats = self.transfer_mgr.stats();
        SimResult {
            total_jobs: m.total_jobs,
            completed_jobs: m.completed_jobs,
            deadline_misses: m.deadline_misses,
            miss_ratio: m.miss_ratio(),
            schedulable: m.is_schedulable(),
            makespan: m.makespan().unwrap_or(0),
            avg_response_time: m.avg_response_time().unwrap_or(0.0),
            per_device_utilization: m.per_device_utilization(&per_device_busy),
            transfer_overhead: transfer_stats.total_transfer_time_ns,
            blocking_breakdown: BlockingBreakdown {
                transfer_ns: transfer_stats.total_transfer_time_ns,
                migration_ns: transfer_stats.migration_transfer_time_ns,
                bus_wait_ns: transfer_stats.bus_wait_time_ns,
            },
            worst_response_time: m.worst_response_time().unwrap_or(0),
            preemption_count: self.preemption_count,
            migration_count: self.migration_count,
            bus_contention_ratio: transfer_stats.bus_contention_ratio(),
            events_processed: self.events_processed,
        }
    }

    pub fn write_trace_jsonl(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        self.metrics.write_jsonl(path)
    }
}

/// Aggregated result of a single simulation run.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SimResult {
    pub total_jobs: u64,
    pub completed_jobs: u64,
    pub deadline_misses: u64,
    pub miss_ratio: f64,
    pub schedulable: bool,
    pub makespan: Nanos,
    pub avg_response_time: f64,
    pub per_device_utilization: Vec<DeviceUtilization>,
    pub transfer_overhead: Nanos,
    pub blocking_breakdown: BlockingBreakdown,
    pub worst_response_time: Nanos,
    pub preemption_count: u64,
    pub migration_count: u64,
    pub bus_contention_ratio: f64,
    pub events_processed: u64,
}

fn sample_exec_time_for_device(
    task: &Task,
    device_type: DeviceType,
    level: CriticalityLevel,
    rng: &mut ChaCha8Rng,
) -> Option<Nanos> {
    let model = task
        .exec_times
        .iter()
        .find(|(dt, _)| *dt == device_type)
        .or_else(|| task.exec_times.first())
        .map(|(_, model)| model);

    match model {
        Some(ExecutionTimeModel::Deterministic { wcet }) => Some(*wcet),
        Some(ExecutionTimeModel::MixedCriticality { wcet_lo, wcet_hi }) => match level {
            CriticalityLevel::Lo => Some(*wcet_lo),
            CriticalityLevel::Hi => Some(*wcet_hi),
        },
        Some(ExecutionTimeModel::Stochastic {
            bcet,
            wcet,
            distribution,
        }) => {
            if wcet <= bcet {
                return Some(*bcet);
            }
            match distribution.to_ascii_lowercase().as_str() {
                "normal" => {
                    let mean = (*bcet as f64 + *wcet as f64) / 2.0;
                    let std = ((*wcet - *bcet) as f64 / 6.0).max(1.0);
                    let u1 = rng.gen_range(f64::EPSILON..1.0);
                    let u2 = rng.gen_range(0.0..1.0);
                    let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
                    let sampled = mean + z * std;
                    Some(sampled.clamp(*bcet as f64, *wcet as f64).round() as u64)
                }
                "lognormal" => {
                    let min_ln = (*bcet as f64).ln();
                    let max_ln = (*wcet as f64).ln();
                    let sampled_ln = rng.gen_range(min_ln..=max_ln);
                    Some(sampled_ln.exp().round().clamp(*bcet as f64, *wcet as f64) as u64)
                }
                _ => Some(rng.gen_range(*bcet..=*wcet)),
            }
        }
        None => None,
    }
}

fn work_to_wall(work_ns: Nanos, speed_factor: f64) -> Nanos {
    if speed_factor <= 0.0 {
        work_ns
    } else if work_ns == 0 {
        0
    } else {
        ((work_ns as f64) / speed_factor).ceil() as u64
    }
}

fn wall_to_work(wall_ns: Nanos, speed_factor: f64) -> Nanos {
    if speed_factor <= 0.0 {
        wall_ns
    } else if wall_ns == 0 {
        0
    } else {
        ((wall_ns as f64) * speed_factor).floor() as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cpu_device() -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(0),
            name: "CPU".to_string(),
            device_group: None,
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption: PreemptionModel::FullyPreemptive,
            context_switch_ns: 1_000,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        }
    }

    #[test]
    fn event_queue_ordering() {
        let config = SimConfig {
            duration_ns: 1_000_000,
            seed: 42,
        };
        let mut engine = SimEngine::new(config, vec![cpu_device()], vec![], vec![]);

        // Schedule events out of order
        engine.schedule_event(
            300,
            EventKind::TaskArrival {
                task_id: TaskId(2),
                job_id: JobId(2),
            },
        );
        engine.schedule_event(
            100,
            EventKind::TaskArrival {
                task_id: TaskId(0),
                job_id: JobId(0),
            },
        );
        engine.schedule_event(
            200,
            EventKind::TaskArrival {
                task_id: TaskId(1),
                job_id: JobId(1),
            },
        );

        // Pop should return in time order
        let e1 = engine.event_queue.pop().expect("missing event 1");
        let e2 = engine.event_queue.pop().expect("missing event 2");
        let e3 = engine.event_queue.pop().expect("missing event 3");
        assert_eq!(e1.time, 100);
        assert_eq!(e2.time, 200);
        assert_eq!(e3.time, 300);
    }

    #[test]
    fn sample_exec_mixed_criticality() {
        let task = Task {
            id: TaskId(0),
            name: "mc".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 100_000 },
            deadline: 100_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::MixedCriticality {
                    wcet_lo: 10_000,
                    wcet_hi: 20_000,
                },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        };

        let mut rng = ChaCha8Rng::seed_from_u64(1);
        assert_eq!(
            sample_exec_time_for_device(&task, DeviceType::Cpu, CriticalityLevel::Lo, &mut rng),
            Some(10_000)
        );
        assert_eq!(
            sample_exec_time_for_device(&task, DeviceType::Cpu, CriticalityLevel::Hi, &mut rng),
            Some(20_000)
        );
    }

    #[test]
    fn summary_exposes_expanded_metrics() {
        let mut engine = SimEngine::new(
            SimConfig {
                duration_ns: 1_000_000,
                seed: 9,
            },
            vec![
                cpu_device(),
                DeviceConfig {
                    id: DeviceId(1),
                    name: "GPU".to_string(),
                    device_group: None,
                    device_type: DeviceType::Gpu,
                    cores: 1,
                    preemption: PreemptionModel::LimitedPreemptive {
                        granularity_ns: 10_000,
                    },
                    context_switch_ns: 1_000,
                    speed_factor: 1.0,
                    multicore_policy: None,
                    power_watts: None,
                },
            ],
            vec![
                InterconnectConfig {
                    from: DeviceId(0),
                    to: DeviceId(1),
                    latency_ns: 100,
                    bandwidth_bytes_per_ns: 1.0,
                    shared_bus: Some(hprss_types::BusId(1)),
                    arbitration: hprss_types::BusArbitration::RoundRobin,
                },
                InterconnectConfig {
                    from: DeviceId(0),
                    to: DeviceId(2),
                    latency_ns: 100,
                    bandwidth_bytes_per_ns: 1.0,
                    shared_bus: Some(hprss_types::BusId(1)),
                    arbitration: hprss_types::BusArbitration::RoundRobin,
                },
            ],
            vec![SharedBusConfig {
                id: hprss_types::BusId(1),
                name: "sys".to_string(),
                total_bandwidth_bytes_per_ns: 1.0,
                arbitration: hprss_types::BusArbitration::RoundRobin,
            }],
        );

        engine.metrics.record_job_release();
        engine.metrics.record_job_release();
        engine
            .metrics
            .record_completion(JobId(0), TaskId(0), 0, 100);
        engine
            .metrics
            .record_completion(JobId(1), TaskId(1), 0, 200);
        engine.device_busy_time_ns = vec![80, 120];
        engine.preemption_count = 3;
        engine.migration_count = 2;

        let first = engine.transfer_mgr.initiate_transfer(
            JobId(1),
            DeviceId(0),
            DeviceId(1),
            100,
            JobTransferKind::Dispatch,
            0,
            1_000,
            1,
        );
        assert_eq!(first.len(), 1);
        let queued = engine.transfer_mgr.initiate_transfer(
            JobId(2),
            DeviceId(0),
            DeviceId(2),
            100,
            JobTransferKind::Migration,
            0,
            1_010,
            1,
        );
        assert!(queued.is_empty());
        let follow_up = engine
            .transfer_mgr
            .on_transfer_complete(JobId(1), first[0].time);
        assert_eq!(follow_up.len(), 1);

        let summary = engine.summary();
        assert_eq!(summary.worst_response_time, 200);
        assert_eq!(summary.preemption_count, 3);
        assert_eq!(summary.migration_count, 2);
        assert_eq!(
            summary.blocking_breakdown.transfer_ns,
            summary.transfer_overhead
        );
        assert!(summary.blocking_breakdown.migration_ns > 0);
        assert!(summary.blocking_breakdown.bus_wait_ns > 0);
        assert!(summary.bus_contention_ratio > 0.0);
        assert_eq!(summary.per_device_utilization.len(), 2);
        assert!((summary.per_device_utilization[0].utilization - 0.4).abs() < f64::EPSILON);
        assert!((summary.per_device_utilization[1].utilization - 0.6).abs() < f64::EPSILON);
    }
}
