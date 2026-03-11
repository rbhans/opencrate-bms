use std::collections::{HashMap, HashSet};

use crate::bridge::bacnet::BacnetBridge;
use crate::bridge::modbus::ModbusBridge;
use crate::discovery::bacnet_adapter::{adapt_bacnet_device, adapt_bacnet_points};
use crate::discovery::modbus_adapter::{adapt_modbus_device, adapt_modbus_points};
use crate::discovery::model::{ConnStatus, DeviceState};
use crate::event::bus::{Event, EventBus};
use crate::haystack::auto_tag::{suggest_equip_tags, suggest_point_tags_multi};
use crate::haystack::provider::Haystack4Provider;
use crate::node::{Node, NodeCapabilities, NodeType};
use crate::store::discovery_store::DiscoveryStore;
use crate::store::entity_store::EntityStore;
use crate::store::node_store::NodeStore;
use crate::store::point_store::PointStore;

/// Central orchestrator for device discovery and acceptance.
/// Observes bridges — does not replace them.
pub struct DiscoveryService {
    pub store: DiscoveryStore,
    node_store: NodeStore,
    entity_store: EntityStore,
    event_bus: EventBus,
    point_store: PointStore,
}

impl DiscoveryService {
    pub fn new(
        store: DiscoveryStore,
        node_store: NodeStore,
        entity_store: EntityStore,
        event_bus: EventBus,
        point_store: PointStore,
    ) -> Self {
        DiscoveryService {
            store,
            node_store,
            entity_store,
            event_bus,
            point_store,
        }
    }

    /// Run a BACnet scan: re-discover devices on the network via the bridge,
    /// then record them in the DiscoveryStore.
    /// User-initiated only — never called automatically.
    pub async fn scan_bacnet(&self, bridge: &mut BacnetBridge) {
        let scan_id = self.store.record_scan("bacnet").await;

        // Perform a live network re-scan: Who-Is → walk → merge into bridge.
        // rescan() returns Err on transport/discovery failure, Ok on success.
        match bridge.rescan(self.point_store.clone()).await {
            Ok(new_devices) => {
                if !new_devices.is_empty() {
                    println!(
                        "Discovery: rescan found {} new device(s)",
                        new_devices.len()
                    );
                }
            }
            Err(e) => {
                eprintln!("Discovery: BACnet rescan error: {e}");
                self.store.finish_scan(scan_id, 0).await;
                self.event_bus.publish(Event::DiscoveryScanComplete {
                    protocol: "bacnet".into(),
                    device_count: 0,
                });
                return;
            }
        };

        // Which instances actually responded to Who-Is in this scan.
        // Only these get marked Online — cached devices that didn't respond stay unchanged.
        let scanned_instances = bridge.last_scan_instances();

        // Iterate the full (merged) device list from the bridge
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

            // Only set Online for devices that responded to Who-Is in this scan.
            // Don't mark others Offline — Who-Is non-response doesn't mean the device
            // is down (it may just have missed the broadcast). The bridge's poll loop
            // and DeviceDown/DeviceDiscovered events are the authority on BACnet health.
            let instance = dev.device_id.instance();
            if scanned_instances.contains(&instance) {
                let _ = self
                    .store
                    .set_conn_status(&device_id, ConnStatus::Online)
                    .await;
            }

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

    /// Run a Modbus scan: read configured devices from the bridge,
    /// verify connectivity, enrich with FC43 device identification, and record in DiscoveryStore.
    /// User-initiated only — never called automatically.
    pub async fn scan_modbus(&self, bridge: &ModbusBridge) {
        let scan_id = self.store.record_scan("modbus").await;

        let mut devices = bridge.discovered_devices();
        let mut device_count = 0;

        // Probe each device for connectivity, enrich reachable ones with FC43
        let mut online_set: HashSet<String> = HashSet::new();
        for dev in &mut devices {
            if bridge.check_device_online(&dev.instance_id, dev.unit_id).await {
                online_set.insert(dev.instance_id.clone());
                bridge.enrich_device_id(dev).await;
            }
        }

        for dev in &devices {
            let adapted_device = adapt_modbus_device(dev);
            let adapted_points = adapt_modbus_points(dev);
            let device_id = adapted_device.id.clone();

            if let Err(e) = self.store.upsert_device(adapted_device).await {
                eprintln!("Discovery: failed to upsert device {device_id}: {e}");
                continue;
            }

            let status = if online_set.contains(&dev.instance_id) {
                ConnStatus::Online
            } else {
                ConnStatus::Offline
            };
            let _ = self.store.set_conn_status(&device_id, status).await;

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
            protocol: "modbus".into(),
            device_count,
        });
    }

