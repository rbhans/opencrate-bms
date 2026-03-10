pub mod normalize;

use crate::config::profile::PointValue;
use crate::node::ProtocolBinding;

/// Raw protocol value from a bridge before normalization.
#[derive(Debug, Clone)]
pub enum RawProtocolValue {
    Bacnet {
        device_instance: u32,
        object_type: String,
        object_instance: u32,
        value: serde_json::Value,
    },
    Modbus {
        host: String,
        unit_id: u8,
        register: u16,
        raw_bytes: Vec<u8>,
    },
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
