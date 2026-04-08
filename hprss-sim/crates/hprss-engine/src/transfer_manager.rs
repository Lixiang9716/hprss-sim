//! Interconnect and data transfer manager.
//!
//! Handles data transfers between devices, shared bus arbitration,
//! and transfer time calculation.

use std::collections::{HashMap, VecDeque};

use hprss_types::{
    BusArbitration, BusId, DeviceId, EventKind, InterconnectConfig, JobId, Nanos, SharedBusConfig,
};

/// An event that should be scheduled by the engine.
pub struct ScheduledEvent {
    pub time: Nanos,
    pub kind: EventKind,
}

struct ActiveTransfer {
    job_id: JobId,
    bus_id: Option<BusId>,
}

struct PendingTransfer {
    job_id: JobId,
    target_device: DeviceId,
    data_size: u64,
    priority: u32,
    expected_version: u64,
}

/// Manages data transfers between devices over interconnect links and shared buses.
pub struct TransferManager {
    interconnects: Vec<InterconnectConfig>,
    buses: Vec<SharedBusConfig>,
    active_transfers: Vec<ActiveTransfer>,
    pending: HashMap<BusId, VecDeque<PendingTransfer>>,
}

impl TransferManager {
    /// Create a new transfer manager with the given interconnect links and bus definitions.
    pub fn new(interconnects: Vec<InterconnectConfig>, buses: Vec<SharedBusConfig>) -> Self {
        let mut pending = HashMap::new();
        for bus in &buses {
            pending.insert(bus.id, VecDeque::new());
        }
        Self {
            interconnects,
            buses,
            active_transfers: Vec::new(),
            pending,
        }
    }

    /// Find the interconnect link between two devices (bidirectional lookup).
    pub fn find_interconnect(&self, from: DeviceId, to: DeviceId) -> Option<&InterconnectConfig> {
        self.interconnects
            .iter()
            .find(|ic| (ic.from == from && ic.to == to) || (ic.from == to && ic.to == from))
    }

    /// Calculate the transfer time for a given data size over an interconnect.
    ///
    /// `transfer_ns = latency_ns + ceil(data_size / bandwidth_bytes_per_ns)`
    pub fn calculate_transfer_time(ic: &InterconnectConfig, data_size: u64) -> Nanos {
        if ic.bandwidth_bytes_per_ns == 0.0 {
            return ic.latency_ns;
        }
        let transfer_cycles = (data_size as f64 / ic.bandwidth_bytes_per_ns).ceil() as u64;
        ic.latency_ns + transfer_cycles
    }

    /// Initiate a data transfer from one device to another.
    ///
    /// Returns scheduled events. For dedicated or no-interconnect links the transfer
    /// completes immediately. For shared buses, the transfer may be queued if the bus
    /// is currently busy.
    #[allow(clippy::too_many_arguments)]
    pub fn initiate_transfer(
        &mut self,
        job_id: JobId,
        from: DeviceId,
        to: DeviceId,
        data_size: u64,
        priority: u32,
        now: Nanos,
        expected_version: u64,
    ) -> Vec<ScheduledEvent> {
        let ic = match self.find_interconnect(from, to) {
            Some(ic) => ic.clone(),
            None => {
                // No interconnect — same-SoC instant transfer
                return vec![ScheduledEvent {
                    time: now,
                    kind: EventKind::TransferComplete {
                        job_id,
                        expected_version,
                        device_id: to,
                    },
                }];
            }
        };

        let transfer_time = Self::calculate_transfer_time(&ic, data_size);

        match ic.shared_bus {
            None => {
                // Dedicated link — start immediately
                self.active_transfers.push(ActiveTransfer {
                    job_id,
                    bus_id: None,
                });
                vec![ScheduledEvent {
                    time: now + transfer_time,
                    kind: EventKind::TransferComplete {
                        job_id,
                        expected_version,
                        device_id: to,
                    },
                }]
            }
            Some(bus_id) => {
                let bus_busy = self
                    .active_transfers
                    .iter()
                    .any(|at| at.bus_id == Some(bus_id));

                if bus_busy {
                    // Queue the transfer
                    self.pending
                        .entry(bus_id)
                        .or_default()
                        .push_back(PendingTransfer {
                            job_id,
                            target_device: to,
                            data_size,
                            priority,
                            expected_version,
                        });
                    vec![]
                } else {
                    // Bus free — start immediately
                    self.active_transfers.push(ActiveTransfer {
                        job_id,
                        bus_id: Some(bus_id),
                    });
                    vec![ScheduledEvent {
                        time: now + transfer_time,
                        kind: EventKind::TransferComplete {
                            job_id,
                            expected_version,
                            device_id: to,
                        },
                    }]
                }
            }
        }
    }

