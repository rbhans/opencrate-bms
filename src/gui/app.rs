use std::path::PathBuf;
use std::sync::Arc;

use dioxus::prelude::*;
use tokio::sync::Mutex;

use crate::bridge::bacnet::{bacnet_config_from_scenario, BacnetBridge};
use crate::bridge::modbus::ModbusBridge;
use crate::bridge::traits::PointSource;
use crate::config::loader::resolve_scenario;
use crate::event::bus::{Event, EventBus};
use crate::discovery::service::DiscoveryService;
use crate::store::alarm_store::start_alarm_engine;
use crate::store::discovery_store::{start_conn_status_listener, start_discovery_store};
use crate::store::entity_store::start_entity_store;
use crate::store::history_store::start_history_collector;
use crate::store::node_store::start_node_store;
use crate::store::point_store::{PointKey, PointStatusFlags, PointStore};
use crate::store::schedule_store::start_schedule_engine;

use super::components::alarm_view::AlarmView;
use super::components::config_view::ConfigView;
use super::components::point_detail::PointDetail;
use super::components::point_table::PointTable;
use super::components::schedule_view::ScheduleView;
use super::components::sidebar::Sidebar;
use super::components::toolbar::Toolbar;
use super::components::floor_plan::FloorPlanCanvas;
use super::components::trend_chart::TrendView;
use super::state::{ActiveView, AppState, DashboardTool, SidebarTab, WriteCommand};

