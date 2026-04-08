//! Platform configuration loading from TOML files.

use std::path::Path;

use hprss_types::{
    BusArbitration, BusId, DeviceId, InterconnectConfig, Nanos, SharedBusConfig,
    device::{DeviceConfig, MultiCorePolicy, PreemptionModel},
    task::DeviceType,
};
use serde::Deserialize;

/// Top-level platform configuration (deserialized from TOML)
#[derive(Debug, Deserialize)]
pub struct PlatformConfig {
    pub simulation: SimulationSection,
    pub device: Vec<DeviceSection>,
    #[serde(default)]
    pub interconnect: Vec<InterconnectSection>,
    #[serde(default)]
    pub shared_bus: Vec<SharedBusSection>,
}

#[derive(Debug, Deserialize)]
pub struct SimulationSection {
    pub duration_ms: u64,
    pub seed: u64,
    #[serde(default = "default_time_unit")]
    pub time_unit: String,
}

fn default_time_unit() -> String {
    "ns".into()
}

#[derive(Debug, Deserialize)]
pub struct DeviceSection {
    pub name: String,
    #[serde(default)]
    pub device_group: Option<String>,
    #[serde(rename = "type")]
    pub device_type: String,
    pub cores: u32,
    pub preemption: String,
    #[serde(default)]
    pub preemption_granularity_us: Option<u64>,
    #[serde(default)]
    pub isr_overhead_us: Option<u64>,
    #[serde(default)]
    pub dma_non_preemptive_us: Option<u64>,
    #[serde(default)]
    pub reconfig_time_us: Option<u64>,
    #[serde(default)]
    pub preempt_overhead_us: u64,
    #[serde(default = "default_context_switch")]
    pub context_switch_us: u64,
    #[serde(default = "default_speed")]
    pub speed_factor: f64,
    #[serde(default)]
    pub multicore_policy: Option<String>,
    #[serde(default)]
    pub power_watts: Option<f64>,
}

fn default_context_switch() -> u64 {
    5
}
fn default_speed() -> f64 {
    1.0
}

#[derive(Debug, Deserialize)]
pub struct InterconnectSection {
    pub from: String,
    pub to: String,
    pub latency_us: f64,
    pub bandwidth_gbps: f64,
    #[serde(default)]
    pub shared_bus: Option<String>,
    #[serde(default = "default_arbitration")]
    pub arbitration: String,
}

fn default_arbitration() -> String {
    "dedicated".into()
}

#[derive(Debug, Deserialize)]
pub struct SharedBusSection {
    pub name: String,
    pub total_bandwidth_gbps: f64,
    pub arbitration: String,
}

impl PlatformConfig {
    /// Load platform configuration from a TOML file
    pub fn load(path: &Path) -> Result<Self, PlatformError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| PlatformError::IoError(path.display().to_string(), e))?;
        toml::from_str(&content)
            .map_err(|e| PlatformError::ParseError(path.display().to_string(), e))
    }

    /// Load from a TOML string (for testing)
    pub fn from_toml(s: &str) -> Result<Self, PlatformError> {
        toml::from_str(s).map_err(|e| PlatformError::ParseError("<string>".into(), e))
    }

    /// Convert to typed DeviceConfig list
    pub fn build_devices(&self) -> Result<Vec<DeviceConfig>, PlatformError> {
        self.device
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let device_type = parse_device_type(&d.device_type)?;
                let preemption = parse_preemption(d)?;
                let multicore_policy = d
                    .multicore_policy
                    .as_deref()
                    .map(parse_multicore_policy)
                    .transpose()?;

                Ok(DeviceConfig {
                    id: DeviceId(i as u32),
                    name: d.name.clone(),
                    device_group: d.device_group.clone(),
                    device_type,
                    cores: d.cores,
                    preemption,
                    context_switch_ns: d.context_switch_us * 1_000,
                    speed_factor: d.speed_factor,
                    multicore_policy,
                    power_watts: d.power_watts,
                })
            })
            .collect()
    }

    /// Convert to typed InterconnectConfig list
    pub fn build_interconnects(
        &self,
        devices: &[DeviceConfig],
    ) -> Result<Vec<InterconnectConfig>, PlatformError> {
        self.interconnect
            .iter()
            .map(|ic| {
                let from = devices
                    .iter()
                    .find(|d| d.name == ic.from)
                    .ok_or_else(|| {
                        PlatformError::InvalidValue(format!("unknown device: {}", ic.from))
                    })?
                    .id;
                let to = devices
                    .iter()
                    .find(|d| d.name == ic.to)
                    .ok_or_else(|| {
                        PlatformError::InvalidValue(format!("unknown device: {}", ic.to))
                    })?
                    .id;

                let shared_bus = ic
                    .shared_bus
                    .as_deref()
                    .map(|name| {
                        self.shared_bus
                            .iter()
                            .position(|b| b.name == name)
                            .map(|idx| BusId(idx as u32))
                            .ok_or_else(|| {
                                PlatformError::InvalidValue(format!("unknown bus: {name}"))
                            })
                    })
                    .transpose()?;

                Ok(InterconnectConfig {
                    from,
                    to,
                    latency_ns: (ic.latency_us * 1_000.0) as u64,
                    bandwidth_bytes_per_ns: ic.bandwidth_gbps / 8.0,
                    shared_bus,
                    arbitration: parse_bus_arbitration(&ic.arbitration)?,
                })
            })
            .collect()
    }

    /// Convert to typed SharedBusConfig list
    pub fn build_buses(&self) -> Result<Vec<SharedBusConfig>, PlatformError> {
        self.shared_bus
            .iter()
            .enumerate()
            .map(|(i, b)| {
                Ok(SharedBusConfig {
                    id: BusId(i as u32),
                    name: b.name.clone(),
                    total_bandwidth_bytes_per_ns: b.total_bandwidth_gbps / 8.0,
                    arbitration: parse_bus_arbitration(&b.arbitration)?,
                })
            })
            .collect()
    }

    /// Get simulation duration in nanoseconds
    pub fn duration_ns(&self) -> Nanos {
        self.simulation.duration_ms * 1_000_000
    }
}

