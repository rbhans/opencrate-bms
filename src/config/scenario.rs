use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioConfig {
    pub scenario: ScenarioMeta,
    pub settings: Option<ScenarioSettings>,
    pub devices: Vec<DeviceInstance>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioMeta {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioSettings {
    pub tick_rate_ms: Option<u64>,
    pub realtime: Option<bool>,
    pub bacnet: Option<BacnetNetworkConfig>,
}

/// BACnet network transport configuration in the scenario file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BacnetNetworkConfig {
    /// "normal", "foreign", or "sc"
    pub mode: Option<String>,
    /// BBMD address for foreign device mode (e.g., "192.168.1.1:47808")
    pub bbmd_addr: Option<String>,
    /// TTL in seconds for foreign device registration (default: 60)
    pub ttl: Option<u16>,
    /// WebSocket endpoint for BACnet/SC mode (e.g., "wss://hub.example.com:1234/bacnet")
    pub hub_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInstance {
    pub profile: String,
    pub instance_id: String,
    pub overrides: Option<serde_json::Value>,
}