#[component]
pub fn App() -> Element {
    let scenario_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("scenarios/small-office.json"));
    let profiles_dir = PathBuf::from("profiles");

    let loaded = use_hook(|| {
        resolve_scenario(&scenario_path, &profiles_dir).unwrap_or_else(|e| {
            eprintln!("Failed to load scenario: {e}");
            std::process::exit(1);
        })
    });

    let event_bus = use_hook(EventBus::new);

    let store = use_hook(|| {
        let s = PointStore::new().with_event_bus(event_bus.clone());
        for dev in &loaded.devices {
            s.initialize_from_profile(&dev.instance_id, &dev.profile);
        }
        s
    });

    let node_store = use_hook(|| start_node_store().with_event_bus(event_bus.clone()));
    let history_store = use_hook(|| start_history_collector(&store, &loaded.devices));
    let alarm_store = use_hook(|| start_alarm_engine(&store, &loaded).with_event_bus(event_bus.clone()));
    let schedule_store = use_hook(|| start_schedule_engine(&store).with_event_bus(event_bus.clone()));
    let entity_store = use_hook(|| start_entity_store().with_event_bus(event_bus.clone()));
    let discovery_store = use_hook(|| {
        let ds = start_discovery_store().with_event_bus(event_bus.clone());
        start_conn_status_listener(ds.clone(), event_bus.clone());
        ds
    });
    let discovery_service = use_hook(|| {
        Arc::new(DiscoveryService::new(
            discovery_store.clone(),
            node_store.clone(),
            entity_store.clone(),
            event_bus.clone(),
        ))
    });

    let mut store_version = use_signal(|| 0u64);
    let selected_device = use_signal(|| Option::<String>::None);
    let selected_point = use_signal(|| Option::<String>::None);
    let mut write_error = use_signal(|| Option::<String>::None);
    let active_view = use_signal(|| ActiveView::Home);
    let sidebar_tab = use_signal(|| SidebarTab::Devices);
    let detail_open = use_signal(|| false);
    let nav_tree = use_signal(Vec::new);
    let next_node_id = use_signal(|| 1u32);
    let pages = use_signal(std::collections::HashMap::new);
    let dashboards = use_signal(Vec::new);
    let active_dashboard_id = use_signal(|| Option::<String>::None);
    let selected_widget = use_signal(|| Option::<String>::None);
    let dashboard_tool = use_signal(|| DashboardTool::Select);
    let next_widget_id = use_signal(|| 1u32);
    let drag_op = use_signal(|| Option::<crate::gui::state::DragOp>::None);
    let quick_trend_device = use_signal(|| Option::<String>::None);
    let quick_trend_point = use_signal(|| Option::<String>::None);
    let quick_trend_range = use_signal(|| crate::gui::state::TrendRange::Hour1);

    let (write_tx, write_rx) = use_hook(|| {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<WriteCommand>();
        (tx, Arc::new(Mutex::new(Some(rx))))
    });

    // Shared bridge handles — created before AppState so discovery view can access them
    let bacnet_bridge: Arc<Mutex<Option<BacnetBridge>>> = use_hook(|| Arc::new(Mutex::new(None)));
    let modbus_bridge: Arc<Mutex<Option<ModbusBridge>>> = use_hook(|| Arc::new(Mutex::new(None)));

    let app_state = use_hook(|| AppState {
        store: store.clone(),
        node_store: node_store.clone(),
        event_bus: event_bus.clone(),
        loaded: loaded.clone(),
        active_view,
        sidebar_tab,
        selected_device,
        selected_point,
        detail_open,
        store_version,
        nav_tree,
        write_tx: write_tx.clone(),
        write_error,
        next_node_id,
        pages,
        history_store: history_store.clone(),
        dashboards,
        active_dashboard_id,
        selected_widget,
        dashboard_tool,
        next_widget_id,
        drag_op,
        quick_trend_device,
        quick_trend_point,
        quick_trend_range,
        alarm_store: alarm_store.clone(),
        schedule_store: schedule_store.clone(),
        entity_store: entity_store.clone(),
        discovery_store: discovery_store.clone(),
        discovery_service: discovery_service.clone(),
        bacnet_bridge: bacnet_bridge.clone(),
    });
    use_context_provider(|| app_state.clone());

    // Store watcher + bridge startup
    let watcher_store = store.clone();
    let bridge_store = store.clone();
    let bridge_loaded = loaded.clone();
    let bacnet_for_start = bacnet_bridge.clone();
    let modbus_for_start = modbus_bridge.clone();
    let bacnet_config = bacnet_config_from_scenario(&loaded.config.settings);
    let bridge_event_bus = event_bus.clone();
    use_hook(move || {
        spawn(async move {
            let mut rx = watcher_store.subscribe();
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                store_version.set(*rx.borrow());
            }
        });
        spawn(async move {
            let mut bacnet = BacnetBridge::new()
                .with_bacnet_config(bacnet_config)
                .with_event_bus(bridge_event_bus);
            if let Err(e) = bacnet.start(bridge_store.clone()).await {
                eprintln!("BACnet bridge error: {e}");
            }
            *bacnet_for_start.lock().await = Some(bacnet);

            let mut modbus =
                ModbusBridge::new().from_loaded_devices(&bridge_loaded.devices);
            if let Err(e) = modbus.start(bridge_store.clone()).await {
                eprintln!("Modbus bridge error: {e}");
            }
            *modbus_for_start.lock().await = Some(modbus);

            // Keep bridges alive
            std::future::pending::<()>().await;
        });
    });

    // Status sync — EventBus-driven alarm flag projection + periodic stale check
    let sync_store = store.clone();
    let sync_alarm = alarm_store.clone();
    let sync_bus = event_bus.clone();
    use_hook(move || {
        // Alarm flag sync via EventBus (immediate, replaces 3-second poll for alarms)
        let alarm_store_clone = sync_store.clone();
        let alarm_alarm_clone = sync_alarm.clone();
        let mut alarm_rx = sync_bus.subscribe();
        spawn(async move {
            // Do an initial full sync on startup
            {
                let keys = alarm_store_clone.all_keys();
                let active = alarm_alarm_clone.get_active_alarms().await;
                let alarmed_points: std::collections::HashSet<(String, String)> = active.iter()
                    .map(|a| (a.device_id.clone(), a.point_id.clone()))
                    .collect();
                for key in &keys {
                    let is_alarmed = alarmed_points.contains(&(key.device_instance_id.clone(), key.point_id.clone()));
                    if is_alarmed {
                        alarm_store_clone.set_status(key, PointStatusFlags::ALARM);
                    } else {
                        alarm_store_clone.clear_status(key, PointStatusFlags::ALARM);
                    }
                }
            }

            // Then react to alarm events
            loop {
                match alarm_rx.recv().await {
                    Ok(event) => match event.as_ref() {
                        Event::AlarmRaised { node_id, .. } => {
                            if let Some((dev, pt)) = node_id.split_once('/') {
                                let key = PointKey {
                                    device_instance_id: dev.to_string(),
                                    point_id: pt.to_string(),
                                };
                                alarm_store_clone.set_status(&key, PointStatusFlags::ALARM);
                            }
                        }
                        Event::AlarmCleared { node_id, .. } => {
                            if let Some((dev, pt)) = node_id.split_once('/') {
                                let key = PointKey {
                                    device_instance_id: dev.to_string(),
                                    point_id: pt.to_string(),
                                };
                                alarm_store_clone.clear_status(&key, PointStatusFlags::ALARM);
                            }
                        }
                        _ => {}
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => break,
                }
            }
        });

        // Stale check remains periodic (every 30 seconds — staleness is time-based)
        let stale_store = sync_store.clone();
        spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                let keys = stale_store.all_keys();
                for key in &keys {
                    if let Some(tv) = stale_store.get(key) {
                        let age = tv.timestamp.elapsed();
                        if age > std::time::Duration::from_secs(300) {
                            stale_store.set_status(key, PointStatusFlags::STALE);
                        } else {
                            stale_store.clear_status(key, PointStatusFlags::STALE);
                        }
                    }
                }
            }
        });
    });

    // Write command handler — routes writes to the appropriate bridge
    let write_store = store.clone();
    let write_rx_slot = write_rx.clone();
    let bacnet_for_write = bacnet_bridge.clone();
    let modbus_for_write = modbus_bridge.clone();
    use_hook(move || {
        spawn(async move {
            let mut rx = write_rx_slot.lock().await.take().unwrap();
            while let Some(cmd) = rx.recv().await {
                let mut written = false;

                // Try Modbus bridge first (matches by instance_id)
                if let Some(ref bridge) = *modbus_for_write.lock().await {
                    match bridge.write_point(&cmd.device_id, &cmd.point_id, cmd.value.clone(), cmd.priority).await {
                        Ok(()) => {
                            written = true;
                            write_error.set(None);
                        }
                        Err(crate::bridge::traits::BridgeError::PointNotFound { .. }) => {
                            // Not a modbus device — try BACnet next
                        }
                        Err(e) => {
                            eprintln!("Modbus write error: {e}");
                            write_error.set(Some(format!("Write failed: {e}")));
                            continue;
                        }
                    }
                }

                // Try BACnet bridge
                if !written {
                    if let Some(ref bridge) = *bacnet_for_write.lock().await {
                        match bridge.write_point(&cmd.device_id, &cmd.point_id, cmd.value.clone(), cmd.priority).await {
                            Ok(()) => {
                                write_error.set(None);
                            }
                            Err(e) => {
                                eprintln!("BACnet write error: {e}");
                                write_error.set(Some(format!("Write failed: {e}")));
                                continue;
                            }
                        }
                    }
                }

                // Also update local store so UI reflects immediately
                let write_key = PointKey {
                    device_instance_id: cmd.device_id.clone(),
                    point_id: cmd.point_id.clone(),
                };
                write_store.set(write_key.clone(), cmd.value);
                write_store.set_status(&write_key, PointStatusFlags::OVERRIDDEN);
            }
        });
    });

    let current_view = active_view.read().clone();
    let show_detail = *detail_open.read();
    let is_history = matches!(current_view, ActiveView::History);
    let is_alarms = matches!(current_view, ActiveView::Alarms);
    let is_schedules = matches!(current_view, ActiveView::Schedules);
    let is_config = matches!(current_view, ActiveView::Config);

    rsx! {
        document::Link { rel: "stylesheet", href: asset!("/assets/style.css") }

        div { class: "app-shell",
            Toolbar {}

            div { class: "app-body",
                if is_history {
                    // History view has its own 3-pane layout
                    TrendView {}
                } else if is_alarms {
                    // Alarm view has its own 3-pane layout
                    AlarmView {}
                } else if is_schedules {
                    // Schedule view has its own 3-pane layout
                    ScheduleView {}
                } else if is_config {
                    // Config view has its own 3-pane layout
                    ConfigView {}
                } else {
                    Sidebar {}

                    div { class: "main-content",
                        match &current_view {
                            ActiveView::Home => rsx! { HomeView {} },
                            ActiveView::Alarms => rsx! { },
                            ActiveView::Schedules => rsx! { },
                            ActiveView::History => rsx! { },
                            ActiveView::Page(id) => rsx! {
                                FloorPlanCanvas { page_id: id.clone() }
                            },
                            ActiveView::Device { .. } => rsx! { PointTable {} },
                            ActiveView::Config => rsx! { },
                        }
                    }

                    if show_detail {
                        DetailsPane {}
                    }
                }
            }
        }
    }
}

