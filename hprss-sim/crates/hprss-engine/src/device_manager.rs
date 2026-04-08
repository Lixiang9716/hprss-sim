//! Device state manager.
//!
//! Tracks per-device running jobs and ready queues.
//! Constructs [`SchedulerView`] snapshots for scheduler callbacks.
//!
//! Performance: uses BTreeMap ready queues for O(log K) insert / O(1) dequeue,
//! and dirty flags to skip rebuilding unchanged device views.

use std::collections::{BTreeMap, VecDeque};

use hprss_types::{
    CriticalityLevel, DeviceId, JobId, Nanos, TaskId,
    device::DeviceConfig,
    job::Job,
    scheduler::{QueuedJobInfo, RunningJobInfo, SchedulerView},
    task::{DeviceType, Task},
};

pub struct DeviceManager {
    devices: Vec<DeviceConfig>,
    /// Per-device: currently running job ID (indexed by `DeviceId.0`)
    running: Vec<Option<JobId>>,
    /// Per-device priority queue: priority → FIFO queue of job IDs.
    /// BTreeMap keeps keys in ascending order (lower number = higher priority).
    ready_queues: Vec<BTreeMap<u32, VecDeque<JobId>>>,
    // Pre-allocated scratch buffers for SchedulerView
    view_running: Vec<(DeviceId, Option<RunningJobInfo>)>,
    view_queues: Vec<(DeviceId, Vec<QueuedJobInfo>)>,
    /// Per-device dirty flag: true when the queue has been mutated since last rebuild
    queue_dirty: Vec<bool>,
}

impl DeviceManager {
    pub fn new(devices: Vec<DeviceConfig>) -> Self {
        let n = devices.len();
        let view_running = devices.iter().map(|d| (d.id, None)).collect();
        let view_queues = devices.iter().map(|d| (d.id, Vec::new())).collect();
        Self {
            devices,
            running: vec![None; n],
            ready_queues: (0..n).map(|_| BTreeMap::new()).collect(),
            view_running,
            view_queues,
            queue_dirty: vec![true; n],
        }
    }

    pub fn device(&self, id: DeviceId) -> &DeviceConfig {
        &self.devices[id.0 as usize]
    }

    pub fn devices(&self) -> &[DeviceConfig] {
        &self.devices
    }

    pub fn device_for_type(&self, dt: DeviceType) -> Option<&DeviceConfig> {
        self.devices.iter().find(|d| d.device_type == dt)
    }

    pub fn running_job(&self, device_id: DeviceId) -> Option<JobId> {
        self.running[device_id.0 as usize]
    }

    pub fn set_running(&mut self, device_id: DeviceId, job_id: JobId) {
        self.running[device_id.0 as usize] = Some(job_id);
    }

    pub fn clear_running(&mut self, device_id: DeviceId) {
        self.running[device_id.0 as usize] = None;
    }

    /// Insert a job into the device's ready queue. O(log K) where K = distinct priorities.
    pub fn enqueue(&mut self, device_id: DeviceId, job_id: JobId, priority: u32) {
        let idx = device_id.0 as usize;
        self.ready_queues[idx]
            .entry(priority)
            .or_default()
            .push_back(job_id);
        self.queue_dirty[idx] = true;
    }

    /// Remove and return the highest-priority (lowest number) job. O(log K) amortized.
    pub fn dequeue(&mut self, device_id: DeviceId) -> Option<JobId> {
        let idx = device_id.0 as usize;
        let queue = &mut self.ready_queues[idx];
        loop {
            let mut entry = queue.first_entry()?;
            if let Some(job_id) = entry.get_mut().pop_front() {
                if entry.get().is_empty() {
                    entry.remove();
                }
                self.queue_dirty[idx] = true;
                return Some(job_id);
            }
            // Empty bucket left by remove_from_queue — clean it up
            entry.remove();
        }
    }

