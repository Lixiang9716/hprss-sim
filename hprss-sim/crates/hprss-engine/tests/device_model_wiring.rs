use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_types::{
    Action, CriticalityLevel, DeviceId, EventKind, Job, Nanos, Scheduler, SchedulerView, TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DeviceType, ExecutionTimeModel, Task},
};

struct BoundaryAwareScheduler {
    preempt_on_higher_priority: bool,
    preemption_points: Vec<Nanos>,
}

impl BoundaryAwareScheduler {
    fn new(preempt_on_higher_priority: bool) -> Self {
        Self {
            preempt_on_higher_priority,
            preemption_points: Vec::new(),
        }
    }

    fn select_device(task: &Task, view: &SchedulerView<'_>) -> DeviceId {
        view.devices
            .iter()
            .find(|d| d.device_type == task.affinity[0])
            .map(|d| d.id)
            .expect("expected compatible device")
    }

    fn maybe_preempt(&self, device_id: DeviceId, view: &SchedulerView<'_>) -> Option<Action> {
        if !self.preempt_on_higher_priority {
            return None;
        }
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref())?;
        let queued = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| q.first())?;
        (queued.priority < running.priority).then_some(Action::Preempt {
            victim: running.job_id,
            by: queued.job_id,
            device_id,
        })
    }
}

impl Scheduler for BoundaryAwareScheduler {
    fn name(&self) -> &str {
        "boundary-aware-test"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let device_id = Self::select_device(task, view);
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, running)| running.as_ref());

        if let Some(running) = running {
            if self.preempt_on_higher_priority && job.effective_priority < running.priority {
                return vec![Action::Preempt {
                    victim: running.job_id,
                    by: job.id,
                    device_id,
                }];
            }
            vec![Action::Enqueue {
                job_id: job.id,
                device_id,
            }]
        } else {
            vec![Action::Dispatch {
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
            Some(next) => vec![Action::Dispatch {
                job_id: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }

    fn on_preemption_point(
        &mut self,
        device_id: DeviceId,
        _running_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.preemption_points.push(view.now);
        self.maybe_preempt(device_id, view)
            .map_or_else(|| vec![Action::NoOp], |action| vec![action])
    }

    fn on_criticality_change(
        &mut self,
        _new_level: CriticalityLevel,
        _trigger_job: &Job,
        _view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        vec![Action::NoOp]
    }
}

fn run_single_device_two_job_scenario(
    device: DeviceConfig,
    low_exec_ns: Nanos,
    high_exec_ns: Nanos,
) -> (SimEngine, BoundaryAwareScheduler) {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 1_000,
            seed: 17,
        },
        vec![device.clone()],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![
        Task {
            id: TaskId(0),
            name: "low".to_string(),
            priority: 2,
            arrival: ArrivalModel::Aperiodic,
            deadline: 10_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(device.device_type, ExecutionTimeModel::Deterministic { wcet: low_exec_ns })],
            affinity: vec![device.device_type],
            data_size: 0,
        },
        Task {
            id: TaskId(1),
            name: "high".to_string(),
            priority: 1,
            arrival: ArrivalModel::Aperiodic,
            deadline: 10_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                device.device_type,
                ExecutionTimeModel::Deterministic { wcet: high_exec_ns },
            )],
            affinity: vec![device.device_type],
            data_size: 0,
        },
    ]);

    let low_job = engine.create_job(TaskId(0), 0, 10_000, Some(low_exec_ns), 2);
    engine.schedule_event(
        0,
        EventKind::TaskArrival {
            task_id: TaskId(0),
            job_id: low_job,
        },
    );
    let high_job = engine.create_job(TaskId(1), 10, 10_010, Some(high_exec_ns), 1);
    engine.schedule_event(
        10,
        EventKind::TaskArrival {
            task_id: TaskId(1),
            job_id: high_job,
        },
    );

    let mut scheduler = BoundaryAwareScheduler::new(true);
    engine.run(&mut scheduler);
    (engine, scheduler)
}

fn completion_time_for(engine: &SimEngine, task_id: TaskId) -> Nanos {
    engine
        .metrics()
        .completions
        .iter()
        .find(|c| c.task_id == task_id)
        .map(|c| c.completion_time)
        .expect("completion should exist")
}

#[test]
fn limited_preemption_is_deferred_until_boundary() {
    let (engine, _) = run_single_device_two_job_scenario(
        DeviceConfig {
            id: DeviceId(0),
            name: "gpu".to_string(),
            device_group: None,
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive { granularity_ns: 50 },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        },
        120,
        20,
    );

    assert_eq!(completion_time_for(&engine, TaskId(1)), 70);
    assert_eq!(engine.summary().preemption_count, 1);
}

#[test]
fn interrupt_isr_overhead_impacts_preempted_job_completion_time() {
    let (without_isr, _) = run_single_device_two_job_scenario(
        DeviceConfig {
            id: DeviceId(0),
            name: "dsp-no-isr".to_string(),
            device_group: None,
            device_type: DeviceType::Dsp,
            cores: 1,
            preemption: PreemptionModel::InterruptLevel {
                isr_overhead_ns: 0,
                dma_non_preemptive_ns: 50,
            },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        },
        120,
        20,
    );
    let (with_isr, _) = run_single_device_two_job_scenario(
        DeviceConfig {
            id: DeviceId(0),
            name: "dsp-isr".to_string(),
            device_group: None,
            device_type: DeviceType::Dsp,
            cores: 1,
            preemption: PreemptionModel::InterruptLevel {
                isr_overhead_ns: 30,
                dma_non_preemptive_ns: 50,
            },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        },
        120,
        20,
    );

    assert_eq!(
        completion_time_for(&with_isr, TaskId(1)) - completion_time_for(&without_isr, TaskId(1)),
        30
    );
}

#[test]
fn preemption_point_reschedules_until_job_finishes() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 500,
            seed: 99,
        },
        vec![DeviceConfig {
            id: DeviceId(0),
            name: "gpu".to_string(),
            device_group: None,
            device_type: DeviceType::Gpu,
            cores: 1,
            preemption: PreemptionModel::LimitedPreemptive { granularity_ns: 40 },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        }],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![Task {
        id: TaskId(0),
        name: "single".to_string(),
        priority: 1,
        arrival: ArrivalModel::Aperiodic,
        deadline: 10_000,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![(DeviceType::Gpu, ExecutionTimeModel::Deterministic { wcet: 130 })],
        affinity: vec![DeviceType::Gpu],
        data_size: 0,
    }]);
    let job_id = engine.create_job(TaskId(0), 0, 10_000, Some(130), 1);
    engine.schedule_event(
        0,
        EventKind::TaskArrival {
            task_id: TaskId(0),
            job_id,
        },
    );

    let mut scheduler = BoundaryAwareScheduler::new(false);
    engine.run(&mut scheduler);

    assert_eq!(scheduler.preemption_points, vec![40, 80, 120]);
}

#[test]
fn non_preemptive_dispatch_applies_reconfiguration_delay() {
    let (engine, _) = run_single_device_two_job_scenario(
        DeviceConfig {
            id: DeviceId(0),
            name: "fpga".to_string(),
            device_group: None,
            device_type: DeviceType::Fpga,
            cores: 1,
            preemption: PreemptionModel::NonPreemptive {
                reconfig_time_ns: 40,
            },
            context_switch_ns: 0,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        },
        100,
        20,
    );

    assert_eq!(completion_time_for(&engine, TaskId(0)), 140);
    assert_eq!(completion_time_for(&engine, TaskId(1)), 200);
    assert_eq!(engine.summary().preemption_count, 0);
}
