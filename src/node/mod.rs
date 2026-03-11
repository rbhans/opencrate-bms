use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Deserializer, Serialize};

use crate::config::profile::PointValue;
use crate::store::point_store::PointStatusFlags;

pub type NodeId = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeType {
    Site,
    Space,
    Equip,
    Point,
    VirtualPoint,
}

impl NodeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Site => "site",
            Self::Space => "space",
            Self::Equip => "equip",
            Self::Point => "point",
            Self::VirtualPoint => "virtual_point",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "site" => Some(Self::Site),
            "space" => Some(Self::Space),
            "equip" => Some(Self::Equip),
            "point" => Some(Self::Point),
            "virtual_point" => Some(Self::VirtualPoint),
            _ => None,
        }
    }
}

/// What this node can do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct NodeCapabilities {
    pub readable: bool,
    pub writable: bool,
    pub historizable: bool,
    pub alarmable: bool,
    pub schedulable: bool,
}

/// How this node connects to the physical world.
/// Protocol-agnostic: any protocol stores its config as JSON under a protocol tag.
///
/// Serialized as `{"protocol":"bacnet","config":{...}}`.
/// Backward-compatible: also deserializes the legacy tagged-enum format
/// `{"protocol":"bacnet","device_instance":1000,...}` by wrapping non-protocol
/// fields into `config`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProtocolBinding {
    /// Protocol identifier (e.g. "bacnet", "modbus", "virtual")
    pub protocol: String,
    /// Protocol-specific configuration (interpretation depends on protocol)
    #[serde(default)]
    pub config: serde_json::Value,
}

impl<'de> Deserialize<'de> for ProtocolBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Deserialize as a generic JSON map first
        let mut map: serde_json::Map<String, serde_json::Value> =
            serde_json::Map::deserialize(deserializer)?;

        let protocol = map
            .remove("protocol")
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_else(|| "virtual".into());

        // New format: has a "config" key
        if let Some(config) = map.remove("config") {
            return Ok(ProtocolBinding { protocol, config });
        }

        // Legacy format: remaining keys ARE the config (old tagged enum)
        if map.is_empty() {
            Ok(ProtocolBinding {
                protocol,
                config: serde_json::Value::Null,
            })
        } else {
            Ok(ProtocolBinding {
                protocol,
                config: serde_json::Value::Object(map),
            })
        }
    }
}

impl ProtocolBinding {
    /// Create a virtual (no-protocol) binding.
    pub fn virtual_binding() -> Self {
        ProtocolBinding {
            protocol: "virtual".into(),
            config: serde_json::Value::Null,
        }
    }

    /// Create a BACnet protocol binding.
    pub fn bacnet(device_instance: u32, object_type: &str, object_instance: u32) -> Self {
        ProtocolBinding {
            protocol: "bacnet".into(),
            config: serde_json::json!({
                "device_instance": device_instance,
                "object_type": object_type,
                "object_instance": object_instance,
            }),
        }
    }

    /// Create a Modbus protocol binding.
    pub fn modbus(host: &str, port: u16, unit_id: u8, register: u16, data_type: &str, scale: f64) -> Self {
        ProtocolBinding {
            protocol: "modbus".into(),
            config: serde_json::json!({
                "host": host,
                "port": port,
                "unit_id": unit_id,
                "register": register,
                "data_type": data_type,
                "scale": scale,
            }),
        }
    }

    pub fn is_virtual(&self) -> bool {
        self.protocol == "virtual"
    }

    pub fn is_bacnet(&self) -> bool {
        self.protocol == "bacnet"
    }

    pub fn is_modbus(&self) -> bool {
        self.protocol == "modbus"
    }
}

/// The unified object model. Every point/device/equipment/site/space is a Node.
#[derive(Debug, Clone)]
pub struct Node {
    pub id: NodeId,
    pub node_type: NodeType,
    pub dis: String,
    pub parent_id: Option<NodeId>,

    // Live state (point nodes only)
    pub value: Option<PointValue>,
    pub timestamp: Option<Instant>,
    pub status: PointStatusFlags,

    // Metadata
    pub tags: HashMap<String, Option<String>>,
    pub refs: HashMap<String, NodeId>,
    pub properties: HashMap<String, String>,

    // Capabilities and binding
    pub capabilities: NodeCapabilities,
    pub binding: Option<ProtocolBinding>,
}

