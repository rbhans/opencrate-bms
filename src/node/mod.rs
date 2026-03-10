use std::collections::HashMap;
use std::time::Instant;

use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum ProtocolBinding {
    Bacnet {
        device_instance: u32,
        object_type: String,
        object_instance: u32,
    },
    Modbus {
        host: String,
        port: u16,
        unit_id: u8,
        register: u16,
        data_type: String,
        scale: f64,
    },
    Virtual,
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
}
