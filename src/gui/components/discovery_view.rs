use dioxus::prelude::*;

use crate::auth::Permission;
use crate::discovery::model::{
    ConnStatus, DeviceState, DiscoveredDevice, DiscoveredPoint, PROTOCOL_BACNET, PROTOCOL_MODBUS,
};
use crate::gui::state::AppState;

use super::bacnet_device_alarms::BacnetDeviceAlarms;
use super::bacnet_device_cov::BacnetDeviceAdvanced;
use super::bacnet_device_files::BacnetDeviceFiles;
use super::bacnet_device_trends::BacnetDeviceTrends;
use super::bacnet_network_tools::BacnetNetworkTools;
use super::modbus_device_diagnostics::ModbusDeviceDiagnostics;
use super::modbus_device_registers::ModbusDeviceRegisters;

/// Helper to increment a signal by 1 without borrow conflicts.
fn bump(sig: &mut Signal<u64>) {
    let v = *sig.read();
    sig.set(v + 1);
}

// ── Top-level discovery sub-tabs ──
#[derive(Clone, Copy, PartialEq)]
enum DiscoveryTab {
    AllDevices,
    Bacnet,
    Modbus,
}

// ── Detail sub-tabs for device detail pane (protocol-aware) ──
#[derive(Clone, Copy, PartialEq)]
enum DeviceDetailTab {
    Overview,
    BacnetManagement,
    BacnetAlarms,
    BacnetTrends,
    BacnetFiles,
    BacnetAdvanced,
    BacnetCommission,
    ModbusRegisters,
    ModbusDiagnostics,
}

impl DeviceDetailTab {
    fn label(self) -> &'static str {
        match self {
            Self::Overview => "Overview",
            Self::BacnetManagement => "Manage",
            Self::BacnetAlarms => "Alarms",
            Self::BacnetTrends => "Trends",
            Self::BacnetFiles => "Files",
            Self::BacnetAdvanced => "Advanced",
            Self::BacnetCommission => "Commission",
            Self::ModbusRegisters => "Registers",
            Self::ModbusDiagnostics => "Diagnostics",
        }
    }
}

/// Return the detail tabs available for a given device.
fn tabs_for_device(protocol: &str, state: DeviceState) -> Vec<DeviceDetailTab> {
    if state != DeviceState::Accepted {
        return vec![DeviceDetailTab::Overview];
    }
    match protocol {
        "bacnet" => vec![
            DeviceDetailTab::Overview,
            DeviceDetailTab::BacnetManagement,
            DeviceDetailTab::BacnetAlarms,
            DeviceDetailTab::BacnetTrends,
            DeviceDetailTab::BacnetFiles,
            DeviceDetailTab::BacnetAdvanced,
            DeviceDetailTab::BacnetCommission,
        ],
        "modbus" => vec![
            DeviceDetailTab::Overview,
            DeviceDetailTab::ModbusRegisters,
            DeviceDetailTab::ModbusDiagnostics,
        ],
        // Unknown protocols get just the overview tab
        _ => vec![DeviceDetailTab::Overview],
    }
}

