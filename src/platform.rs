use std::path::Path;

use crate::bridge::bacnet::{bacnet_config_from_scenario, BacnetBridge};
use crate::bridge::modbus::ModbusBridge;
use crate::bridge::traits::PointSource;
use crate::config::loader::{resolve_scenario, LoadedScenario};
use crate::config::template::auto_create_nodes;
use crate::event::bus::EventBus;
use crate::plugin::PluginRegistry;
use crate::discovery::service::DiscoveryService;
use crate::store::alarm_store::{start_alarm_engine, AlarmStore};
use crate::store::discovery_store::{start_conn_status_listener, start_discovery_store, DiscoveryStore};
use crate::store::entity_store::{start_entity_store, EntityStore};
use crate::store::history_store::{start_history_collector, HistoryStore};
use crate::store::node_store::{start_node_store, NodeStore};
use crate::store::point_store::PointStore;
use crate::store::schedule_store::{start_schedule_engine, ScheduleStore};

/// Core model state — the platform data layer.
pub struct ModelState {
    pub point_store: PointStore,
    pub node_store: NodeStore,
    pub event_bus: EventBus,
    pub plugin_registry: PluginRegistry,
    pub loaded: LoadedScenario,
}

/// Automation engines — alarm, schedule, history.
pub struct AutomationState {
    pub alarm_store: AlarmStore,
    pub schedule_store: ScheduleStore,
    pub history_store: HistoryStore,
    pub entity_store: EntityStore,
    pub discovery_store: DiscoveryStore,
}

/// The full platform — everything except GUI signals.
pub struct Platform {
    pub model: ModelState,
    pub automation: AutomationState,
    pub discovery_service: DiscoveryService,
}

/// Bridge handles for write routing.
pub struct BridgeHandles {
    pub bacnet: Option<BacnetBridge>,
    pub modbus: Option<ModbusBridge>,
}

/// Initialize the platform from a scenario file.
/// Used by both CLI and GUI.
pub async fn init_platform(
    scenario_path: &Path,
    profiles_dir: &Path,
) -> Result<(Platform, BridgeHandles), Box<dyn std::error::Error>> {
    let loaded = resolve_scenario(scenario_path, profiles_dir)?;

    let event_bus = EventBus::new();
    let point_store = PointStore::new().with_event_bus(event_bus.clone());
    let node_store = start_node_store().with_event_bus(event_bus.clone());

    // Initialize point store from loaded profiles
    for dev in &loaded.devices {
        point_store.initialize_from_profile(&dev.instance_id, &dev.profile);
    }

    // Auto-create nodes from scenario (equip + point nodes with auto-tagging)
    auto_create_nodes(&node_store, &loaded).await;

    // Start automation engines
    let history_store = start_history_collector(&point_store, &loaded.devices);
    let alarm_store = start_alarm_engine(&point_store, &loaded).with_event_bus(event_bus.clone());
    let schedule_store = start_schedule_engine(&point_store).with_event_bus(event_bus.clone());
    let entity_store = start_entity_store().with_event_bus(event_bus.clone());
    let discovery_store = start_discovery_store().with_event_bus(event_bus.clone());
    start_conn_status_listener(discovery_store.clone(), event_bus.clone());

    // Start protocol bridges
    let bacnet_config = bacnet_config_from_scenario(&loaded.config.settings);
    let mut bacnet = BacnetBridge::new()
        .with_bacnet_config(bacnet_config)
        .with_event_bus(event_bus.clone())
        .with_history_store(history_store.clone());
    if let Err(e) = bacnet.start(point_store.clone()).await {
        eprintln!("BACnet bridge error: {e}");
    }

    let mut modbus = ModbusBridge::new().from_loaded_devices(&loaded.devices);
    if let Err(e) = modbus.start(point_store.clone()).await {
        eprintln!("Modbus bridge error: {e}");
    }

    let plugin_registry = PluginRegistry::new();

    let discovery_service = DiscoveryService::new(
        discovery_store.clone(),
        node_store.clone(),
        entity_store.clone(),
        event_bus.clone(),
    );

    let platform = Platform {
        model: ModelState {
            point_store,
            node_store,
            event_bus,
            plugin_registry,
            loaded,
        },
        automation: AutomationState {
            alarm_store,
            schedule_store,
            history_store,
            entity_store,
            discovery_store,
        },
        discovery_service,
    };

    let bridges = BridgeHandles {
        bacnet: Some(bacnet),
        modbus: Some(modbus),
    };

    Ok((platform, bridges))
}