    /// Handle a completed transfer. Starts the next pending transfer on the bus
    /// if one exists (work-conserving).
    pub fn on_transfer_complete(&mut self, job_id: JobId, now: Nanos) -> Vec<ScheduledEvent> {
        // Find and remove the completed transfer
        let pos = self
            .active_transfers
            .iter()
            .position(|at| at.job_id == job_id);
        let completed = match pos {
            Some(i) => self.active_transfers.swap_remove(i),
            None => return vec![],
        };

        // If the transfer was on a shared bus, start the next pending transfer
        match completed.bus_id {
            Some(bus_id) => self.start_next_pending(bus_id, now),
            None => vec![],
        }
    }

    /// Trigger bus arbitration if the bus is currently idle.
    pub fn on_bus_arbitration(&mut self, bus_id: BusId, now: Nanos) -> Vec<ScheduledEvent> {
        let bus_busy = self
            .active_transfers
            .iter()
            .any(|at| at.bus_id == Some(bus_id));
        if bus_busy {
            vec![]
        } else {
            self.start_next_pending(bus_id, now)
        }
    }

    /// Cancel a job's transfer (active or pending). If an active shared-bus transfer is
    /// cancelled, immediately start the next pending transfer on that bus.
    pub fn cancel_job(&mut self, job_id: JobId, now: Nanos) -> Vec<ScheduledEvent> {
        for queue in self.pending.values_mut() {
            queue.retain(|p| p.job_id != job_id);
        }

        let active_idx = self
            .active_transfers
            .iter()
            .position(|at| at.job_id == job_id);
        let Some(idx) = active_idx else {
            return vec![];
        };
        let cancelled = self.active_transfers.swap_remove(idx);
        match cancelled.bus_id {
            Some(bus_id) => self.start_next_pending(bus_id, now),
            None => vec![],
        }
    }

