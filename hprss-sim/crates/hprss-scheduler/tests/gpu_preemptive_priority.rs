use hprss_scheduler::GpuPreemptivePriorityScheduler;
use hprss_types::{
    Action, CriticalityLevel, DeviceId, Job, JobId, JobState, Scheduler, SchedulerView, TaskId,
    device::{DeviceConfig, PreemptionModel},
    scheduler::{QueuedJobInfo, RunningJobInfo},
    task::{ArrivalModel, DeviceType, ExecutionTimeModel, Task},
};

fn gpu_device(id: u32, preemption: PreemptionModel) -> DeviceConfig {
    DeviceConfig {
        id: DeviceId(id),
        name: format!("gpu-{id}"),
        device_group: None,
        device_type: DeviceType::Gpu,
        cores: 1,
        preemption,
        context_switch_ns: 0,
        speed_factor: 1.0,
        multicore_policy: None,
        power_watts: None,
    }
}

fn gpu_task(id: u32, priority: u32, criticality: CriticalityLevel) -> Task {
    Task {
        id: TaskId(id),
        name: format!("t{id}"),
        priority,
        arrival: ArrivalModel::Aperiodic,
        deadline: 100_000,
        criticality,
        exec_times: vec![(
            DeviceType::Gpu,
            ExecutionTimeModel::Deterministic { wcet: 10_000 },
        )],
        affinity: vec![DeviceType::Gpu],
        data_size: 0,
    }
}

fn job(id: u64, priority: u32) -> Job {
    Job {
        id: JobId(id),
        task_id: TaskId(id as u32),
        state: JobState::Ready,
        version: 0,
        release_time: id * 10,
        absolute_deadline: 100_000,
        actual_exec_ns: Some(10_000),
        dag_provenance: None,
        executed_ns: 0,
        assigned_device: None,
        exec_start_time: None,
        effective_priority: priority,
    }
}

fn queued(
    id: u64,
    priority: u32,
    criticality: CriticalityLevel,
    release_time: u64,
) -> QueuedJobInfo {
    QueuedJobInfo {
        job_id: JobId(id),
        task_id: TaskId(id as u32),
        priority,
        release_time,
        absolute_deadline: 100_000,
        criticality,
    }
}

#[test]
fn arrival_on_limited_gpu_enqueues_high_priority_until_preemption_point() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let incoming_task = gpu_task(2, 1, CriticalityLevel::Hi);
    let incoming_job = job(2, 1);
    let running_job = job(1, 9);
    let devices = [gpu_device(
        0,
        PreemptionModel::LimitedPreemptive {
            granularity_ns: 5_000,
        },
    )];
    let view = SchedulerView {
        now: 100,
        devices: &devices,
        running_jobs: &[(
            DeviceId(0),
            Some(RunningJobInfo {
                job_id: running_job.id,
                task_id: running_job.task_id,
                priority: running_job.effective_priority,
                release_time: running_job.release_time,
                absolute_deadline: running_job.absolute_deadline,
                criticality: CriticalityLevel::Lo,
                elapsed_ns: 3_000,
            }),
        )],
        ready_queues: &[(DeviceId(0), vec![])],
        criticality_level: CriticalityLevel::Lo,
    };

    let arrival_actions = sched.on_job_arrival(&incoming_job, &incoming_task, &view);
    assert!(matches!(
        arrival_actions.as_slice(),
        [Action::Enqueue {
            job_id: JobId(2),
            device_id: DeviceId(0)
        }]
    ));

    let preemption_view = SchedulerView {
        now: 5_000,
        devices: &devices,
        running_jobs: view.running_jobs,
        ready_queues: &[(
            DeviceId(0),
            vec![queued(
                2,
                1,
                CriticalityLevel::Hi,
                incoming_job.release_time,
            )],
        )],
        criticality_level: CriticalityLevel::Lo,
    };

    let preempt_actions = sched.on_preemption_point(DeviceId(0), &running_job, &preemption_view);
    assert!(matches!(
        preempt_actions.as_slice(),
        [Action::Preempt {
            victim: JobId(1),
            by: JobId(2),
            device_id: DeviceId(0)
        }]
    ));
}

#[test]
fn completion_dispatches_highest_ranked_waiting_job_to_avoid_priority_inversion() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let finished_job = job(10, 10);
    let devices = [gpu_device(0, PreemptionModel::FullyPreemptive)];
    let view = SchedulerView {
        now: 1_000,
        devices: &devices,
        running_jobs: &[(DeviceId(0), None)],
        ready_queues: &[(
            DeviceId(0),
            vec![
                queued(20, 8, CriticalityLevel::Lo, 20),
                queued(21, 1, CriticalityLevel::Lo, 30),
                queued(22, 1, CriticalityLevel::Lo, 10),
            ],
        )],
        criticality_level: CriticalityLevel::Lo,
    };

    let actions = sched.on_job_complete(&finished_job, DeviceId(0), &view);
    assert!(matches!(
        actions.as_slice(),
        [Action::Dispatch {
            job_id: JobId(22),
            device_id: DeviceId(0)
        }]
    ));
}