    /// Scan a TCP host for responding Modbus unit IDs and record them in DiscoveryStore.
    /// This is a network-level scan — probes unit IDs in the given range.
    /// Previously-scanned devices not found in this pass are marked Offline.
    /// Returns the number of responding devices found.
    pub async fn scan_modbus_network(
        &self,
        bridge: &ModbusBridge,
        host: &str,
        port: u16,
        start_unit: u8,
        end_unit: u8,
    ) -> usize {
        let scan_id = self.store.record_scan("modbus").await;

        let devices = bridge.scan_unit_ids(host, port, start_unit, end_unit).await;
        let found_ids: HashSet<String> = devices
            .iter()
            .map(|d| adapt_modbus_device(d).id)
            .collect();
        let mut device_count = 0;

        for dev in &devices {
            let adapted_device = adapt_modbus_device(dev);
            let device_id = adapted_device.id.clone();

            if let Err(e) = self.store.upsert_device(adapted_device).await {
                eprintln!("Discovery: failed to upsert scanned device {device_id}: {e}");
                continue;
            }

            let _ = self
                .store
                .set_conn_status(&device_id, ConnStatus::Online)
                .await;

            device_count += 1;
        }

        // Mark previously-scanned devices (scan- prefix for this host) that weren't
        // found in this pass as Offline so stale entries don't linger as Online.
        let scan_prefix = format!("modbus-scan-{host}-{port}-");
        self.mark_missing_offline(&scan_prefix, &found_ids).await;

        self.store.finish_scan(scan_id, device_count).await;

        self.event_bus.publish(Event::DiscoveryScanComplete {
            protocol: "modbus".into(),
            device_count,
        });

        device_count
    }

    /// Scan an RTU serial bus for responding Modbus unit IDs.
    /// Previously-scanned devices not found in this pass are marked Offline.
    /// Returns the number of responding devices found.
    pub async fn scan_modbus_rtu(
        &self,
        bridge: &ModbusBridge,
        start_unit: u8,
        end_unit: u8,
    ) -> usize {
        let scan_id = self.store.record_scan("modbus").await;

        let devices = bridge.scan_rtu_unit_ids(start_unit, end_unit).await;
        let found_ids: HashSet<String> = devices
            .iter()
            .map(|d| adapt_modbus_device(d).id)
            .collect();
        let mut device_count = 0;

        for dev in &devices {
            let adapted_device = adapt_modbus_device(dev);
            let device_id = adapted_device.id.clone();

            if let Err(e) = self.store.upsert_device(adapted_device).await {
                eprintln!("Discovery: failed to upsert RTU device {device_id}: {e}");
                continue;
            }

            let _ = self
                .store
                .set_conn_status(&device_id, ConnStatus::Online)
                .await;

            device_count += 1;
        }

        // Mark previously-scanned RTU devices not found in this pass as Offline
        let scan_prefix = "modbus-scan-rtu-";
        self.mark_missing_offline(scan_prefix, &found_ids).await;

        self.store.finish_scan(scan_id, device_count).await;

        self.event_bus.publish(Event::DiscoveryScanComplete {
            protocol: "modbus".into(),
            device_count,
        });

        device_count
    }

    /// Mark devices with IDs matching `prefix` that are NOT in `found_ids` as Offline.
    async fn mark_missing_offline(&self, prefix: &str, found_ids: &HashSet<String>) {
        let all_devices = self.store.list_devices(None).await;
        for dev in &all_devices {
            if dev.id.starts_with(prefix) && !found_ids.contains(&dev.id) {
                let _ = self
                    .store
                    .set_conn_status(&dev.id, ConnStatus::Offline)
                    .await;
            }
        }
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
