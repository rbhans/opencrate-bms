use std::path::Path;

use crate::bridge::bacnet::{bacnet_config_from_scenario, BacnetBridge};
use crate::bridge::modbus::{modbus_config_from_scenario, ModbusBridge};
use crate::bridge::traits::PointSource;
use crate::config::loader::{resolve_scenario, LoadedScenario};
use crate::config::template::auto_create_nodes;
use crate::event::bus::EventBus;
use crate::plugin::PluginRegistry;
use crate::project::ProjectPaths;
use crate::discovery::service::DiscoveryService;
use crate::store::alarm_store::{start_alarm_engine_with_path, AlarmStore};
use crate::store::discovery_store::{start_conn_status_listener, start_discovery_store_with_path, DiscoveryStore};
use crate::store::entity_store::{start_entity_store_with_path, EntityStore};
use crate::store::history_store::{start_history_collector_with_path, HistoryStore};
use crate::store::node_store::{start_node_store_with_path, NodeStore};
use crate::store::point_store::PointStore;
use crate::store::schedule_store::{start_schedule_engine_with_path, ScheduleStore};
use crate::logic::engine::ExecutionEngine;
use crate::logic::store::{start_program_store_with_path, ProgramStore};

/// Core model state — the platform data layer.
pub struct ModelState {
    pub point_store: PointStore,
    pub node_store: NodeStore,
    pub event_bus: EventBus,
    pub plugin_registry: PluginRegistry,
    pub loaded: LoadedScenario,
}

/// Automation engines — alarm, schedule, history, logic.
pub struct AutomationState {
    pub alarm_store: AlarmStore,
    pub schedule_store: ScheduleStore,
    pub history_store: HistoryStore,
    pub entity_store: EntityStore,
    pub discovery_store: DiscoveryStore,
    pub program_store: ProgramStore,
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

/// Initialize the platform from project paths.
/// Used by both CLI and GUI.
pub async fn init_platform(
    paths: &ProjectPaths,
) -> Result<(Platform, BridgeHandles), Box<dyn std::error::Error>> {
    // Ensure data directory exists
    if !paths.data_dir.exists() {
        std::fs::create_dir_all(&paths.data_dir)?;
    }

    let loaded = resolve_scenario(&paths.scenario, &paths.profiles_dir)?;

    let event_bus = EventBus::new();
    let point_store = PointStore::new().with_event_bus(event_bus.clone());
    let node_store = start_node_store_with_path(&paths.db_path("nodes.db")).with_event_bus(event_bus.clone());

    // Initialize point store from loaded profiles
    for dev in &loaded.devices {
        point_store.initialize_from_profile(&dev.instance_id, &dev.profile);
    }

    // Auto-create nodes from scenario (equip + point nodes with auto-tagging)
    auto_create_nodes(&node_store, &loaded).await;

    // Start automation engines
    let history_store = start_history_collector_with_path(&point_store, &loaded.devices, &paths.db_path("history.db"));
    let alarm_store = start_alarm_engine_with_path(&point_store, &paths.db_path("alarms.db")).with_event_bus(event_bus.clone());
    let schedule_store = start_schedule_engine_with_path(&point_store, &paths.db_path("schedules.db")).with_event_bus(event_bus.clone());
    let entity_store = start_entity_store_with_path(&paths.db_path("entities.db")).with_event_bus(event_bus.clone());
    let discovery_store = start_discovery_store_with_path(&paths.db_path("discovery.db")).with_event_bus(event_bus.clone());
    start_conn_status_listener(discovery_store.clone(), event_bus.clone());

    // Start logic engine
    let program_store = start_program_store_with_path(&paths.db_path("programs.db"));
    let logic_engine = ExecutionEngine {
        program_store: program_store.clone(),
        point_store: point_store.clone(),
        event_bus: event_bus.clone(),
        write_callback: None,
    };
    logic_engine.start();

    // Start protocol bridges
    let bacnet_config = bacnet_config_from_scenario(&loaded.config.settings);
    let mut bacnet = BacnetBridge::new()
        .with_bacnet_config(bacnet_config)
        .with_event_bus(event_bus.clone())
        .with_history_store(history_store.clone());

    // Init server object store BEFORE start() so the server handler is available when transport is created
    if let Some(server_instance) = loaded
        .config
        .settings
        .as_ref()
        .and_then(|s| s.bacnet.as_ref())
        .and_then(|b| b.server_device_instance)
    {
        bacnet.init_server_store(server_instance, &point_store);
    }

    if let Err(e) = bacnet.start(point_store.clone()).await {
        eprintln!("BACnet bridge error: {e}");
    }

    let modbus_config = modbus_config_from_scenario(&loaded.config.settings);
    let mut modbus = ModbusBridge::new()
        .with_modbus_config(modbus_config)
        .with_event_bus(event_bus.clone())
        .from_loaded_devices(&loaded.devices);
    if let Err(e) = modbus.start(point_store.clone()).await {
        eprintln!("Modbus bridge error: {e}");
    }

    let plugin_registry = PluginRegistry::new();

    let discovery_service = DiscoveryService::new(
        discovery_store.clone(),
        node_store.clone(),
        entity_store.clone(),
        event_bus.clone(),
        point_store.clone(),
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
            program_store,
        },
        discovery_service,
    };

    let bridges = BridgeHandles {
        bacnet: Some(bacnet),
        modbus: Some(modbus),
    };

    Ok((platform, bridges))
}

/// Legacy convenience: initialize from raw scenario + profiles paths.
/// Constructs a temporary ProjectPaths treating CWD as project root.
pub async fn init_platform_legacy(
    scenario_path: &Path,
    profiles_dir: &Path,
) -> Result<(Platform, BridgeHandles), Box<dyn std::error::Error>> {
    // Build a ProjectPaths that points to CWD-relative locations
    let cwd = std::env::current_dir()?;
    let paths = ProjectPaths {
        root: cwd.clone(),
        scenario: if scenario_path.is_absolute() {
            scenario_path.to_path_buf()
        } else {
            cwd.join(scenario_path)
        },
        profiles_dir: if profiles_dir.is_absolute() {
            profiles_dir.to_path_buf()
        } else {
            cwd.join(profiles_dir)
        },
        data_dir: cwd.join("data"),
    };
    init_platform(&paths).await
}