fn parse_device_type(s: &str) -> Result<DeviceType, PlatformError> {
    match s.to_lowercase().as_str() {
        "cpu" => Ok(DeviceType::Cpu),
        "gpu" => Ok(DeviceType::Gpu),
        "dsp" => Ok(DeviceType::Dsp),
        "fpga" => Ok(DeviceType::Fpga),
        _ => Err(PlatformError::InvalidValue(format!(
            "unknown device type: {s}"
        ))),
    }
}

fn parse_preemption(d: &DeviceSection) -> Result<PreemptionModel, PlatformError> {
    match d.preemption.to_lowercase().as_str() {
        "fully_preemptive" | "full" => Ok(PreemptionModel::FullyPreemptive),
        "limited" | "limited_preemptive" => {
            let granularity_us = d.preemption_granularity_us.unwrap_or(50);
            Ok(PreemptionModel::LimitedPreemptive {
                granularity_ns: granularity_us * 1_000,
            })
        }
        "interrupt_level" | "interrupt" => {
            let isr = d.isr_overhead_us.unwrap_or(5);
            let dma = d.dma_non_preemptive_us.unwrap_or(100);
            Ok(PreemptionModel::InterruptLevel {
                isr_overhead_ns: isr * 1_000,
                dma_non_preemptive_ns: dma * 1_000,
            })
        }
        "non_preemptive" | "none" => {
            let reconfig = d.reconfig_time_us.unwrap_or(2000);
            Ok(PreemptionModel::NonPreemptive {
                reconfig_time_ns: reconfig * 1_000,
            })
        }
        _ => Err(PlatformError::InvalidValue(format!(
            "unknown preemption model: {}",
            d.preemption
        ))),
    }
}

fn parse_multicore_policy(s: &str) -> Result<MultiCorePolicy, PlatformError> {
    match s.to_lowercase().as_str() {
        "partitioned" => Ok(MultiCorePolicy::Partitioned),
        "global" => Ok(MultiCorePolicy::Global),
        s if s.starts_with("clustered") => {
            // Parse "clustered_2" or "clustered(2)"
            let k: u32 = s
                .trim_start_matches("clustered")
                .trim_start_matches(['_', '('])
                .trim_end_matches(')')
                .parse()
                .unwrap_or(2);
            Ok(MultiCorePolicy::Clustered { k })
        }
        _ => Err(PlatformError::InvalidValue(format!(
            "unknown multicore policy: {s}"
        ))),
    }
}