#[test]
fn arrival_on_fully_preemptive_gpu_preempts_lower_priority_running_job() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let incoming_task = gpu_task(3, 2, CriticalityLevel::Lo);
    let incoming_job = job(3, 2);
    let devices = [gpu_device(0, PreemptionModel::FullyPreemptive)];
    let view = SchedulerView {
        now: 100,
        devices: &devices,
        running_jobs: &[(
            DeviceId(0),
            Some(RunningJobInfo {
                job_id: JobId(1),
                task_id: TaskId(1),
                priority: 9,
                release_time: 0,
                absolute_deadline: 100_000,
                criticality: CriticalityLevel::Lo,
                elapsed_ns: 1_000,
            }),
        )],
        ready_queues: &[(DeviceId(0), vec![])],
        criticality_level: CriticalityLevel::Lo,
    };

    let actions = sched.on_job_arrival(&incoming_job, &incoming_task, &view);
    assert!(matches!(
        actions.as_slice(),
        [Action::Preempt {
            victim: JobId(1),
            by: JobId(3),
            device_id: DeviceId(0)
        }]
    ));
}

#[test]
fn arrival_equal_priority_does_not_preempt_running_gpu_job() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let incoming_task = gpu_task(4, 5, CriticalityLevel::Lo);
    let incoming_job = job(4, 5);
    let devices = [gpu_device(0, PreemptionModel::FullyPreemptive)];
    let view = SchedulerView {
        now: 100,
        devices: &devices,
        running_jobs: &[(
            DeviceId(0),
            Some(RunningJobInfo {
                job_id: JobId(1),
                task_id: TaskId(1),
                priority: 5,
                release_time: 1,
                absolute_deadline: 100_000,
                criticality: CriticalityLevel::Lo,
                elapsed_ns: 1_000,
            }),
        )],
        ready_queues: &[(DeviceId(0), vec![])],
        criticality_level: CriticalityLevel::Lo,
    };

    let actions = sched.on_job_arrival(&incoming_job, &incoming_task, &view);
    assert!(matches!(
        actions.as_slice(),
        [Action::Enqueue {
            job_id: JobId(4),
            device_id: DeviceId(0)
        }]
    ));
}

#[test]
fn arrival_preempts_worst_running_job_with_numeric_rank_tie_breakers() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let incoming_task = gpu_task(7, 3, CriticalityLevel::Lo);
    let incoming_job = job(7, 3);
    let devices = [
        gpu_device(0, PreemptionModel::FullyPreemptive),
        gpu_device(1, PreemptionModel::FullyPreemptive),
    ];
    let view = SchedulerView {
        now: 1_000,
        devices: &devices,
        running_jobs: &[
            (
                DeviceId(0),
                Some(RunningJobInfo {
                    job_id: JobId(10),
                    task_id: TaskId(10),
                    priority: 9,
                    release_time: 0,
                    absolute_deadline: 100_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            ),
            (
                DeviceId(1),
                Some(RunningJobInfo {
                    job_id: JobId(11),
                    task_id: TaskId(11),
                    priority: 9,
                    release_time: 0,
                    absolute_deadline: 100_000,
                    criticality: CriticalityLevel::Lo,
                    elapsed_ns: 100,
                }),
            ),
        ],
        ready_queues: &[(DeviceId(0), vec![]), (DeviceId(1), vec![])],
        criticality_level: CriticalityLevel::Lo,
    };

    let actions = sched.on_job_arrival(&incoming_job, &incoming_task, &view);
    assert!(
        matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(11),
                by: JobId(7),
                device_id: DeviceId(1)
            }]
        ),
        "unexpected actions: {actions:?}"
    );
}

#[test]
fn preemption_point_uses_running_snapshot_for_victim_id() {
    let mut sched = GpuPreemptivePriorityScheduler;
    let running_job_arg = job(99, 9);
    let devices = [gpu_device(
        0,
        PreemptionModel::LimitedPreemptive {
            granularity_ns: 5_000,
        },
    )];
    let view = SchedulerView {
        now: 1_000,
        devices: &devices,
        running_jobs: &[(
            DeviceId(0),
            Some(RunningJobInfo {
                job_id: JobId(1),
                task_id: TaskId(1),
                priority: 9,
                release_time: 0,
                absolute_deadline: 100_000,
                criticality: CriticalityLevel::Lo,
                elapsed_ns: 4_000,
            }),
        )],
        ready_queues: &[(DeviceId(0), vec![queued(2, 1, CriticalityLevel::Hi, 10)])],
        criticality_level: CriticalityLevel::Lo,
    };

    let actions = sched.on_preemption_point(DeviceId(0), &running_job_arg, &view);
    assert!(
        matches!(
            actions.as_slice(),
            [Action::Preempt {
                victim: JobId(1),
                by: JobId(2),
                device_id: DeviceId(0)
            }]
        ),
        "unexpected actions: {actions:?}"
    );
}
