use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_types::{
    Action, CriticalityLevel, DeviceId, EventKind, Job, Nanos, ReevaluationPolicy,
    ReevaluationTrigger, Scheduler, SchedulerView, TaskId,
    device::{DeviceConfig, PreemptionModel},
    task::{ArrivalModel, DeviceType, ExecutionTimeModel, Task},
};

fn cpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("cpu-{id}"),
        device_group: None,
        device_type: DeviceType::Cpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

struct ReevaluatingScheduler {
    policy: ReevaluationPolicy,
    preempt_on_reevaluation: bool,
    reevaluation_log: Vec<(ReevaluationTrigger, Nanos)>,
}

impl ReevaluatingScheduler {
    fn new(policy: ReevaluationPolicy) -> Self {
        Self {
            policy,
            preempt_on_reevaluation: true,
            reevaluation_log: Vec::new(),
        }
    }

    fn with_preemption(mut self, enabled: bool) -> Self {
        self.preempt_on_reevaluation = enabled;
        self
    }
}

impl Scheduler for ReevaluatingScheduler {
    fn name(&self) -> &str {
        "reevaluating-test"
    }

    fn reevaluation_policy(&self) -> ReevaluationPolicy {
        self.policy
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let device_id = view
            .devices
            .iter()
            .find(|d| d.device_type == task.affinity[0])
            .map(|d| d.id)
            .expect("expected compatible device");

        let running = view
            .running_jobs
            .iter()
            .find(|(id, _)| *id == device_id)
            .and_then(|(_, running)| running.as_ref());

        if running.is_some() {
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
            .find(|(id, _)| *id == device_id)
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
        _device_id: DeviceId,
        _running_job: &Job,
        _view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        vec![Action::NoOp]
    }

    fn on_criticality_change(
        &mut self,
        _new_level: CriticalityLevel,
        _trigger_job: &Job,
        _view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        vec![Action::NoOp]
    }

    fn on_reevaluation(
        &mut self,
        trigger: ReevaluationTrigger,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        self.reevaluation_log.push((trigger, view.now));
        if !self.preempt_on_reevaluation {
            return vec![Action::NoOp];
        }

        let Some((device_id, running)) = view
            .running_jobs
            .iter()
            .find_map(|(did, running)| running.as_ref().map(|r| (*did, r)))
        else {
            return vec![Action::NoOp];
        };

        let queued = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, q)| q.first());

        match queued {
            Some(next) => vec![Action::Preempt {
                victim: running.job_id,
                by: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }
}

#[test]
fn reevaluation_callback_is_triggered() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 50_000,
            seed: 11,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    engine.register_tasks(vec![Task {
        id: TaskId(0),
        name: "job0".to_string(),
        priority: 1,
        arrival: ArrivalModel::Periodic { period: 1_000_000 },
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![(
            DeviceType::Cpu,
            ExecutionTimeModel::Deterministic { wcet: 30_000 },
        )],
        affinity: vec![DeviceType::Cpu],
        data_size: 0,
    }]);
    engine.schedule_initial_arrivals();

    let mut scheduler =
        ReevaluatingScheduler::new(ReevaluationPolicy::Periodic { interval_ns: 5_000 });
    engine.run(&mut scheduler);

    assert!(
        !scheduler.reevaluation_log.is_empty(),
        "periodic reevaluation callback should be invoked"
    );
}

#[test]
fn reevaluation_actions_are_executed() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 200_000,
            seed: 12,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    engine.register_tasks(vec![
        Task {
            id: TaskId(0),
            name: "long".to_string(),
            priority: 2,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 120_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
        Task {
            id: TaskId(1),
            name: "short".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 10_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
    ]);
    engine.schedule_initial_arrivals();

    let mut scheduler =
        ReevaluatingScheduler::new(ReevaluationPolicy::Periodic { interval_ns: 5_000 });
    engine.run(&mut scheduler);

    assert!(engine.summary().preemption_count > 0);
    assert_eq!(engine.metrics().completed_jobs, 2);
}

#[test]
fn triggered_policy_debounces_burst_events() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 80_000,
            seed: 77,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    engine.register_tasks(vec![
        Task {
            id: TaskId(0),
            name: "burst-a".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 8_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
        Task {
            id: TaskId(1),
            name: "burst-b".to_string(),
            priority: 2,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 1_000_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 8_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
    ]);
    engine.schedule_initial_arrivals();

    let min_interval_ns = 10_000;
    let mut scheduler =
        ReevaluatingScheduler::new(ReevaluationPolicy::Triggered { min_interval_ns })
            .with_preemption(false);
    engine.run(&mut scheduler);

    let triggered_times: Vec<Nanos> = scheduler
        .reevaluation_log
        .iter()
        .filter_map(|(trigger, ts)| (*trigger != ReevaluationTrigger::Periodic).then_some(*ts))
        .collect();

    assert!(!triggered_times.is_empty());
    assert_eq!(triggered_times[0], min_interval_ns);
    assert!(
        triggered_times
            .windows(2)
            .all(|w| w[1].saturating_sub(w[0]) >= min_interval_ns),
        "triggered reevaluation callbacks must respect debounce interval"
    );
}

#[test]
fn hybrid_policy_periodic_cadence_is_stable_with_triggered_callbacks() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 35_000,
            seed: 88,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    engine.register_tasks(vec![Task {
        id: TaskId(0),
        name: "hybrid".to_string(),
        priority: 1,
        arrival: ArrivalModel::Periodic { period: 1_000_000 },
        deadline: 1_000_000,
        criticality: CriticalityLevel::Lo,
        exec_times: vec![(
            DeviceType::Cpu,
            ExecutionTimeModel::Deterministic { wcet: 100_000 },
        )],
        affinity: vec![DeviceType::Cpu],
        data_size: 0,
    }]);
    engine.schedule_initial_arrivals();

    let mut scheduler = ReevaluatingScheduler::new(ReevaluationPolicy::Hybrid {
        interval_ns: 10_000,
        min_interval_ns: 3_000,
    })
    .with_preemption(false);
    engine.run(&mut scheduler);

    let periodic_times: Vec<Nanos> = scheduler
        .reevaluation_log
        .iter()
        .filter_map(|(trigger, ts)| (*trigger == ReevaluationTrigger::Periodic).then_some(*ts))
        .collect();
    let triggered_times: Vec<Nanos> = scheduler
        .reevaluation_log
        .iter()
        .filter_map(|(trigger, ts)| (*trigger != ReevaluationTrigger::Periodic).then_some(*ts))
        .collect();

    assert_eq!(periodic_times, vec![10_000, 20_000, 30_000]);
    assert_eq!(triggered_times, vec![3_000]);
}

#[test]
fn stale_reevaluation_event_is_ignored() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 25_000,
            seed: 99,
        },
        vec![cpu_device(0)],
        vec![],
        vec![],
    );

    let mut scheduler = ReevaluatingScheduler::new(ReevaluationPolicy::Periodic {
        interval_ns: 10_000,
    })
    .with_preemption(false);

    engine.schedule_event(
        5_000,
        EventKind::SchedulerReevaluation {
            generation: 999,
            trigger: ReevaluationTrigger::Periodic,
        },
    );

    engine.run(&mut scheduler);

    let times: Vec<Nanos> = scheduler
        .reevaluation_log
        .iter()
        .map(|(_, ts)| *ts)
        .collect();
    assert_eq!(times, vec![10_000, 20_000]);
}