    /// Remove a specific job from a device's ready queue.
    pub fn remove_from_queue(&mut self, device_id: DeviceId, job_id: JobId) -> bool {
        let idx = device_id.0 as usize;
        let mut found = false;
        for (_prio, deque) in self.ready_queues[idx].iter_mut() {
            if let Some(pos) = deque.iter().position(|&id| id == job_id) {
                deque.remove(pos);
                found = true;
                break;
            }
        }
        if found {
            self.ready_queues[idx].retain(|_prio, deque| !deque.is_empty());
            self.queue_dirty[idx] = true;
        }
        found
    }

    /// Remove a job from ALL device queues (used for `DropJob`).
    pub fn remove_job_from_all_queues(&mut self, job_id: JobId) {
        for (idx, queue) in self.ready_queues.iter_mut().enumerate() {
            for (_prio, deque) in queue.iter_mut() {
                if let Some(pos) = deque.iter().position(|&id| id == job_id) {
                    deque.remove(pos);
                    self.queue_dirty[idx] = true;
                    break;
                }
            }
        }
        // Clean up any empty priority buckets
        for queue in &mut self.ready_queues {
            queue.retain(|_prio, deque| !deque.is_empty());
        }
    }

    /// Populate `view_running` and `view_queues` from current state.
    ///
    /// Running info is always updated (cheap: 1 entry per device).
    /// Queue view is only rebuilt for devices whose queue has been mutated (dirty flag).
    pub fn rebuild_view_data(&mut self, now: Nanos, jobs: &[Option<Job>], tasks: &[Task]) {
        for i in 0..self.devices.len() {
            // Always update running info (cheap — one entry per device, and
            // elapsed_ns depends on `now` which changes every event)
            self.view_running[i].1 = self.running[i].and_then(|job_id| {
                let job = jobs.get(job_id.0 as usize)?.as_ref()?;
                let task = &tasks[job.task_id.0 as usize];
                let elapsed = job
                    .exec_start_time
                    .map_or(0, |start| now.saturating_sub(start));
                Some(RunningJobInfo {
                    job_id: job.id,
                    task_id: job.task_id,
                    priority: job.effective_priority,
                    release_time: job.release_time,
                    absolute_deadline: job.absolute_deadline,
                    criticality: task.criticality,
                    elapsed_ns: elapsed,
                })
            });

            // Only rebuild queue view when dirty (Vec::clear keeps the allocation)
            if self.queue_dirty[i] {
                self.queue_dirty[i] = false;
                self.view_queues[i].1.clear();
                for deque in self.ready_queues[i].values() {
                    for &job_id in deque {
                        if let Some(job) = jobs.get(job_id.0 as usize).and_then(|o| o.as_ref()) {
                            let task = &tasks[job.task_id.0 as usize];
                            self.view_queues[i].1.push(QueuedJobInfo {
                                job_id: job.id,
                                task_id: job.task_id,
                                priority: job.effective_priority,
                                release_time: job.release_time,
                                absolute_deadline: job.absolute_deadline,
                                criticality: task.criticality,
                            });
                        }
                    }
                }
            }
        }
    }

    /// Return a [`SchedulerView`] borrowing from the internal scratch buffers.
    pub fn scheduler_view(&self, now: Nanos, criticality: CriticalityLevel) -> SchedulerView<'_> {
        SchedulerView {
            now,
            devices: &self.devices,
            running_jobs: &self.view_running,
            ready_queues: &self.view_queues,
            criticality_level: criticality,
        }
    }
}

// Suppress the unused‐import lint for `TaskId` — it is used only in tests
// but the import is grouped here for documentation clarity.
const _: () = {
    fn _use_task_id(_: TaskId) {}
};

#[cfg(test)]
mod tests {
    use super::*;
    use hprss_types::{
        device::PreemptionModel,
        task::{ArrivalModel, ExecutionTimeModel},
    };

    fn make_device(id: u32) -> DeviceConfig {
        DeviceConfig {
            id: DeviceId(id),
            name: format!("cpu{id}"),
            device_group: None,
            device_type: DeviceType::Cpu,
            cores: 1,
            preemption: PreemptionModel::FullyPreemptive,
            context_switch_ns: 10_000,
            speed_factor: 1.0,
            multicore_policy: None,
            power_watts: None,
        }
    }