#[component]
pub fn DiscoveryView() -> Element {
    let state = use_context::<AppState>();
    let user_is_admin = state.has_permission(Permission::ManageDiscovery);
    let mut devices = use_signal(Vec::<DiscoveredDevice>::new);
    let selected_device_id = use_signal(|| Option::<String>::None);
    let mut selected_points = use_signal(Vec::<DiscoveredPoint>::new);
    let mut scanning_bacnet = use_signal(|| false);
    let mut scanning_modbus = use_signal(|| false);
    let mut refresh_counter = use_signal(|| 0u64);
    let event_infos: Signal<Vec<crate::bridge::bacnet::BacnetEventInfo>> =
        use_signal(Vec::new);
    let trend_logs: Signal<Vec<(u32, String)>> = use_signal(Vec::new);
    let create_object_type: Signal<String> = use_signal(|| "AnalogValue".to_string());
    let delete_object_input: Signal<String> = use_signal(String::new);
    let commission_status: Signal<Option<String>> = use_signal(|| None);

    let mut detail_tab = use_signal(|| DeviceDetailTab::Overview);
    let mut discovery_tab = use_signal(|| DiscoveryTab::AllDevices);

    // Filter state
    let mut filter_text = use_signal(String::new);

    // Modbus network scan state
    let mut modbus_scan_host = use_signal(|| "192.168.1.1".to_string());
    let mut modbus_scan_port = use_signal(|| "502".to_string());
    let mut modbus_scan_start = use_signal(|| "1".to_string());
    let mut modbus_scan_end = use_signal(|| "10".to_string());
    let mut scanning_modbus_network = use_signal(|| false);
    let mut modbus_scan_result: Signal<Option<String>> = use_signal(|| None);
    let mut modbus_is_rtu = use_signal(|| false);

    // Detect RTU mode on mount
    {
        let bridge_handle = state.modbus_bridge.clone();
        use_effect(move || {
            let bridge_handle = bridge_handle.clone();
            spawn(async move {
                let guard = bridge_handle.lock().await;
                if let Some(ref bridge) = *guard {
                    modbus_is_rtu.set(bridge.is_rtu());
                }
            });
        });
    }

    // Load devices when refresh_counter changes
    let ds = state.discovery_store.clone();
    let _counter = *refresh_counter.read();
    use_effect(move || {
        let ds = ds.clone();
        spawn(async move {
            let all = ds.list_devices(None).await;
            devices.set(all);
        });
    });

    // Load points when selected device changes
    let ds2 = state.discovery_store.clone();
    let sel_id = selected_device_id.read().clone();
    use_effect(move || {
        let ds2 = ds2.clone();
        let sel_id = sel_id.clone();
        spawn(async move {
            if let Some(ref id) = sel_id {
                let pts = ds2.get_points(id).await;
                selected_points.set(pts);
            } else {
                selected_points.set(vec![]);
            }
        });
    });

    let all_devices = devices.read();

    // Apply filters: text + protocol (protocol filtered by active tab)
    let active_tab = *discovery_tab.read();
    let filter_text_val = filter_text.read().clone();
    let protocol_filter: Option<&str> = match active_tab {
        DiscoveryTab::AllDevices => None,
        DiscoveryTab::Bacnet => Some(PROTOCOL_BACNET),
        DiscoveryTab::Modbus => Some(PROTOCOL_MODBUS),
    };
    let filtered_devices: Vec<&DiscoveredDevice> = all_devices
        .iter()
        .filter(|d| {
            if let Some(proto) = protocol_filter {
                if d.protocol != proto {
                    return false;
                }
            }
            if !filter_text_val.is_empty() {
                let needle = filter_text_val.to_lowercase();
                let name_match = d.display_name.to_lowercase().contains(&needle);
                let addr_match = d.address.to_lowercase().contains(&needle);
                if !name_match && !addr_match {
                    return false;
                }
            }
            true
        })
        .collect();

    let pending: Vec<&&DiscoveredDevice> = filtered_devices
        .iter()
        .filter(|d| d.state == DeviceState::Discovered)
        .collect();
    let accepted: Vec<&&DiscoveredDevice> = filtered_devices
        .iter()
        .filter(|d| d.state == DeviceState::Accepted)
        .collect();
    let ignored: Vec<&&DiscoveredDevice> = filtered_devices
        .iter()
        .filter(|d| d.state == DeviceState::Ignored)
        .collect();

    let sel = selected_device_id.read().clone();
    let selected_dev = sel
        .as_ref()
        .and_then(|id| all_devices.iter().find(|d| d.id == *id));
    let points = selected_points.read();

    let detail_dev_state = selected_dev.map(|d| d.state);
    let detail_dev_id = selected_dev.map(|d| d.id.clone());
    let detail_display = selected_dev.map(|d| d.display_name.clone());
    let detail_proto = selected_dev.map(|d| d.protocol.as_str());
    let detail_dev_protocol = selected_dev.map(|d| d.protocol.as_str());
    let detail_addr = selected_dev.map(|d| d.address.clone());
    let detail_vendor = selected_dev.and_then(|d| d.vendor.clone());
    let detail_model = selected_dev.and_then(|d| d.model.clone());
    let detail_state_str = selected_dev.map(|d| d.state.as_str());

    let is_bacnet_accepted = detail_dev_state == Some(DeviceState::Accepted)
        && detail_dev_id
            .as_ref()
            .map(|id| id.starts_with("bacnet-"))
            .unwrap_or(false);

    let is_modbus_accepted = detail_dev_state == Some(DeviceState::Accepted)
        && detail_dev_id
            .as_ref()
            .map(|id| id.starts_with("modbus-"))
            .unwrap_or(false);

    let current_detail = *detail_tab.read();

    // Compute available tabs for selected device
    let available_tabs = match (detail_dev_protocol, detail_dev_state) {
        (Some(proto), Some(st)) => tabs_for_device(proto, st),
        _ => vec![DeviceDetailTab::Overview],
    };

    // Pre-clone handles used across multiple RSX match arms
    let scan_svc = state.discovery_service.clone();
    let scan_svc_modbus = state.discovery_service.clone();
    let scan_svc_network = state.discovery_service.clone();
    let scan_svc_rtu = state.discovery_service.clone();
    let scan_bridge = state.bacnet_bridge.clone();
    let scan_modbus_bridge = state.modbus_bridge.clone();
    let scan_modbus_network_bridge = state.modbus_bridge.clone();
    let scan_modbus_rtu_bridge = state.modbus_bridge.clone();

    // Count devices per protocol for tab badges
    let bacnet_count = all_devices.iter().filter(|d| d.protocol == PROTOCOL_BACNET).count();
    let modbus_count = all_devices.iter().filter(|d| d.protocol == PROTOCOL_MODBUS).count();

    rsx! {
        div { class: "discovery-view",
            // ── Left sidebar ──
            div { class: "discovery-device-list",
                // ── Discovery sub-tab bar ──
                div { class: "discovery-tab-bar",
                    button {
                        class: if active_tab == DiscoveryTab::AllDevices { "discovery-tab active" } else { "discovery-tab" },
                        onclick: move |_| discovery_tab.set(DiscoveryTab::AllDevices),
                        "All"
                        if !all_devices.is_empty() {
                            span { class: "discovery-tab-count", "{all_devices.len()}" }
                        }
                    }
                    button {
                        class: if active_tab == DiscoveryTab::Bacnet { "discovery-tab active bacnet" } else { "discovery-tab" },
                        onclick: move |_| discovery_tab.set(DiscoveryTab::Bacnet),
                        "BACnet"
                        if bacnet_count > 0 {
                            span { class: "discovery-tab-count", "{bacnet_count}" }
                        }
                    }
                    button {
                        class: if active_tab == DiscoveryTab::Modbus { "discovery-tab active modbus" } else { "discovery-tab" },
                        onclick: move |_| discovery_tab.set(DiscoveryTab::Modbus),
                        "Modbus"
                        if modbus_count > 0 {
                            span { class: "discovery-tab-count", "{modbus_count}" }
                        }
                    }
                }

                // ── Protocol-specific toolbar (only on protocol tabs) ──
                match active_tab {
                    DiscoveryTab::Bacnet => rsx! {
                        div { class: "discovery-scan-toolbar",
                            button {
                                class: "discovery-scan-btn bacnet",
                                disabled: *scanning_bacnet.read(),
                                onclick: move |_| {
                                    scanning_bacnet.set(true);
                                    let svc = scan_svc.clone();
                                    let bridge_handle = scan_bridge.clone();
                                    spawn(async move {
                                        let mut guard = bridge_handle.lock().await;
                                        if let Some(ref mut bridge) = *guard {
                                            svc.scan_bacnet(bridge).await;
                                        }
                                        drop(guard);
                                        scanning_bacnet.set(false);
                                        bump(&mut refresh_counter);
                                    });
                                },
                                if *scanning_bacnet.read() { "Scanning..." } else { "Scan Network" }
                            }
                        }
                        if *scanning_bacnet.read() {
                            div { class: "discovery-scan-progress", "Scanning BACnet network..." }
                        }
                    },
                    DiscoveryTab::Modbus => rsx! {
                        div { class: "discovery-scan-toolbar",
                            button {
                                class: "discovery-scan-btn modbus",
                                disabled: *scanning_modbus.read(),
                                onclick: move |_| {
                                    scanning_modbus.set(true);
                                    let svc = scan_svc_modbus.clone();
                                    let bridge_handle = scan_modbus_bridge.clone();
                                    spawn(async move {
                                        let guard = bridge_handle.lock().await;
                                        if let Some(ref bridge) = *guard {
                                            svc.scan_modbus(bridge).await;
                                        }
                                        drop(guard);
                                        scanning_modbus.set(false);
                                        bump(&mut refresh_counter);
                                    });
                                },
                                if *scanning_modbus.read() { "Refreshing..." } else { "Refresh Devices" }
                            }
                        }
                        if *scanning_modbus.read() {
                            div { class: "discovery-scan-progress", "Checking configured Modbus devices..." }
                        }
                        // ── Network/RTU scan form ──
                        div { class: "discovery-scan-section",
                            h4 { class: "discovery-scan-section-title",
                                if *modbus_is_rtu.read() { "Scan RTU Bus" } else { "Scan Network" }
                            }
                            div { class: "discovery-scan-form",
                                // TCP mode: host + port row
                                if !*modbus_is_rtu.read() {
                                    div { class: "discovery-scan-row",
                                        label { class: "discovery-scan-label", "Host" }
                                        input {
                                            class: "discovery-input",
                                            r#type: "text",
                                            placeholder: "IP address",
                                            value: "{modbus_scan_host}",
                                            oninput: move |e| modbus_scan_host.set(e.value()),
                                        }
                                        label { class: "discovery-scan-label", "Port" }
                                        input {
                                            class: "discovery-input short",
                                            r#type: "number",
                                            placeholder: "502",
                                            value: "{modbus_scan_port}",
                                            oninput: move |e| modbus_scan_port.set(e.value()),
                                        }
                                    }
                                }
                                // Unit ID range row (both modes)
                                div { class: "discovery-scan-row",
                                    label { class: "discovery-scan-label", "Unit IDs" }
                                    input {
                                        class: "discovery-input short",
                                        r#type: "number",
                                        placeholder: "1",
                                        value: "{modbus_scan_start}",
                                        oninput: move |e| modbus_scan_start.set(e.value()),
                                    }
                                    span { class: "discovery-scan-separator", "to" }
                                    input {
                                        class: "discovery-input short",
                                        r#type: "number",
                                        placeholder: "10",
                                        value: "{modbus_scan_end}",
                                        oninput: move |e| modbus_scan_end.set(e.value()),
                                    }
                                    if *modbus_is_rtu.read() {
                                        button {
                                            class: "discovery-scan-btn modbus",
                                            disabled: *scanning_modbus_network.read(),
                                            onclick: {
                                                let svc = scan_svc_rtu.clone();
                                                let bridge_handle = scan_modbus_rtu_bridge.clone();
                                                move |_| {
                                                    let start: u8 = modbus_scan_start.read().parse().unwrap_or(1);
                                                    let end: u8 = modbus_scan_end.read().parse().unwrap_or(10);
                                                    let svc = svc.clone();
                                                    let bridge_handle = bridge_handle.clone();
                                                    scanning_modbus_network.set(true);
                                                    modbus_scan_result.set(None);
                                                    spawn(async move {
                                                        let guard = bridge_handle.lock().await;
                                                        let found = if let Some(ref bridge) = *guard {
                                                            svc.scan_modbus_rtu(bridge, start, end).await
                                                        } else {
                                                            0
                                                        };
                                                        drop(guard);
                                                        modbus_scan_result.set(Some(
                                                            if found == 0 {
                                                                "No responding devices found on bus.".to_string()
                                                            } else {
                                                                format!("Found {found} responding device(s) on bus.")
                                                            }
                                                        ));
                                                        scanning_modbus_network.set(false);
                                                        bump(&mut refresh_counter);
                                                    });
                                                }
                                            },
                                            if *scanning_modbus_network.read() { "Scanning..." } else { "Scan Bus" }
                                        }
                                    } else {
                                        button {
                                            class: "discovery-scan-btn modbus",
                                            disabled: *scanning_modbus_network.read(),
                                            onclick: {
                                                let svc = scan_svc_network.clone();
                                                let bridge_handle = scan_modbus_network_bridge.clone();
                                                move |_| {
                                                    let host = modbus_scan_host.read().clone();
                                                    let port: u16 = modbus_scan_port.read().parse().unwrap_or(502);
                                                    let start: u8 = modbus_scan_start.read().parse().unwrap_or(1);
                                                    let end: u8 = modbus_scan_end.read().parse().unwrap_or(10);
                                                    let svc = svc.clone();
                                                    let bridge_handle = bridge_handle.clone();
                                                    scanning_modbus_network.set(true);
                                                    modbus_scan_result.set(None);
                                                    spawn(async move {
                                                        let guard = bridge_handle.lock().await;
                                                        let found = if let Some(ref bridge) = *guard {
                                                            svc.scan_modbus_network(bridge, &host, port, start, end).await
                                                        } else {
                                                            0
                                                        };
                                                        drop(guard);
                                                        modbus_scan_result.set(Some(
                                                            if found == 0 {
                                                                "No responding devices found.".to_string()
                                                            } else {
                                                                format!("Found {found} responding device(s).")
                                                            }
                                                        ));
                                                        scanning_modbus_network.set(false);
                                                        bump(&mut refresh_counter);
                                                    });
                                                }
                                            },
                                            if *scanning_modbus_network.read() { "Scanning..." } else { "Scan" }
                                        }
                                    }
                                }
                                if *scanning_modbus_network.read() {
                                    div { class: "discovery-scan-progress", "Probing unit IDs..." }
                                }
                                if let Some(ref msg) = *modbus_scan_result.read() {
                                    div { class: "discovery-scan-result", "{msg}" }
                                }
                            }
                        }
                    },
                    DiscoveryTab::AllDevices => rsx! {},
                }

                // ── Filter bar ──
                div { class: "discovery-filter-bar",
                    input {
                        class: "discovery-filter-input",
                        r#type: "text",
                        placeholder: "Filter devices...",
                        value: "{filter_text.read()}",
                        oninput: move |evt: Event<FormData>| filter_text.set(evt.value()),
                    }
                }

                // ── Device list ──
                div { class: "discovery-device-list-body",
                    // Pending devices
                    if !pending.is_empty() {
                        div { class: "discovery-group",
                            div { class: "discovery-group-header", "Pending ({pending.len()})" }
                            for dev in pending.iter() {
                                { render_pending_device(dev, &sel, &state, selected_device_id, detail_tab, refresh_counter) }
                            }
                        }
                    }

                    // Accepted devices
                    if !accepted.is_empty() {
                        div { class: "discovery-group",
                            div { class: "discovery-group-header", "Accepted ({accepted.len()})" }
                            for dev in accepted.iter() {
                                { render_accepted_device(dev, &sel, selected_device_id, detail_tab, refresh_counter) }
                            }
                        }
                    }

                    // Ignored devices
                    if !ignored.is_empty() {
                        div { class: "discovery-group",
                            div { class: "discovery-group-header", "Ignored ({ignored.len()})" }
                            for dev in ignored.iter() {
                                { render_ignored_device(dev, &sel, &state, selected_device_id, detail_tab, refresh_counter) }
                            }
                        }
                    }

                    if filtered_devices.is_empty() {
                        div { class: "discovery-empty",
                            match active_tab {
                                DiscoveryTab::AllDevices => if all_devices.is_empty() {
                                    rsx! {
                                        p { "No devices discovered yet." }
                                        p { class: "discovery-hint", "Switch to the BACnet or Modbus tab to scan for devices." }
                                    }
                                } else {
                                    rsx! { p { "No devices match the current filter." } }
                                },
                                DiscoveryTab::Bacnet => rsx! {
                                    p { "No BACnet devices discovered." }
                                    p { class: "discovery-hint", "Click \"Scan Network\" to send a Who-Is broadcast." }
                                },
                                DiscoveryTab::Modbus => rsx! {
                                    p { "No Modbus devices found." }
                                    p { class: "discovery-hint", "Click \"Refresh Devices\" to check configured devices, or use Scan Network to probe unit IDs." }
                                },
                            }
                        }
                    }
                }

                // ── BACnet Network Tools (only on BACnet tab, below device list) ──
                if active_tab == DiscoveryTab::Bacnet {
                    div { class: "discovery-tools-section",
                        div { class: "discovery-group-header", "Network Tools" }
                        div { class: "discovery-tools-body",
                            BacnetNetworkTools {}
                        }
                    }
                }
            }

            // ── Right pane — device detail ──
            div { class: "discovery-detail",
                if let Some(ref display) = detail_display {
                    // Device header (always visible)
                    div { class: "discovery-detail-header",
                        h3 { "{display}" }
                        div { class: "discovery-detail-meta",
                            if let Some(proto) = detail_proto {
                                span {
                                    class: if proto == "bacnet" { "discovery-meta-chip protocol-bacnet" } else { "discovery-meta-chip protocol-modbus" },
                                    "{proto}"
                                }
                            }
                            if let Some(ref addr) = detail_addr {
                                span { class: "discovery-meta-chip", "Address: {addr}" }
                            }
                            if let Some(ref v) = detail_vendor {
                                span { class: "discovery-meta-chip", "Vendor: {v}" }
                            }
                            if let Some(ref m) = detail_model {
                                span { class: "discovery-meta-chip", "Model: {m}" }
                            }
                            if let Some(st) = detail_state_str {
                                span { class: "discovery-meta-chip", "State: {st}" }
                            }
                        }

                        // Accept/Ignore for pending devices (admin only for accept)
                        if detail_dev_state == Some(DeviceState::Discovered) && user_is_admin {
                            div { class: "discovery-detail-actions",
                                {
                                    let accept_id = detail_dev_id.clone().unwrap_or_default();
                                    let ignore_id = accept_id.clone();
                                    let svc = state.discovery_service.clone();
                                    let svc2 = state.discovery_service.clone();
                                    let accept_audit = state.clone();
                                    rsx! {
                                        button {
                                            class: "discovery-action-btn accept primary",
                                            onclick: move |_| {
                                                let svc = svc.clone();
                                                let id = accept_id.clone();
                                                let audit_state = accept_audit.clone();
                                                spawn(async move {
                                                    if let Err(e) = svc.accept_device(&id).await {
                                                        eprintln!("Accept failed: {e}");
                                                        audit_state.audit(
                                                            crate::store::audit_store::AuditEntryBuilder::new(
                                                                crate::store::audit_store::AuditAction::AcceptDevice, "device",
                                                            ).resource_id(&id).failure(&format!("{e}")),
                                                        );
                                                    } else {
                                                        audit_state.audit(
                                                            crate::store::audit_store::AuditEntryBuilder::new(
                                                                crate::store::audit_store::AuditAction::AcceptDevice, "device",
                                                            ).resource_id(&id),
                                                        );
                                                    }
                                                    bump(&mut refresh_counter);
                                                });
                                            },
                                            "Accept Device"
                                        }
                                        button {
                                            class: "discovery-action-btn ignore",
                                            onclick: move |_| {
                                                let svc2 = svc2.clone();
                                                let id = ignore_id.clone();
                                                spawn(async move {
                                                    let _ = svc2.ignore_device(&id).await;
                                                    bump(&mut refresh_counter);
                                                });
                                            },
                                            "Ignore"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Detail tab bar (protocol-aware)
                    if available_tabs.len() > 1 {
                        div { class: "discovery-detail-tab-bar",
                            for tab in available_tabs.iter() {
                                {
                                    let t = *tab;
                                    rsx! {
                                        button {
                                            class: if current_detail == t { "discovery-detail-tab active" } else { "discovery-detail-tab" },
                                            onclick: move |_| detail_tab.set(t),
                                            "{t.label()}"
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Detail tab content
                    div { class: "discovery-detail-body",
                        match current_detail {
                            DeviceDetailTab::Overview => rsx! {
                                // Point table
                                if !points.is_empty() {
                                    div { class: "discovery-point-table-wrapper",
                                        h4 { "Points ({points.len()})" }
                                        table { class: "discovery-point-table",
                                            thead {
                                                tr {
                                                    th { "Name" }
                                                    th { "Description" }
                                                    th { "Units" }
                                                    th { "Kind" }
                                                    th { "Writable" }
                                                }
                                            }
                                            tbody {
                                                for pt in points.iter() {
                                                    tr { key: "{pt.id}",
                                                        td { "{pt.display_name}" }
                                                        td { class: "text-muted", "{pt.description.as_deref().unwrap_or(\"—\")}" }
                                                        td { "{pt.units.as_deref().unwrap_or(\"—\")}" }
                                                        td {
                                                            span { class: "discovery-kind-badge", "{kind_label(pt.point_kind)}" }
                                                        }
                                                        td {
                                                            if pt.writable { "Yes" } else { "—" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                } else {
                                    div { class: "discovery-tab-empty",
                                        if detail_dev_protocol == Some("modbus") {
                                            p { "No points mapped for this device." }
                                            p { class: "discovery-hint",
                                                "Modbus devices don't self-describe their registers. "
                                                "Points are defined in device profiles (profiles/*.json) "
                                                "and mapped to register addresses. Use the Registers tab "
                                                "to browse raw registers on accepted devices."
                                            }
                                        } else {
                                            "No points discovered for this device."
                                        }
                                    }
                                }
                            },
                            DeviceDetailTab::BacnetManagement if is_bacnet_accepted => rsx! {
                                { render_bacnet_management(&state, &detail_dev_id, event_infos, trend_logs) }
                            },
                            DeviceDetailTab::BacnetAlarms if is_bacnet_accepted => rsx! {
                                if let Some(inst) = detail_dev_id.as_ref().and_then(|id| extract_bacnet_instance(id)) {
                                    BacnetDeviceAlarms { device_instance: inst }
                                }
                            },
                            DeviceDetailTab::BacnetTrends if is_bacnet_accepted => rsx! {
                                if let Some(inst) = detail_dev_id.as_ref().and_then(|id| extract_bacnet_instance(id)) {
                                    BacnetDeviceTrends { device_instance: inst }
                                }
                            },
                            DeviceDetailTab::BacnetFiles if is_bacnet_accepted => rsx! {
                                if let Some(inst) = detail_dev_id.as_ref().and_then(|id| extract_bacnet_instance(id)) {
                                    BacnetDeviceFiles { device_instance: inst }
                                }
                            },
                            DeviceDetailTab::BacnetAdvanced if is_bacnet_accepted => rsx! {
                                if let Some(inst) = detail_dev_id.as_ref().and_then(|id| extract_bacnet_instance(id)) {
                                    BacnetDeviceAdvanced { device_instance: inst }
                                }
                            },
                            DeviceDetailTab::BacnetCommission if is_bacnet_accepted => rsx! {
                                if let Some(inst) = detail_dev_id.as_ref().and_then(|id| extract_bacnet_instance(id)) {
                                    { render_bacnet_commission(&state, inst, create_object_type, delete_object_input, commission_status) }
                                }
                            },
                            DeviceDetailTab::ModbusRegisters if is_modbus_accepted => rsx! {
                                if let Some(ref dev_id) = detail_dev_id {
                                    {
                                        let instance_id = extract_modbus_instance_id(dev_id);
                                        rsx! {
                                            ModbusDeviceRegisters { device_id: instance_id }
                                        }
                                    }
                                }
                            },
                            DeviceDetailTab::ModbusDiagnostics if is_modbus_accepted => rsx! {
                                if let Some(ref dev_id) = detail_dev_id {
                                    {
                                        let instance_id = extract_modbus_instance_id(dev_id);
                                        rsx! {
                                            ModbusDeviceDiagnostics { device_id: instance_id }
                                        }
                                    }
                                }
                            },
                            _ => rsx! {
                                if !points.is_empty() {
                                    div { class: "discovery-point-table-wrapper",
                                        h4 { "Points ({points.len()})" }
                                        table { class: "discovery-point-table",
                                            thead {
                                                tr {
                                                    th { "Name" }
                                                    th { "Description" }
                                                    th { "Units" }
                                                    th { "Kind" }
                                                    th { "Writable" }
                                                }
                                            }
                                            tbody {
                                                for pt in points.iter() {
                                                    tr { key: "{pt.id}",
                                                        td { "{pt.display_name}" }
                                                        td { class: "text-muted", "{pt.description.as_deref().unwrap_or(\"—\")}" }
                                                        td { "{pt.units.as_deref().unwrap_or(\"—\")}" }
                                                        td {
                                                            span { class: "discovery-kind-badge", "{kind_label(pt.point_kind)}" }
                                                        }
                                                        td {
                                                            if pt.writable { "Yes" } else { "—" }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            },
                        }
                    }
                } else {
                    div { class: "discovery-detail-empty",
                        p { "Select a device to view its details and points." }
                    }
                }
            }
        }
    }
}

// ── Helper render functions to reduce nesting ──

fn render_pending_device(
    dev: &DiscoveredDevice,
    sel: &Option<String>,
    state: &AppState,
    mut selected_device_id: Signal<Option<String>>,
    mut detail_tab: Signal<DeviceDetailTab>,
    mut refresh_counter: Signal<u64>,
) -> Element {
    let dev_id = dev.id.clone();
    let dev_id2 = dev.id.clone();
    let dev_id3 = dev.id.clone();
    let is_selected = sel.as_deref() == Some(&dev.id);
    let svc = state.discovery_service.clone();
    let svc2 = state.discovery_service.clone();
    let badge_class = protocol_badge_class(&dev.protocol);
    rsx! {
        div {
            class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
            onclick: move |_| {
                selected_device_id.set(Some(dev_id.clone()));
                detail_tab.set(DeviceDetailTab::Overview);
                bump(&mut refresh_counter);
            },
            span { class: "discovery-protocol-badge {badge_class}", "{protocol_badge(&dev.protocol)}" }
            span { class: "discovery-device-name", "{dev.display_name}" }
            span { class: "discovery-device-addr", "{dev.address}" }
            if dev.point_count > 0 {
                span { class: "discovery-point-count", "{dev.point_count} pts" }
            }
            div { class: "discovery-actions",
                if state.has_permission(Permission::ManageDiscovery) {
                    button {
                        class: "discovery-action-btn accept",
                        onclick: {
                            let list_audit = state.clone();
                            move |evt: Event<MouseData>| {
                                evt.stop_propagation();
                                let svc = svc.clone();
                                let id = dev_id2.clone();
                                let audit_state = list_audit.clone();
                                spawn(async move {
                                    if let Err(e) = svc.accept_device(&id).await {
                                        eprintln!("Accept failed: {e}");
                                        audit_state.audit(
                                            crate::store::audit_store::AuditEntryBuilder::new(
                                                crate::store::audit_store::AuditAction::AcceptDevice, "device",
                                            ).resource_id(&id).failure(&format!("{e}")),
                                        );
                                    } else {
                                        audit_state.audit(
                                            crate::store::audit_store::AuditEntryBuilder::new(
                                                crate::store::audit_store::AuditAction::AcceptDevice, "device",
                                            ).resource_id(&id),
                                        );
                                    }
                                    bump(&mut refresh_counter);
                                });
                            }
                        },
                        "Accept"
                    }
                }
                button {
                    class: "discovery-action-btn ignore",
                    onclick: move |evt| {
                        evt.stop_propagation();
                        let svc2 = svc2.clone();
                        let id = dev_id3.clone();
                        spawn(async move {
                            let _ = svc2.ignore_device(&id).await;
                            bump(&mut refresh_counter);
                        });
                    },
                    "Ignore"
                }
            }
        }
    }
}

fn render_accepted_device(
    dev: &DiscoveredDevice,
    sel: &Option<String>,
    mut selected_device_id: Signal<Option<String>>,
    mut detail_tab: Signal<DeviceDetailTab>,
    mut refresh_counter: Signal<u64>,
) -> Element {
    let dev_id = dev.id.clone();
    let is_selected = sel.as_deref() == Some(&dev.id);
    let badge_class = protocol_badge_class(&dev.protocol);
    rsx! {
        div {
            class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
            onclick: move |_| {
                selected_device_id.set(Some(dev_id.clone()));
                detail_tab.set(DeviceDetailTab::Overview);
                bump(&mut refresh_counter);
            },
            span { class: "discovery-protocol-badge {badge_class}", "{protocol_badge(&dev.protocol)}" }
            span { class: "discovery-device-name", "{dev.display_name}" }
            span { class: "discovery-device-addr", "{dev.address}" }
            ConnBadge { status: dev.conn_status }
            if dev.point_count > 0 {
                span { class: "discovery-point-count", "{dev.point_count} pts" }
            }
        }
    }
}

fn render_ignored_device(
    dev: &DiscoveredDevice,
    sel: &Option<String>,
    state: &AppState,
    mut selected_device_id: Signal<Option<String>>,
    mut detail_tab: Signal<DeviceDetailTab>,
    mut refresh_counter: Signal<u64>,
) -> Element {
    let dev_id = dev.id.clone();
    let dev_id2 = dev.id.clone();
    let is_selected = sel.as_deref() == Some(&dev.id);
    let svc = state.discovery_service.clone();
    let badge_class = protocol_badge_class(&dev.protocol);
    rsx! {
        div {
            class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
            onclick: move |_| {
                selected_device_id.set(Some(dev_id.clone()));
                detail_tab.set(DeviceDetailTab::Overview);
                bump(&mut refresh_counter);
            },
            span { class: "discovery-protocol-badge {badge_class}", "{protocol_badge(&dev.protocol)}" }
            span { class: "discovery-device-name dimmed", "{dev.display_name}" }
            button {
                class: "discovery-action-btn",
                onclick: move |evt| {
                    evt.stop_propagation();
                    let svc = svc.clone();
                    let id = dev_id2.clone();
                    spawn(async move {
                        let _ = svc.unignore_device(&id).await;
                        bump(&mut refresh_counter);
                    });
                },
                "Un-ignore"
            }
        }
    }
}

fn render_bacnet_management(
    state: &AppState,
    detail_dev_id: &Option<String>,
    mut event_infos: Signal<Vec<crate::bridge::bacnet::BacnetEventInfo>>,
    mut trend_logs: Signal<Vec<(u32, String)>>,
) -> Element {
    let mgmt_dev_id = detail_dev_id.clone().unwrap_or_default();
    let instance = extract_bacnet_instance(&mgmt_dev_id);
    let bridge_warmstart = state.bacnet_bridge.clone();
    let bridge_coldstart = state.bacnet_bridge.clone();
    let bridge_sync = state.bacnet_bridge.clone();
    let bridge_disable = state.bacnet_bridge.clone();
    let bridge_enable = state.bacnet_bridge.clone();
    let bridge_events = state.bacnet_bridge.clone();
    let bridge_trendlogs = state.bacnet_bridge.clone();
    rsx! {
        if let Some(inst) = instance {
            div { class: "discovery-mgmt-section",
                h4 { "Device Control" }
                div { class: "discovery-mgmt-grid",
                    button {
                        class: "discovery-mgmt-btn",
                        onclick: move |_| {
                            let bridge = bridge_sync.clone();
                            spawn(async move {
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    if let Err(e) = b.sync_time(inst).await {
                                        eprintln!("Time sync failed: {e}");
                                    }
                                }
                            });
                        },
                        div { class: "discovery-mgmt-btn-icon", "🕐" }
                        span { "Sync Time" }
                    }
                    button {
                        class: "discovery-mgmt-btn",
                        onclick: move |_| {
                            let bridge = bridge_warmstart.clone();
                            spawn(async move {
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    if let Err(e) = b.reinitialize_device(inst, true).await {
                                        eprintln!("Warmstart failed: {e}");
                                    }
                                }
                            });
                        },
                        div { class: "discovery-mgmt-btn-icon", "↻" }
                        span { "Warmstart" }
                    }
                    button {
                        class: "discovery-mgmt-btn warn",
                        onclick: move |_| {
                            let bridge = bridge_coldstart.clone();
                            spawn(async move {
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    if let Err(e) = b.reinitialize_device(inst, false).await {
                                        eprintln!("Coldstart failed: {e}");
                                    }
                                }
                            });
                        },
                        div { class: "discovery-mgmt-btn-icon", "⚡" }
                        span { "Coldstart" }
                    }
                    button {
                        class: "discovery-mgmt-btn warn",
                        onclick: move |_| {
                            let bridge = bridge_disable.clone();
                            spawn(async move {
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    if let Err(e) = b.device_communication_control(inst, false, Some(30)).await {
                                        eprintln!("Disable comm failed: {e}");
                                    }
                                }
                            });
                        },
                        div { class: "discovery-mgmt-btn-icon", "⏸" }
                        span { "Disable Comm" }
                    }
                    button {
                        class: "discovery-mgmt-btn",
                        onclick: move |_| {
                            let bridge = bridge_enable.clone();
                            spawn(async move {
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    if let Err(e) = b.device_communication_control(inst, true, None).await {
                                        eprintln!("Enable comm failed: {e}");
                                    }
                                }
                            });
                        },
                        div { class: "discovery-mgmt-btn-icon", "▶" }
                        span { "Enable Comm" }
                    }
                }
            }

            // Events section
            div { class: "discovery-mgmt-section",
                h4 { "Event Information" }
                button {
                    class: "discovery-action-btn",
                    onclick: move |_| {
                        let bridge = bridge_events.clone();
                        spawn(async move {
                            let guard = bridge.lock().await;
                            if let Some(ref b) = *guard {
                                match b.get_event_info(inst).await {
                                    Ok(events) => event_infos.set(events),
                                    Err(e) => eprintln!("GetEventInfo failed: {e}"),
                                }
                            }
                        });
                    },
                    "Fetch Events"
                }
                if !event_infos.read().is_empty() {
                    table { class: "discovery-point-table",
                        thead {
                            tr {
                                th { "Object" }
                                th { "State" }
                                th { "Ack" }
                            }
                        }
                        tbody {
                            for ev in event_infos.read().iter() {
                                tr {
                                    td { "{ev.object_id.object_type()}-{ev.object_id.instance()}" }
                                    td { "{event_state_label(ev.event_state)}" }
                                    td {
                                        if let Some(ref bits) = ev.acknowledged_transitions {
                                            if bits.is_empty() || bits[0] == 0 { "Unacked" } else { "Acked" }
                                        } else {
                                            "Unknown"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // TrendLog backfill section
            div { class: "discovery-mgmt-section",
                h4 { "Trend Log Backfill" }
                button {
                    class: "discovery-action-btn",
                    onclick: move |_| {
                        let bridge = bridge_trendlogs.clone();
                        spawn(async move {
                            let guard = bridge.lock().await;
                            if let Some(ref b) = *guard {
                                let tls: Vec<(u32, String)> = b.discovered_devices()
                                    .iter()
                                    .filter(|d| d.device_id.instance() == inst)
                                    .flat_map(|d| d.trend_logs.iter())
                                    .map(|tl| (tl.object_id.instance(), tl.object_name.clone().unwrap_or_else(|| format!("TrendLog-{}", tl.object_id.instance()))))
                                    .collect();
                                trend_logs.set(tls);
                            }
                        });
                    },
                    "Load TrendLogs"
                }
                if !trend_logs.read().is_empty() {
                    table { class: "discovery-point-table",
                        thead {
                            tr {
                                th { "Name" }
                                th { "Instance" }
                                th { "" }
                            }
                        }
                        tbody {
                            for (tl_inst, tl_name) in trend_logs.read().iter() {
                                {
                                    let backfill_bridge = state.bacnet_bridge.clone();
                                    let backfill_history = state.history_store.clone();
                                    let dev_key_tl = mgmt_dev_id.clone();
                                    let tl_i = *tl_inst;
                                    let tl_n = tl_name.clone();
                                    rsx! {
                                        tr {
                                            td { "{tl_name}" }
                                            td { "{tl_inst}" }
                                            td {
                                                button {
                                                    class: "discovery-action-btn",
                                                    onclick: move |_| {
                                                        let bridge = backfill_bridge.clone();
                                                        let history = backfill_history.clone();
                                                        let dk = dev_key_tl.clone();
                                                        let tn = tl_n.clone();
                                                        spawn(async move {
                                                            let guard = bridge.lock().await;
                                                            if let Some(ref b) = *guard {
                                                                if let Some(inst) = dk.strip_prefix("bacnet-").and_then(|s| s.parse::<u32>().ok()) {
                                                                    match b.backfill_trend_log(inst, tl_i, &dk, &tn, &history).await {
                                                                        Ok(n) => println!("Backfilled {n} records from TrendLog-{tl_i}"),
                                                                        Err(e) => eprintln!("Backfill failed: {e}"),
                                                                    }
                                                                }
                                                            }
                                                        });
                                                    },
                                                    "Backfill"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn render_bacnet_commission(
    state: &AppState,
    inst: u32,
    mut create_object_type: Signal<String>,
    mut delete_object_input: Signal<String>,
    commission_status: Signal<Option<String>>,
) -> Element {
    let bridge_create = state.bacnet_bridge.clone();
    let bridge_delete = state.bacnet_bridge.clone();
    let mut cs_create = commission_status;
    let mut cs_delete = commission_status;
    let mut cs_delete_err = commission_status;
    rsx! {
        div { class: "discovery-commission-panel",
            div { class: "discovery-mgmt-section",
                h4 { "Create Object" }
                div { class: "discovery-form-row",
                    select {
                        class: "discovery-input",
                        value: "{create_object_type}",
                        onchange: move |e| create_object_type.set(e.value()),
                        option { value: "AnalogValue", "Analog Value" }
                        option { value: "BinaryValue", "Binary Value" }
                        option { value: "MultiStateValue", "Multi-State Value" }
                        option { value: "AnalogInput", "Analog Input" }
                        option { value: "AnalogOutput", "Analog Output" }
                    }
                    button {
                        class: "discovery-action-btn",
                        onclick: move |_| {
                            let bridge = bridge_create.clone();
                            let obj_type_str = create_object_type.read().clone();
                            spawn(async move {
                                let obj_type = match obj_type_str.as_str() {
                                    "AnalogValue" => rustbac_core::types::ObjectType::AnalogValue,
                                    "BinaryValue" => rustbac_core::types::ObjectType::BinaryValue,
                                    "MultiStateValue" => rustbac_core::types::ObjectType::MultiStateValue,
                                    "AnalogInput" => rustbac_core::types::ObjectType::AnalogInput,
                                    "AnalogOutput" => rustbac_core::types::ObjectType::AnalogOutput,
                                    _ => return,
                                };
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    match b.create_object(inst, obj_type).await {
                                        Ok(created_id) => {
                                            cs_create.set(Some(format!("Created: {}-{}", created_id.object_type(), created_id.instance())));
                                        }
                                        Err(e) => cs_create.set(Some(format!("Create failed: {e}"))),
                                    }
                                }
                            });
                        },
                        "Create Object"
                    }
                }
            }
            div { class: "discovery-mgmt-section",
                h4 { "Delete Object" }
                div { class: "discovery-form-row",
                    input {
                        r#type: "text",
                        class: "discovery-input",
                        placeholder: "Object instance to delete",
                        value: "{delete_object_input}",
                        oninput: move |e| delete_object_input.set(e.value()),
                    }
                    button {
                        class: "discovery-action-btn warn",
                        onclick: move |_| {
                            let bridge = bridge_delete.clone();
                            let obj_inst_str = delete_object_input.read().clone();
                            let obj_type_str = create_object_type.read().clone();
                            spawn(async move {
                                let obj_inst: u32 = match obj_inst_str.parse() {
                                    Ok(v) => v,
                                    Err(_) => { cs_delete_err.set(Some("Invalid instance".into())); return; }
                                };
                                let obj_type = match obj_type_str.as_str() {
                                    "AnalogValue" => rustbac_core::types::ObjectType::AnalogValue,
                                    "BinaryValue" => rustbac_core::types::ObjectType::BinaryValue,
                                    "MultiStateValue" => rustbac_core::types::ObjectType::MultiStateValue,
                                    "AnalogInput" => rustbac_core::types::ObjectType::AnalogInput,
                                    "AnalogOutput" => rustbac_core::types::ObjectType::AnalogOutput,
                                    _ => return,
                                };
                                let object_id = rustbac_core::types::ObjectId::new(obj_type, obj_inst);
                                let guard = bridge.lock().await;
                                if let Some(ref b) = *guard {
                                    match b.delete_object(inst, object_id).await {
                                        Ok(()) => cs_delete.set(Some(format!("Deleted {obj_type_str}-{obj_inst}"))),
                                        Err(e) => cs_delete.set(Some(format!("Delete failed: {e}"))),
                                    }
                                }
                            });
                        },
                        "Delete Object"
                    }
                }
            }
            if let Some(ref status) = *commission_status.read() {
                div { class: "discovery-status-msg", "{status}" }
            }
        }
    }
}

#[component]
fn ConnBadge(status: ConnStatus) -> Element {
    let (class, label) = match status {
        ConnStatus::Online => ("discovery-status-badge online", "Online"),
        ConnStatus::Offline => ("discovery-status-badge offline", "Offline"),
        ConnStatus::Unknown => ("discovery-status-badge unknown", "Unknown"),
    };
    rsx! {
        span { class: "{class}", "{label}" }
    }
}

fn protocol_badge(proto: &str) -> &'static str {
    match proto {
        "bacnet" => "B",
        "modbus" => "M",
        _ => "?",
    }
}

fn protocol_badge_class(proto: &str) -> &'static str {
    match proto {
        "bacnet" => "bacnet",
        "modbus" => "modbus",
        _ => "unknown",
    }
}

fn kind_label(kind: crate::discovery::model::PointKindHint) -> &'static str {
    match kind {
        crate::discovery::model::PointKindHint::Analog => "A",
        crate::discovery::model::PointKindHint::Binary => "B",
        crate::discovery::model::PointKindHint::Multistate => "M",
    }
}

/// Extract BACnet device instance number from device ID (e.g., "bacnet-1000" → Some(1000)).
fn extract_bacnet_instance(device_id: &str) -> Option<u32> {
    device_id.strip_prefix("bacnet-")?.parse().ok()
}

/// Extract the Modbus instance_id from a discovery device_id like "modbus-vav-101".
fn extract_modbus_instance_id(device_id: &str) -> String {
    device_id.strip_prefix("modbus-").unwrap_or(device_id).to_string()
}

fn event_state_label(state: u32) -> &'static str {
    match state {
        0 => "Normal",
        1 => "Fault",
        2 => "Offnormal",
        3 => "High Limit",
        4 => "Low Limit",
        5 => "Life Safety",
        _ => "Unknown",
    }
}
