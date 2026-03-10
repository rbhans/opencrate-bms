use std::collections::HashMap;

use crate::bridge::bacnet::BacnetBridge;
use crate::discovery::bacnet_adapter::{adapt_bacnet_device, adapt_bacnet_points};
use crate::discovery::model::{ConnStatus, DeviceState};
use crate::event::bus::{Event, EventBus};
use crate::haystack::auto_tag::{suggest_equip_tags, suggest_point_tags_multi};
use crate::haystack::provider::Haystack4Provider;
use crate::node::{Node, NodeCapabilities, NodeType};
use crate::store::discovery_store::DiscoveryStore;
use crate::store::entity_store::EntityStore;
use crate::store::node_store::NodeStore;

/// Central orchestrator for device discovery and acceptance.
/// Observes bridges — does not replace them.
pub struct DiscoveryService {
    pub store: DiscoveryStore,
    node_store: NodeStore,
    entity_store: EntityStore,
    event_bus: EventBus,
}

impl DiscoveryService {
    pub fn new(
        store: DiscoveryStore,
        node_store: NodeStore,
        entity_store: EntityStore,
        event_bus: EventBus,
    ) -> Self {
        DiscoveryService {
            store,
            node_store,
            entity_store,
            event_bus,
        }
    }

    /// Run a BACnet scan using an already-started bridge's discovered devices.
    /// User-initiated only — never called automatically.
    pub async fn scan_bacnet(&self, bridge: &BacnetBridge) {
        let scan_id = self.store.record_scan("bacnet").await;
        let devices = bridge.discovered_devices();
        let mut device_count = 0;

        for dev in devices {
            let adapted_device = adapt_bacnet_device(dev);
            let adapted_points = adapt_bacnet_points(dev);
            let device_id = adapted_device.id.clone();

            if let Err(e) = self.store.upsert_device(adapted_device).await {
                eprintln!("Discovery: failed to upsert device {device_id}: {e}");
                continue;
            }

            // Update connectivity
            let _ = self
                .store
                .set_conn_status(&device_id, ConnStatus::Online)
                .await;

            if let Err(e) = self
                .store
                .upsert_points(&device_id, adapted_points)
                .await
            {
                eprintln!("Discovery: failed to upsert points for {device_id}: {e}");
            }

            device_count += 1;
        }

        self.store.finish_scan(scan_id, device_count).await;

        self.event_bus.publish(Event::DiscoveryScanComplete {
            protocol: "bacnet".into(),
            device_count,
        });
    }

    /// Accept a discovered device — creates nodes + entities + tags in one pass.
    pub async fn accept_device(&self, device_id: &str) -> Result<(), String> {
        let device = self
            .store
            .get_device(device_id)
            .await
            .map_err(|e| format!("Device not found: {e}"))?;

        if device.state == DeviceState::Accepted {
            return Ok(()); // Already accepted
        }

        let points = self.store.get_points(device_id).await;
        let provider = Haystack4Provider;

        // 1. Create equip node (ignore error if already exists)
        let equip_node = Node::new(device_id, NodeType::Equip, &device.display_name);
        let _ = self.node_store.create_node(equip_node).await;

        // 2. Auto-tag equipment
        let equip_tags = suggest_equip_tags(&device.display_name, &provider);

        // 3. Create equip entity with tags
        let _ = self
            .entity_store
            .create_entity(
                device_id,
                "equip",
                &device.display_name,
                None,
                equip_tags.clone(),
            )
            .await;

        // Build equip_tags as HashMap for point tagging
        let equip_tag_map: HashMap<String, Option<String>> =
            equip_tags.into_iter().collect();

        // 4. Create point nodes + entities
        for pt in &points {
            let point_node_id = format!("{}/{}", device_id, pt.id);

            // Build node with capabilities + binding
            let caps = NodeCapabilities {
                readable: true,
                writable: pt.writable,
                historizable: true,
                alarmable: true,
                schedulable: pt.writable,
            };

            let node = Node::new(&point_node_id, NodeType::Point, &pt.display_name)
                .with_parent(device_id)
                .with_capabilities(caps)
                .with_binding(pt.binding.clone());

            let _ = self.node_store.create_node(node).await;

            // Auto-tag point using multiple name sources
            let names: Vec<&str> = vec![&pt.id, &pt.display_name];
            let point_tags = suggest_point_tags_multi(
                &names,
                pt.units.as_deref(),
                &equip_tag_map,
                &provider,
            );

            // Create point entity
            let _ = self
                .entity_store
                .create_entity(
                    &point_node_id,
                    "point",
                    &pt.display_name,
                    Some(device_id),
                    point_tags,
                )
                .await;

            // Set equipRef on point entity
            let _ = self
                .entity_store
                .set_ref(&point_node_id, "equipRef", device_id)
                .await;
        }

        // 5. Update device state
        self.store
            .set_device_state(device_id, DeviceState::Accepted)
            .await
            .map_err(|e| format!("Failed to update state: {e}"))?;

        // 6. Publish event
        self.event_bus.publish(Event::DeviceAccepted {
            device_key: device_id.to_string(),
            protocol: device.protocol.as_str().to_string(),
            point_count: points.len(),
        });

        Ok(())
    }

    /// Ignore a discovered device.
    pub async fn ignore_device(&self, device_id: &str) -> Result<(), String> {
        self.store
            .set_device_state(device_id, DeviceState::Ignored)
            .await
            .map_err(|e| format!("Failed to ignore device: {e}"))
    }

    /// Un-ignore a device (move back to Discovered).
    pub async fn unignore_device(&self, device_id: &str) -> Result<(), String> {
        self.store
            .set_device_state(device_id, DeviceState::Discovered)
            .await
            .map_err(|e| format!("Failed to unignore device: {e}"))
    }
}