    #[test]
    fn test_enqueue_priority_order() {
        let mut dm = DeviceManager::new(vec![make_device(0)]);
        let dev = DeviceId(0);

        dm.enqueue(dev, JobId(10), 3);
        dm.enqueue(dev, JobId(11), 1);
        dm.enqueue(dev, JobId(12), 2);

        assert_eq!(dm.dequeue(dev), Some(JobId(11))); // priority 1
        assert_eq!(dm.dequeue(dev), Some(JobId(12))); // priority 2
        assert_eq!(dm.dequeue(dev), Some(JobId(10))); // priority 3
        assert_eq!(dm.dequeue(dev), None);
    }

    #[test]
    fn test_set_clear_running() {
        let mut dm = DeviceManager::new(vec![make_device(0)]);
        let dev = DeviceId(0);

        assert_eq!(dm.running_job(dev), None);
        dm.set_running(dev, JobId(42));
        assert_eq!(dm.running_job(dev), Some(JobId(42)));
        dm.clear_running(dev);
        assert_eq!(dm.running_job(dev), None);
    }

    #[test]
    fn test_remove_from_queue() {
        let mut dm = DeviceManager::new(vec![make_device(0)]);
        let dev = DeviceId(0);

        dm.enqueue(dev, JobId(1), 1);
        dm.enqueue(dev, JobId(2), 2);
        dm.enqueue(dev, JobId(3), 3);

        assert!(dm.remove_from_queue(dev, JobId(2)));
        assert!(!dm.remove_from_queue(dev, JobId(99)));

        assert_eq!(dm.dequeue(dev), Some(JobId(1)));
        assert_eq!(dm.dequeue(dev), Some(JobId(3)));
        assert_eq!(dm.dequeue(dev), None);
    }

    #[test]
    fn test_rebuild_view_data() {
        let mut dm = DeviceManager::new(vec![make_device(0), make_device(1)]);

        // Running job on device 0, queued job on device 1
        dm.set_running(DeviceId(0), JobId(0));
        dm.enqueue(DeviceId(1), JobId(1), 2);

        let mut job0 = Job::new(JobId(0), TaskId(0), 0, 10_000_000, Some(3_000_000), 1);
        job0.exec_start_time = Some(500_000);

        let job1 = Job::new(JobId(1), TaskId(0), 0, 10_000_000, Some(3_000_000), 2);

        let jobs: Vec<Option<Job>> = vec![Some(job0), Some(job1)];

        let task = Task {
            id: TaskId(0),
            name: "test".into(),
            priority: 1,
            arrival: ArrivalModel::Periodic { period: 10_000_000 },
            deadline: 10_000_000,
            criticality: CriticalityLevel::Hi,
            exec_times: vec![(
                DeviceType::Cpu,
                ExecutionTimeModel::Deterministic { wcet: 3_000_000 },
            )],
            affinity: vec![DeviceType::Cpu],
            data_size: 0,
        };
        let tasks = vec![task];

        let now = 1_000_000;
        dm.rebuild_view_data(now, &jobs, &tasks);

        let view = dm.scheduler_view(now, CriticalityLevel::Lo);

        // Device 0: running job with elapsed = now - exec_start_time
        assert_eq!(view.running_jobs[0].0, DeviceId(0));
        let info = view.running_jobs[0].1.as_ref().unwrap();
        assert_eq!(info.job_id, JobId(0));
        assert_eq!(info.priority, 1);
        assert_eq!(info.elapsed_ns, 500_000);
        assert_eq!(info.criticality, CriticalityLevel::Hi);

        // Device 1: no running job
        assert_eq!(view.running_jobs[1].0, DeviceId(1));
        assert!(view.running_jobs[1].1.is_none());

        // Device 0: empty queue
        assert!(view.ready_queues[0].1.is_empty());

        // Device 1: one queued job
        assert_eq!(view.ready_queues[1].1.len(), 1);
        let queued = &view.ready_queues[1].1[0];
        assert_eq!(queued.job_id, JobId(1));
        assert_eq!(queued.priority, 2);
        assert_eq!(queued.criticality, CriticalityLevel::Hi);
    }
}
