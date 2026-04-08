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
    pub fn from_str(s: &str) -> Result<Self, PlatformError> {
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
        let config = PlatformConfig::from_str(SAMPLE_TOML).expect("failed to parse TOML");
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
    fn preemption_models_parsed() {
        let config = PlatformConfig::from_str(SAMPLE_TOML).unwrap();
        let devices = config.build_devices().unwrap();

        assert!(matches!(devices[0].preemption, PreemptionModel::FullyPreemptive));
        assert!(matches!(
            devices[1].preemption,
            PreemptionModel::LimitedPreemptive { granularity_ns: 50_000 }
        ));
        assert!(matches!(
            devices[2].preemption,
            PreemptionModel::InterruptLevel { .. }
        ));
        assert!(matches!(
            devices[3].preemption,
            PreemptionModel::NonPreemptive { reconfig_time_ns: 2_000_000 }
        ));
    }
}
