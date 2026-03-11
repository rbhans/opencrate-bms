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
    /// BACnet network config (kept for backward compat with existing scenarios)
    pub bacnet: Option<BacnetNetworkConfig>,
    /// Modbus network config (kept for backward compat with existing scenarios)
    pub modbus: Option<ModbusNetworkConfig>,
    /// Extensible protocol configs for plugin-provided protocols.
    /// Key = protocol identifier (e.g. "knx"), value = protocol-specific JSON config.
    #[serde(default)]
    pub protocols: std::collections::HashMap<String, serde_json::Value>,
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
    /// If set, start a BACnet server exposing local points as this device instance.
    pub server_device_instance: Option<u32>,
    /// Serial port path for MS/TP mode (e.g., "/dev/ttyUSB0" or "COM3").
    pub serial_port: Option<String>,
    /// Baud rate for MS/TP mode (default: 38400).
    pub baud_rate: Option<u32>,
    /// This node's MAC address for MS/TP mode (0-127, default: 0).
    pub mac_address: Option<u8>,
    /// Highest MAC address to poll for new masters in MS/TP mode (default: 127).
    pub max_master: Option<u8>,
}

/// Modbus network transport configuration in the scenario file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModbusNetworkConfig {
    /// "tcp" (default) or "rtu"
    pub mode: Option<String>,
    /// RTU: serial port path (e.g. "/dev/ttyUSB0" or "COM3")
    pub serial_port: Option<String>,
    /// RTU: baud rate (default: 9600)
    pub baud_rate: Option<u32>,
    /// Response timeout in milliseconds (default: 5000)
    pub default_timeout_ms: Option<u64>,
    /// Number of retries on read failure (default: 3)
    pub default_retry_count: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInstance {
    pub profile: String,
    pub instance_id: String,
    pub overrides: Option<serde_json::Value>,
}
