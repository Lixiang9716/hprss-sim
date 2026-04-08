use hprss_engine::engine::{SimConfig, SimEngine};
use hprss_scheduler::LlfScheduler;
use hprss_types::{
    Action, CriticalityLevel, DeviceId, EventKind, Job, JobId, JobState, Scheduler, SchedulerView,
    TaskId,
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

fn gpu_device(id: u32) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("gpu-{id}"),
        device_group: None,
        device_type: DeviceType::Gpu,
        cores: 1,
        preemption: PreemptionModel::FullyPreemptive,
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

#[derive(Default)]
struct DropLoOnModeSwitchScheduler;

impl DropLoOnModeSwitchScheduler {
    fn select_device(task: &Task, view: &SchedulerView<'_>) -> Option<DeviceId> {
        task.affinity.iter().find_map(|dt| {
            view.devices
                .iter()
                .find(|d| d.device_type == *dt)
                .map(|d| d.id)
        })
    }
}

impl Scheduler for DropLoOnModeSwitchScheduler {
    fn name(&self) -> &str {
        "drop-lo-mode-switch-test"
    }

    fn on_job_arrival(&mut self, job: &Job, task: &Task, view: &SchedulerView<'_>) -> Vec<Action> {
        let Some(device_id) = Self::select_device(task, view) else {
            return vec![Action::NoOp];
        };
        let running = view
            .running_jobs
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, info)| info.as_ref());
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
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| queue.first());
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
        new_level: CriticalityLevel,
        _trigger_job: &Job,
        view: &SchedulerView<'_>,
    ) -> Vec<Action> {
        if new_level != CriticalityLevel::Hi {
            return vec![Action::NoOp];
        }
        let mut drops = Vec::new();
        for (_, running) in view.running_jobs {
            if let Some(running) = running
                && running.criticality == CriticalityLevel::Lo
            {
                drops.push(Action::DropJob {
                    job_id: running.job_id,
                });
            }
        }
        for (_, queue) in view.ready_queues {
            for queued in queue {
                if queued.criticality == CriticalityLevel::Lo {
                    drops.push(Action::DropJob {
                        job_id: queued.job_id,
                    });
                }
            }
        }
        if drops.is_empty() {
            vec![Action::NoOp]
        } else {
            drops
        }
    }

    fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
        let next = view
            .ready_queues
            .iter()
            .find(|(did, _)| *did == device_id)
            .and_then(|(_, queue)| queue.first());
        match next {
            Some(next) => vec![Action::Dispatch {
                job_id: next.job_id,
                device_id,
            }],
            None => vec![Action::NoOp],
        }
    }
}

#[test]
fn criticality_switch_immediately_dispatches_hi_jobs_on_newly_idle_devices() {
    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 100_000,
            seed: 99,
        },
        vec![cpu_device(0), gpu_device(1)],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![
        Task {
            id: TaskId(0),
            name: "lo-gpu-runner".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 100_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 70_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        },
        Task {
            id: TaskId(1),
            name: "hi-trigger-cpu".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 100_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 30_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
        Task {
            id: TaskId(2),
            name: "hi-gpu-waiting".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 100_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 15_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        },
    ]);
    engine.schedule_initial_arrivals();
    engine.schedule_event(
        5_000,
        EventKind::BudgetOverrun {
            job_id: JobId(1),
            expected_version: 1,
        },
    );

    let mut scheduler = DropLoOnModeSwitchScheduler;
    engine.run(&mut scheduler);

    let summary = engine.summary();
    assert_eq!(summary.deadline_misses, 0);
    assert_eq!(summary.completed_jobs, 2);

    let hi_gpu = engine
        .get_job(JobId(2))
        .expect("HI GPU waiting job should exist after release");
    assert_eq!(hi_gpu.state, JobState::Completed);

    let lo_gpu = engine
        .get_job(JobId(0))
        .expect("LO GPU runner should exist after release");
    assert_eq!(lo_gpu.state, JobState::Dropped);
}

#[test]
fn llf_criticality_switch_redispatch_does_not_corrupt_running_job_state() {
    struct GuardedLlfScheduler {
        inner: LlfScheduler,
    }

    impl Default for GuardedLlfScheduler {
        fn default() -> Self {
            Self {
                inner: LlfScheduler::default(),
            }
        }
    }

    impl Scheduler for GuardedLlfScheduler {
        fn name(&self) -> &str {
            self.inner.name()
        }

        fn reevaluation_policy(&self) -> hprss_types::ReevaluationPolicy {
            self.inner.reevaluation_policy()
        }

        fn on_job_arrival(
            &mut self,
            job: &Job,
            task: &Task,
            view: &SchedulerView<'_>,
        ) -> Vec<Action> {
            self.inner.on_job_arrival(job, task, view)
        }

        fn on_job_complete(
            &mut self,
            job: &Job,
            device_id: DeviceId,
            view: &SchedulerView<'_>,
        ) -> Vec<Action> {
            assert_eq!(
                job.state,
                JobState::Completed,
                "on_job_complete must only be called for truly completed jobs"
            );
            self.inner.on_job_complete(job, device_id, view)
        }

        fn on_preemption_point(
            &mut self,
            device_id: DeviceId,
            running_job: &Job,
            view: &SchedulerView<'_>,
        ) -> Vec<Action> {
            self.inner.on_preemption_point(device_id, running_job, view)
        }

        fn on_criticality_change(
            &mut self,
            new_level: CriticalityLevel,
            trigger_job: &Job,
            view: &SchedulerView<'_>,
        ) -> Vec<Action> {
            self.inner
                .on_criticality_change(new_level, trigger_job, view)
        }

        fn on_device_idle(&mut self, device_id: DeviceId, view: &SchedulerView<'_>) -> Vec<Action> {
            self.inner.on_device_idle(device_id, view)
        }

        fn on_reevaluation(
            &mut self,
            trigger: hprss_types::ReevaluationTrigger,
            view: &SchedulerView<'_>,
        ) -> Vec<Action> {
            self.inner.on_reevaluation(trigger, view)
        }
    }

    let mut engine = SimEngine::new(
        SimConfig {
            duration_ns: 120_000,
            seed: 123,
        },
        vec![cpu_device(0), gpu_device(1)],
        vec![],
        vec![],
    );
    engine.register_tasks(vec![
        Task {
            id: TaskId(0),
            name: "lo-gpu-runner".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 120_000,
            criticality: CriticalityLevel::Lo,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 70_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        },
        Task {
            id: TaskId(1),
            name: "hi-trigger-cpu".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 120_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::MixedCriticality {
                    wcet_lo: 10_000,
                    wcet_hi: 60_000,
                },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
        Task {
            id: TaskId(2),
            name: "hi-gpu-waiting".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 120_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Gpu,
                ExecutionTimeModel::Deterministic { wcet: 15_000 },
            )],
            affinity: vec![DeviceType::Gpu],
            data_size: 0,
        },
        Task {
            id: TaskId(3),
            name: "hi-cpu-queued".to_string(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 1_000_000 },
            deadline: 120_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 30_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        },
    ]);
    engine.schedule_initial_arrivals();
    engine.schedule_event(
        5_000,
        EventKind::BudgetOverrun {
            job_id: JobId(1),
            expected_version: 1,
        },
    );

    let mut scheduler = GuardedLlfScheduler::default();
    engine.run(&mut scheduler);

    let summary = engine.summary();
    assert_eq!(summary.deadline_misses, 0);
    assert_eq!(summary.completed_jobs, 3);
    assert_eq!(
        engine.get_job(JobId(2)).map(|job| job.state),
        Some(JobState::Completed)
    );
}