#[component]
fn HomeView() -> Element {
    let state = use_context::<AppState>();
    let selected = state.selected_device.read().clone();

    if selected.is_some() {
        rsx! { PointTable {} }
    } else {
        rsx! {
            div { class: "view-placeholder",
                h2 { "Welcome" }
                p { "Select a device from the sidebar to view its points." }
            }
        }
    }
}

#[component]
fn DetailsPane() -> Element {
    let mut state = use_context::<AppState>();
    let selected_device = state.selected_device.read().clone();
    let selected_point = state.selected_point.read().clone();

    rsx! {
        div { class: "details-pane",
            div { class: "details-header",
                span { "Details" }
                button {
                    class: "close-btn",
                    onclick: move |_| state.detail_open.set(false),
                    "x"
                }
            }
            if selected_point.is_some() {
                PointDetail {}
            } else if let Some(dev_id) = selected_device {
                DeviceSummary { device_id: dev_id }
            } else {
                div { class: "point-detail-body",
                    p { class: "placeholder", "Select a zone or point to view details." }
                }
            }
        }
    }
}

/// Compact device summary shown in the detail pane when a zone is clicked.
#[component]
fn DeviceSummary(device_id: String) -> Element {
    let state = use_context::<AppState>();
    let _version = state.store_version.read();

    let device = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == device_id);

    let Some(dev) = device else {
        return rsx! {
            div { class: "point-detail-body",
                p { class: "placeholder", "Device '{device_id}' not found." }
            }
        };
    };

    let profile_name = dev.profile.profile.name.clone();

    rsx! {
        div { class: "point-detail-body",
            h4 { class: "detail-point-name", "{device_id}" }
            p { class: "detail-subtitle", "{profile_name}" }

            table { class: "detail-point-table",
                thead {
                    tr {
                        th { "Point" }
                        th { "Value" }
                    }
                }
                tbody {
                    for pt in dev.profile.points.iter() {
                        {
                            let key = PointKey {
                                device_instance_id: device_id.clone(),
                                point_id: pt.id.clone(),
                            };
                            let val = state.store.get(&key);
                            let val_str = match &val {
                                Some(tv) => format!("{:?}", tv.value),
                                None => "—".into(),
                            };
                            let units = pt.units.clone().unwrap_or_default();
                            rsx! {
                                tr {
                                    key: "{pt.id}",
                                    td { "{pt.name}" }
                                    td { "{val_str} {units}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
