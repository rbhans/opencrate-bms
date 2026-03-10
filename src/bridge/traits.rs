use crate::config::profile::PointValue;
use crate::store::point_store::PointStore;

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("Connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Point not found: device={device_id}, point={point_id}")]
    PointNotFound {
        device_id: String,
        point_id: String,
    },
    #[error("Write rejected: {0}")]
    WriteRejected(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
}

pub trait PointSource {
    fn start(
        &mut self,
        store: PointStore,
    ) -> impl std::future::Future<Output = Result<(), BridgeError>> + Send;

    fn stop(&mut self) -> impl std::future::Future<Output = Result<(), BridgeError>> + Send;

    fn write_point(
        &self,
        device_id: &str,
        point_id: &str,
        value: PointValue,
        priority: Option<u8>,
    ) -> impl std::future::Future<Output = Result<(), BridgeError>> + Send;
}
