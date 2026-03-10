use crate::config::profile::PointValue;
use crate::node::NodeId;
use crate::store::alarm_store::{AlarmConfig, AlarmState};

// ----------------------------------------------------------------
// Plugin traits — trait boundaries for future dynamic loading
// ----------------------------------------------------------------

/// Plugin that provides a protocol driver (BACnet, Modbus, etc.)
pub trait ProtocolDriverPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn create_driver(&self, config: &serde_json::Value) -> Box<dyn ProtocolDriverBoxed>;
}

/// Object-safe version of ProtocolDriver for use in plugin registry.
/// The real ProtocolDriver uses RPITIT and can't be boxed directly.
pub trait ProtocolDriverBoxed: Send {
    fn protocol_name(&self) -> &str;
}

/// Plugin that provides a history storage backend.
pub trait HistoryBackend: Send + Sync {
    fn write_batch(
        &self,
        samples: Vec<HistorySample>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), HistoryBackendError>> + Send + '_>>;

    fn query(
        &self,
        query: HistoryQuery,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<HistoryResult, HistoryBackendError>> + Send + '_>>;
}

/// Plugin that provides custom alarm evaluation logic.
pub trait AlarmEvaluator: Send + Sync {
    fn evaluate(
        &self,
        config: &AlarmConfig,
        value: &PointValue,
        prev: AlarmState,
    ) -> AlarmState;
}

/// Plugin that provides a logic/program engine.
pub trait LogicEnginePlugin: Send + Sync {
    fn name(&self) -> &str;
    fn evaluate(&self, ctx: &LogicContext) -> Vec<(NodeId, PointValue)>;
}

/// Plugin for importing/exporting data.
pub trait ImportExportPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn supported_formats(&self) -> Vec<String>;
    fn import(&self, data: &[u8], format: &str) -> Result<Vec<ImportedNode>, ImportExportError>;
    fn export(&self, nodes: &[ExportNode], format: &str) -> Result<Vec<u8>, ImportExportError>;
}

// ----------------------------------------------------------------
// Supporting types for plugin traits
// ----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct HistorySample {
    pub node_id: NodeId,
    pub timestamp_ms: i64,
    pub value: f64,
}

#[derive(Debug, Clone)]
pub struct HistoryQuery {
    pub node_id: NodeId,
    pub start_ms: i64,
    pub end_ms: i64,
    pub max_results: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct HistoryResult {
    pub node_id: NodeId,
    pub samples: Vec<HistorySample>,
}

#[derive(Debug, thiserror::Error)]
pub enum HistoryBackendError {
    #[error("backend error: {0}")]
    Backend(String),
}

pub struct LogicContext {
    pub tick_ms: i64,
    pub inputs: Vec<(NodeId, PointValue)>,
}

#[derive(Debug, Clone)]
pub struct ImportedNode {
    pub id: NodeId,
    pub node_type: String,
    pub dis: String,
    pub parent_id: Option<NodeId>,
    pub tags: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone)]
pub struct ExportNode {
    pub id: NodeId,
    pub node_type: String,
    pub dis: String,
    pub parent_id: Option<NodeId>,
    pub tags: Vec<(String, Option<String>)>,
    pub refs: Vec<(String, NodeId)>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportExportError {
    #[error("format error: {0}")]
    Format(String),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
}

// ----------------------------------------------------------------
// Plugin Registry
// ----------------------------------------------------------------

/// Central registry for all plugins. Plugins are registered at startup.
/// No dynamic loading — all plugins are compiled in.
pub struct PluginRegistry {
    pub protocol_drivers: Vec<Box<dyn ProtocolDriverPlugin>>,
    pub history_backends: Vec<Box<dyn HistoryBackend>>,
    pub alarm_evaluators: Vec<Box<dyn AlarmEvaluator>>,
    pub logic_engines: Vec<Box<dyn LogicEnginePlugin>>,
    pub import_export: Vec<Box<dyn ImportExportPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        PluginRegistry {
            protocol_drivers: Vec::new(),
            history_backends: Vec::new(),
            alarm_evaluators: Vec::new(),
            logic_engines: Vec::new(),
            import_export: Vec::new(),
        }
    }

    pub fn register_protocol_driver(&mut self, plugin: Box<dyn ProtocolDriverPlugin>) {
        self.protocol_drivers.push(plugin);
    }

    pub fn register_history_backend(&mut self, backend: Box<dyn HistoryBackend>) {
        self.history_backends.push(backend);
    }

    pub fn register_alarm_evaluator(&mut self, evaluator: Box<dyn AlarmEvaluator>) {
        self.alarm_evaluators.push(evaluator);
    }

    pub fn register_logic_engine(&mut self, engine: Box<dyn LogicEnginePlugin>) {
        self.logic_engines.push(engine);
    }

    pub fn register_import_export(&mut self, plugin: Box<dyn ImportExportPlugin>) {
        self.import_export.push(plugin);
    }

    pub fn find_protocol_driver(&self, name: &str) -> Option<&dyn ProtocolDriverPlugin> {
        self.protocol_drivers
            .iter()
            .find(|p| p.name() == name)
            .map(|p| p.as_ref())
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ----------------------------------------------------------------
// Standard alarm evaluator (extracts existing logic into a plugin)
// ----------------------------------------------------------------

/// Default alarm evaluator — implements the standard BAS alarm logic.
pub struct StandardAlarmEvaluator;

impl AlarmEvaluator for StandardAlarmEvaluator {
    fn evaluate(
        &self,
        _config: &AlarmConfig,
        _value: &PointValue,
        prev: AlarmState,
    ) -> AlarmState {
        // Default: delegate to the existing alarm engine logic.
        // This is a placeholder — the actual evaluation still lives in alarm_store.rs
        // for now. The trait boundary is what matters.
        prev
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_creation() {
        let reg = PluginRegistry::new();
        assert!(reg.protocol_drivers.is_empty());
        assert!(reg.history_backends.is_empty());
    }

    #[test]
    fn standard_evaluator() {
        let eval = StandardAlarmEvaluator;
        let state = eval.evaluate(
            &AlarmConfig {
                id: 1,
                device_id: "d".into(),
                point_id: "p".into(),
                alarm_type: crate::store::alarm_store::AlarmType::HighLimit,
                severity: crate::store::alarm_store::AlarmSeverity::Warning,
                enabled: true,
                params: crate::store::alarm_store::AlarmParams::HighLimit {
                    limit: 100.0,
                    deadband: 1.0,
                    delay_secs: 0,
                },
            },
            &PointValue::Float(105.0),
            AlarmState::Normal,
        );
        // Standard evaluator is a placeholder — just returns prev state
        assert_eq!(state, AlarmState::Normal);
    }
}
