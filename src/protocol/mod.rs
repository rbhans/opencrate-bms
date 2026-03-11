pub mod normalize;

use crate::config::profile::PointValue;

/// Raw protocol value from a bridge before normalization.
/// Protocol-agnostic: any protocol pushes raw data as JSON with a protocol tag.
#[derive(Debug, Clone)]
pub struct RawProtocolValue {
    /// Protocol identifier (e.g. "bacnet", "modbus", "knx")
    pub protocol: String,
    /// Device key within the protocol (e.g. device instance, host:unit combo)
    pub device_key: String,
    /// Point key within the device (e.g. object type+instance, register address)
    pub point_key: String,
    /// Raw data from the protocol — interpretation is protocol-specific
    pub raw_data: serde_json::Value,
}

/// Trait for protocol drivers — replaces the old PointSource trait.
/// Bridges push raw values to a ValueSink instead of writing to PointStore.
pub trait ProtocolDriver: Send {
    fn start(
        &mut self,
        sink: Box<dyn ValueSink>,
    ) -> impl std::future::Future<Output = Result<(), DriverError>> + Send;

    fn stop(&mut self) -> impl std::future::Future<Output = Result<(), DriverError>> + Send;

    fn write(
        &self,
        binding: &ProtocolBinding,
        value: PointValue,
    ) -> impl std::future::Future<Output = Result<(), DriverError>> + Send;

    fn protocol_name(&self) -> &str;
}

use crate::node::ProtocolBinding;

/// Receives raw values from protocol drivers.
pub trait ValueSink: Send + Sync {
    fn on_value(&self, raw: RawProtocolValue);
    fn on_device_status(&self, device_key: &str, online: bool);
}

#[derive(Debug, thiserror::Error)]
pub enum DriverError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("write rejected: {0}")]
    WriteRejected(String),
    #[error("protocol error: {0}")]
    Protocol(String),
}