impl Node {
    pub fn new(id: impl Into<NodeId>, node_type: NodeType, dis: impl Into<String>) -> Self {
        Node {
            id: id.into(),
            node_type,
            dis: dis.into(),
            parent_id: None,
            value: None,
            timestamp: None,
            status: PointStatusFlags::default(),
            tags: HashMap::new(),
            refs: HashMap::new(),
            properties: HashMap::new(),
            capabilities: NodeCapabilities::default(),
            binding: None,
        }
    }

    pub fn with_parent(mut self, parent_id: impl Into<NodeId>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    pub fn with_capabilities(mut self, caps: NodeCapabilities) -> Self {
        self.capabilities = caps;
        self
    }

    pub fn with_binding(mut self, binding: ProtocolBinding) -> Self {
        self.binding = Some(binding);
        self
    }

    pub fn is_point(&self) -> bool {
        matches!(self.node_type, NodeType::Point | NodeType::VirtualPoint)
    }
}

/// Lightweight snapshot of live state for the hot cache.
#[derive(Debug, Clone)]
pub struct NodeSnapshot {
    pub value: Option<PointValue>,
    pub timestamp: Option<Instant>,
    pub status: PointStatusFlags,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_builder() {
        let node = Node::new("ahu-1/dat", NodeType::Point, "Discharge Air Temp")
            .with_parent("ahu-1")
            .with_capabilities(NodeCapabilities {
                readable: true,
                writable: false,
                historizable: true,
                alarmable: true,
                schedulable: false,
            });

        assert_eq!(node.id, "ahu-1/dat");
        assert_eq!(node.parent_id.as_deref(), Some("ahu-1"));
        assert!(node.is_point());
        assert!(node.capabilities.readable);
        assert!(!node.capabilities.writable);
    }

    #[test]
    fn equip_node() {
        let node = Node::new("ahu-1", NodeType::Equip, "AHU-1");
        assert!(!node.is_point());
        assert_eq!(node.node_type, NodeType::Equip);
    }

    #[test]
    fn node_type_roundtrip() {
        for nt in &[NodeType::Site, NodeType::Space, NodeType::Equip, NodeType::Point, NodeType::VirtualPoint] {
            let s = nt.as_str();
            let parsed = NodeType::from_str(s).unwrap();
            assert_eq!(&parsed, nt);
        }
    }

    #[test]
    fn protocol_binding_new_format() {
        let b = ProtocolBinding::bacnet(1000, "analog-input", 1);
        let json = serde_json::to_string(&b).unwrap();
        let deser: ProtocolBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.protocol, "bacnet");
        assert_eq!(deser.config["device_instance"], 1000);
        assert_eq!(deser.config["object_type"], "analog-input");
    }

    #[test]
    fn protocol_binding_legacy_bacnet() {
        // Old tagged-enum format: {"protocol":"bacnet","device_instance":1000,...}
        let legacy = r#"{"protocol":"bacnet","device_instance":1000,"object_type":"analog-input","object_instance":1}"#;
        let b: ProtocolBinding = serde_json::from_str(legacy).unwrap();
        assert_eq!(b.protocol, "bacnet");
        assert_eq!(b.config["device_instance"], 1000);
        assert_eq!(b.config["object_type"], "analog-input");
        assert_eq!(b.config["object_instance"], 1);
    }

    #[test]
    fn protocol_binding_legacy_modbus() {
        let legacy = r#"{"protocol":"modbus","host":"192.168.1.1","port":502,"unit_id":1,"register":100,"data_type":"uint16","scale":1.0}"#;
        let b: ProtocolBinding = serde_json::from_str(legacy).unwrap();
        assert_eq!(b.protocol, "modbus");
        assert_eq!(b.config["host"], "192.168.1.1");
        assert_eq!(b.config["port"], 502);
    }

    #[test]
    fn protocol_binding_legacy_virtual() {
        let legacy = r#"{"protocol":"virtual"}"#;
        let b: ProtocolBinding = serde_json::from_str(legacy).unwrap();
        assert!(b.is_virtual());
        assert_eq!(b.config, serde_json::Value::Null);
    }

    #[test]
    fn protocol_binding_custom_protocol() {
        let b = ProtocolBinding {
            protocol: "knx".into(),
            config: serde_json::json!({"group_address": "1/2/3"}),
        };
        let json = serde_json::to_string(&b).unwrap();
        let deser: ProtocolBinding = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.protocol, "knx");
        assert_eq!(deser.config["group_address"], "1/2/3");
    }
}