    /// Start the next pending transfer on a shared bus.
    fn start_next_pending(&mut self, bus_id: BusId, now: Nanos) -> Vec<ScheduledEvent> {
        let arbitration = self
            .buses
            .iter()
            .find(|b| b.id == bus_id)
            .map(|b| b.arbitration.clone());

        let queue = match self.pending.get_mut(&bus_id) {
            Some(q) if !q.is_empty() => q,
            _ => return vec![],
        };

        let idx = match arbitration.as_ref() {
            Some(BusArbitration::PriorityBased) => {
                // Pick lowest priority number (highest priority)
                queue
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, p)| p.priority)
                    .map(|(i, _)| i)
                    .unwrap()
            }
            // RoundRobin, TDMA, Dedicated — all use FIFO
            _ => 0,
        };

        let next = queue.remove(idx).unwrap();

        // Find the interconnect that reaches the target device on this bus
        let ic = self
            .interconnects
            .iter()
            .find(|ic| {
                ic.shared_bus == Some(bus_id)
                    && (ic.to == next.target_device || ic.from == next.target_device)
            })
            .cloned();

        match ic {
            Some(ic) => {
                let transfer_time = Self::calculate_transfer_time(&ic, next.data_size);
                self.active_transfers.push(ActiveTransfer {
                    job_id: next.job_id,
                    bus_id: Some(bus_id),
                });
                vec![ScheduledEvent {
                    time: now + transfer_time,
                    kind: EventKind::TransferComplete {
                        job_id: next.job_id,
                        expected_version: next.expected_version,
                        device_id: next.target_device,
                    },
                }]
            }
            None => vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dedicated_interconnect(from: DeviceId, to: DeviceId) -> InterconnectConfig {
        InterconnectConfig {
            from,
            to,
            latency_ns: 100,
            bandwidth_bytes_per_ns: 2.0,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        }
    }

    fn shared_bus_interconnect(from: DeviceId, to: DeviceId, bus_id: BusId) -> InterconnectConfig {
        InterconnectConfig {
            from,
            to,
            latency_ns: 200,
            bandwidth_bytes_per_ns: 1.0,
            shared_bus: Some(bus_id),
            arbitration: BusArbitration::RoundRobin,
        }
    }

    #[test]
    fn test_dedicated_link_immediate() {
        let cpu = DeviceId(0);
        let gpu = DeviceId(1);
        let ic = dedicated_interconnect(cpu, gpu);
        let mut mgr = TransferManager::new(vec![ic], vec![]);

        let events = mgr.initiate_transfer(JobId(1), cpu, gpu, 1000, 0, 500, 1);

        assert_eq!(events.len(), 1);
        // latency=100 + ceil(1000/2.0)=500 → transfer_time=600
        assert_eq!(events[0].time, 500 + 600);
        match &events[0].kind {
            EventKind::TransferComplete {
                job_id,
                expected_version,
                device_id,
            } => {
                assert_eq!(*job_id, JobId(1));
                assert_eq!(*expected_version, 1);
                assert_eq!(*device_id, gpu);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
    }

    #[test]
    fn test_shared_bus_queuing() {
        let cpu = DeviceId(0);
        let gpu = DeviceId(1);
        let dsp = DeviceId(2);
        let bus = BusId(0);

        let ic1 = shared_bus_interconnect(cpu, gpu, bus);
        let ic2 = shared_bus_interconnect(cpu, dsp, bus);
        let bus_cfg = SharedBusConfig {
            id: bus,
            name: "system_bus".to_string(),
            total_bandwidth_bytes_per_ns: 1.0,
            arbitration: BusArbitration::RoundRobin,
        };
        let mut mgr = TransferManager::new(vec![ic1, ic2], vec![bus_cfg]);

        // First transfer starts immediately
        let ev1 = mgr.initiate_transfer(JobId(1), cpu, gpu, 500, 0, 1000, 1);
        assert_eq!(ev1.len(), 1);
        // latency=200 + ceil(500/1.0)=500 → time = 1000 + 700 = 1700
        assert_eq!(ev1[0].time, 1700);

        // Second transfer is queued (bus busy)
        let ev2 = mgr.initiate_transfer(JobId(2), cpu, dsp, 300, 0, 1050, 1);
        assert!(ev2.is_empty());

        // Complete first transfer → second should start
        let ev3 = mgr.on_transfer_complete(JobId(1), 1700);
        assert_eq!(ev3.len(), 1);
        // latency=200 + ceil(300/1.0)=300 → time = 1700 + 500 = 2200
        assert_eq!(ev3[0].time, 2200);
        match &ev3[0].kind {
            EventKind::TransferComplete {
                job_id, device_id, ..
            } => {
                assert_eq!(*job_id, JobId(2));
                assert_eq!(*device_id, dsp);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
    }

    #[test]
    fn test_no_interconnect_instant() {
        let mut mgr = TransferManager::new(vec![], vec![]);

        let events = mgr.initiate_transfer(JobId(5), DeviceId(0), DeviceId(3), 4096, 0, 2000, 1);

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].time, 2000); // instant
        match &events[0].kind {
            EventKind::TransferComplete {
                job_id, device_id, ..
            } => {
                assert_eq!(*job_id, JobId(5));
                assert_eq!(*device_id, DeviceId(3));
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
    }

    #[test]
    fn test_transfer_time_calculation() {
        let ic = InterconnectConfig {
            from: DeviceId(0),
            to: DeviceId(1),
            latency_ns: 2000,
            bandwidth_bytes_per_ns: 1.5,
            shared_bus: None,
            arbitration: BusArbitration::Dedicated,
        };
        // ceil(4096 / 1.5) = ceil(2730.666...) = 2731
        let time = TransferManager::calculate_transfer_time(&ic, 4096);
        assert_eq!(time, 2000 + 2731);
    }
}
