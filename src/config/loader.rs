use std::path::Path;

use crate::config::profile::DeviceProfile;
use crate::config::scenario::ScenarioConfig;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("Profile not found: {0}")]
    ProfileNotFound(String),
    #[error("Validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Clone)]
pub struct LoadedDevice {
    pub instance_id: String,
    pub profile: DeviceProfile,
}

#[derive(Debug, Clone)]
pub struct LoadedScenario {
    pub config: ScenarioConfig,
    pub devices: Vec<LoadedDevice>,
    pub warnings: Vec<String>,
}

pub fn resolve_scenario(
    scenario_path: &Path,
    profiles_dir: &Path,
) -> Result<LoadedScenario, ConfigError> {
    let scenario_json = std::fs::read_to_string(scenario_path)?;
    let config: ScenarioConfig = serde_json::from_str(&scenario_json)?;

    let mut devices = Vec::new();
    let mut warnings = Vec::new();

    for device_instance in &config.devices {
        let profile_path = profiles_dir.join(format!("{}.json", device_instance.profile));

        if !profile_path.exists() {
            warnings.push(format!(
                "Profile '{}' not found for device '{}' (expected at {})",
                device_instance.profile,
                device_instance.instance_id,
                profile_path.display()
            ));
            continue;
        }

        let profile_json = std::fs::read_to_string(&profile_path)?;
        let mut profile: DeviceProfile = serde_json::from_str(&profile_json)?;

        if let Some(overrides) = &device_instance.overrides {
            apply_overrides(&mut profile, overrides);
        }

        devices.push(LoadedDevice {
            instance_id: device_instance.instance_id.clone(),
            profile,
        });
    }

    Ok(LoadedScenario {
        config,
        devices,
        warnings,
    })
}

fn apply_overrides(profile: &mut DeviceProfile, overrides: &serde_json::Value) {
    let defaults = profile.defaults.get_or_insert(
        crate::config::profile::DeviceDefaults { protocols: None },
    );

    let protocols = defaults.protocols.get_or_insert(
        crate::config::profile::ProtocolDefaults {
            bacnet: None,
            modbus: None,
        },
    );

    let bacnet = protocols.bacnet.get_or_insert(
        crate::config::profile::BacnetDefaults {
            device_id: None,
            device_name: None,
            vendor_id: None,
        },
    );

    if let Some(id) = overrides.get("bacnet_device_id").and_then(|v| v.as_u64()) {
        bacnet.device_id = Some(id as u32);
    }
    if let Some(name) = overrides
        .get("bacnet_device_name")
        .and_then(|v| v.as_str())
    {
        bacnet.device_name = Some(name.to_string());
    }

    // Modbus overrides
    let modbus = protocols.modbus.get_or_insert(
        crate::config::profile::ModbusDefaults {
            unit_id: None,
            host: None,
            port: None,
            byte_order: None,
            word_order: None,
            response_timeout_ms: None,
            retry_count: None,
            throttle_delay_ms: None,
        },
    );

    if let Some(host) = overrides.get("modbus_host").and_then(|v| v.as_str()) {
        modbus.host = Some(host.to_string());
    }
    if let Some(port) = overrides.get("modbus_port").and_then(|v| v.as_u64()) {
        modbus.port = Some(port as u16);
    }
    if let Some(unit_id) = overrides.get("modbus_unit_id").and_then(|v| v.as_u64()) {
        modbus.unit_id = Some(unit_id as u8);
    }
}