fn parse_bus_arbitration(s: &str) -> Result<BusArbitration, PlatformError> {
    match s.to_lowercase().as_str() {
        "dedicated" => Ok(BusArbitration::Dedicated),
        "round_robin" => Ok(BusArbitration::RoundRobin),
        "priority_based" => Ok(BusArbitration::PriorityBased),
        s if s.starts_with("tdma") => {
            let slot: u64 = s
                .trim_start_matches("tdma")
                .trim_start_matches(['_', '('])
                .trim_end_matches(')')
                .parse()
                .map_err(|_| PlatformError::InvalidValue(format!("invalid TDMA slot: {s}")))?;
            Ok(BusArbitration::Tdma {
                slot_ns: slot * 1_000,
            })
        }
        _ => Err(PlatformError::InvalidValue(format!(
            "unknown arbitration: {s}"
        ))),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("IO error reading {0}: {1}")]
    IoError(String, std::io::Error),
    #[error("TOML parse error in {0}: {1}")]
    ParseError(String, toml::de::Error),
    #[error("Invalid value: {0}")]
    InvalidValue(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[simulation]
duration_ms = 10000
seed = 42

[[device]]
name = "FT2000"
type = "cpu"
cores = 4
preemption = "fully_preemptive"
context_switch_us = 5
speed_factor = 1.0
multicore_policy = "partitioned"

[[device]]
name = "GP201"
type = "gpu"
cores = 128
preemption = "limited"
preemption_granularity_us = 50
context_switch_us = 200
speed_factor = 20.0

[[device]]
name = "FT6678"
type = "dsp"
cores = 8
preemption = "interrupt_level"
isr_overhead_us = 5
dma_non_preemptive_us = 100
context_switch_us = 2
speed_factor = 8.0

[[device]]
name = "Zynq_FPGA"
type = "fpga"
cores = 1
preemption = "non_preemptive"
reconfig_time_us = 2000
context_switch_us = 2000
speed_factor = 15.0

[[interconnect]]
from = "FT2000"
to = "GP201"
latency_us = 2.0
bandwidth_gbps = 12.0
shared_bus = "pcie_bus"
arbitration = "priority_based"

[[shared_bus]]
name = "pcie_bus"
total_bandwidth_gbps = 16.0
arbitration = "priority_based"
"#;

    #[test]
    fn parse_sample_platform() {
        let config = PlatformConfig::from_toml(SAMPLE_TOML).expect("failed to parse TOML");
        assert_eq!(config.device.len(), 4);
        assert_eq!(config.simulation.duration_ms, 10000);
        assert_eq!(config.simulation.seed, 42);

        let devices = config.build_devices().expect("failed to build devices");
        assert_eq!(devices.len(), 4);
        assert_eq!(devices[0].name, "FT2000");
        assert_eq!(devices[0].device_type, DeviceType::Cpu);
        assert_eq!(devices[1].device_type, DeviceType::Gpu);
        assert_eq!(devices[2].device_type, DeviceType::Dsp);
        assert_eq!(devices[3].device_type, DeviceType::Fpga);
    }

    #[test]
    fn build_interconnects_from_sample() {
        let config = PlatformConfig::from_toml(SAMPLE_TOML).unwrap();
        let devices = config.build_devices().unwrap();
        let ics = config.build_interconnects(&devices).unwrap();
        assert_eq!(ics.len(), 1);
        assert_eq!(ics[0].from, DeviceId(0)); // FT2000
        assert_eq!(ics[0].to, DeviceId(1)); // GP201
        assert_eq!(ics[0].latency_ns, 2_000); // 2.0 us
        assert!((ics[0].bandwidth_bytes_per_ns - 1.5).abs() < 1e-9); // 12 / 8
        assert_eq!(ics[0].shared_bus, Some(BusId(0)));
        assert!(matches!(ics[0].arbitration, BusArbitration::PriorityBased));
    }

    #[test]
    fn build_buses_from_sample() {
        let config = PlatformConfig::from_toml(SAMPLE_TOML).unwrap();
        let buses = config.build_buses().unwrap();
        assert_eq!(buses.len(), 1);
        assert_eq!(buses[0].id, BusId(0));
        assert_eq!(buses[0].name, "pcie_bus");
        assert!((buses[0].total_bandwidth_bytes_per_ns - 2.0).abs() < 1e-9); // 16 / 8
        assert!(matches!(
            buses[0].arbitration,
            BusArbitration::PriorityBased
        ));
    }

    #[test]
    fn parse_bus_arbitration_variants() {
        use super::parse_bus_arbitration;

        assert!(matches!(
            parse_bus_arbitration("dedicated").unwrap(),
            BusArbitration::Dedicated
        ));
        assert!(matches!(
            parse_bus_arbitration("round_robin").unwrap(),
            BusArbitration::RoundRobin
        ));
        assert!(matches!(
            parse_bus_arbitration("priority_based").unwrap(),
            BusArbitration::PriorityBased
        ));
        assert!(matches!(
            parse_bus_arbitration("tdma_100").unwrap(),
            BusArbitration::Tdma { slot_ns: 100_000 }
        ));
        assert!(matches!(
            parse_bus_arbitration("TDMA(50)").unwrap(),
            BusArbitration::Tdma { slot_ns: 50_000 }
        ));
        assert!(parse_bus_arbitration("bogus").is_err());
    }

    #[test]
    fn preemption_models_parsed() {
        let config = PlatformConfig::from_toml(SAMPLE_TOML).unwrap();
        let devices = config.build_devices().unwrap();

        assert!(matches!(
            devices[0].preemption,
            PreemptionModel::FullyPreemptive
        ));
        assert!(matches!(
            devices[1].preemption,
            PreemptionModel::LimitedPreemptive {
                granularity_ns: 50_000
            }
        ));
        assert!(matches!(
            devices[2].preemption,
            PreemptionModel::InterruptLevel { .. }
        ));
        assert!(matches!(
            devices[3].preemption,
            PreemptionModel::NonPreemptive {
                reconfig_time_ns: 2_000_000
            }
        ));
    }

    #[test]
    fn explicit_per_core_cpu_devices_keep_group() {
        let toml = r#"
[simulation]
duration_ms = 10
seed = 1

[[device]]
name = "FT2000-core0"
device_group = "FT2000"
type = "cpu"
cores = 1
preemption = "fully_preemptive"

[[device]]
name = "FT2000-core1"
device_group = "FT2000"
type = "cpu"
cores = 1
preemption = "fully_preemptive"
"#;
        let config = PlatformConfig::from_toml(toml).unwrap();
        let devices = config.build_devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_group.as_deref(), Some("FT2000"));
        assert_eq!(devices[1].device_group.as_deref(), Some("FT2000"));
    }
}
