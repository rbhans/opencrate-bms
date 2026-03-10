use dioxus::prelude::*;

use crate::discovery::model::{ConnStatus, DeviceState, DiscoveredDevice, DiscoveredPoint};
use crate::gui::state::AppState;

/// Helper to increment a signal by 1 without borrow conflicts.
fn bump(sig: &mut Signal<u64>) {
    let v = *sig.read();
    sig.set(v + 1);
}

#[component]
pub fn DiscoveryView() -> Element {
    let state = use_context::<AppState>();
    let mut devices = use_signal(Vec::<DiscoveredDevice>::new);
    let mut selected_device_id = use_signal(|| Option::<String>::None);
    let mut selected_points = use_signal(Vec::<DiscoveredPoint>::new);
    let mut scanning = use_signal(|| false);
    let mut refresh_counter = use_signal(|| 0u64);

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
    let pending: Vec<&DiscoveredDevice> = all_devices
        .iter()
        .filter(|d| d.state == DeviceState::Discovered)
        .collect();
    let accepted: Vec<&DiscoveredDevice> = all_devices
        .iter()
        .filter(|d| d.state == DeviceState::Accepted)
        .collect();
    let ignored: Vec<&DiscoveredDevice> = all_devices
        .iter()
        .filter(|d| d.state == DeviceState::Ignored)
        .collect();

    let sel = selected_device_id.read().clone();
    let selected_dev = sel
        .as_ref()
        .and_then(|id| all_devices.iter().find(|d| d.id == *id));
    let points = selected_points.read();

    // Clone what we need for detail pane closures
    let detail_dev_state = selected_dev.map(|d| d.state);
    let detail_dev_id = selected_dev.map(|d| d.id.clone());
    let detail_display = selected_dev.map(|d| d.display_name.clone());
    let detail_proto = selected_dev.map(|d| d.protocol.as_str());
    let detail_addr = selected_dev.map(|d| d.address.clone());
    let detail_vendor = selected_dev.and_then(|d| d.vendor.clone());
    let detail_model = selected_dev.and_then(|d| d.model.clone());
    let detail_state_str = selected_dev.map(|d| d.state.as_str());

    rsx! {
        div { class: "discovery-view",
            // Left pane — device list
            div { class: "discovery-device-list",
                div { class: "discovery-header",
                    h3 { "Discovered Devices" }
                    span { class: "discovery-count", "{all_devices.len()}" }
                    button {
                        class: "discovery-scan-btn",
                        disabled: *scanning.read(),
                        onclick: move |_| {
                            scanning.set(true);
                            let svc = state.discovery_service.clone();
                            let bridge_handle = state.bacnet_bridge.clone();
                            spawn(async move {
                                let guard = bridge_handle.lock().await;
                                if let Some(ref bridge) = *guard {
                                    svc.scan_bacnet(bridge).await;
                                }
                                drop(guard);
                                scanning.set(false);
                                bump(&mut refresh_counter);
                            });
                        },
                        if *scanning.read() { "Scanning..." } else { "Scan" }
                    }
                }

                // Pending devices
                if !pending.is_empty() {
                    div { class: "discovery-group",
                        div { class: "discovery-group-header", "Pending ({pending.len()})" }
                        for dev in pending.iter() {
                            {
                                let dev_id = dev.id.clone();
                                let dev_id2 = dev.id.clone();
                                let dev_id3 = dev.id.clone();
                                let is_selected = sel.as_deref() == Some(&dev.id);
                                let svc = state.discovery_service.clone();
                                let svc2 = state.discovery_service.clone();
                                rsx! {
                                    div {
                                        class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
                                        onclick: move |_| {
                                            selected_device_id.set(Some(dev_id.clone()));
                                            bump(&mut refresh_counter);
                                        },
                                        span { class: "discovery-protocol-badge bacnet", "{protocol_badge(dev.protocol)}" }
                                        span { class: "discovery-device-name", "{dev.display_name}" }
                                        span { class: "discovery-device-addr", "{dev.address}" }
                                        span { class: "discovery-point-count", "{dev.point_count} pts" }
                                        div { class: "discovery-actions",
                                            button {
                                                class: "discovery-action-btn accept",
                                                onclick: move |evt| {
                                                    evt.stop_propagation();
                                                    let svc = svc.clone();
                                                    let id = dev_id2.clone();
                                                    spawn(async move {
                                                        if let Err(e) = svc.accept_device(&id).await {
                                                            eprintln!("Accept failed: {e}");
                                                        }
                                                        bump(&mut refresh_counter);
                                                    });
                                                },
                                                "Accept"
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
                        }
                    }
                }

                // Accepted devices
                if !accepted.is_empty() {
                    div { class: "discovery-group",
                        div { class: "discovery-group-header", "Accepted ({accepted.len()})" }
                        for dev in accepted.iter() {
                            {
                                let dev_id = dev.id.clone();
                                let is_selected = sel.as_deref() == Some(&dev.id);
                                rsx! {
                                    div {
                                        class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
                                        onclick: move |_| {
                                            selected_device_id.set(Some(dev_id.clone()));
                                            bump(&mut refresh_counter);
                                        },
                                        span { class: "discovery-protocol-badge bacnet", "{protocol_badge(dev.protocol)}" }
                                        span { class: "discovery-device-name", "{dev.display_name}" }
                                        span { class: "discovery-device-addr", "{dev.address}" }
                                        ConnBadge { status: dev.conn_status }
                                        span { class: "discovery-point-count", "{dev.point_count} pts" }
                                    }
                                }
                            }
                        }
                    }
                }

                // Ignored devices
                if !ignored.is_empty() {
                    div { class: "discovery-group",
                        div { class: "discovery-group-header", "Ignored ({ignored.len()})" }
                        for dev in ignored.iter() {
                            {
                                let dev_id = dev.id.clone();
                                let dev_id2 = dev.id.clone();
                                let is_selected = sel.as_deref() == Some(&dev.id);
                                let svc = state.discovery_service.clone();
                                rsx! {
                                    div {
                                        class: if is_selected { "discovery-device-row selected" } else { "discovery-device-row" },
                                        onclick: move |_| {
                                            selected_device_id.set(Some(dev_id.clone()));
                                            bump(&mut refresh_counter);
                                        },
                                        span { class: "discovery-protocol-badge bacnet", "{protocol_badge(dev.protocol)}" }
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
                        }
                    }
                }

                if all_devices.is_empty() {
                    div { class: "discovery-empty",
                        p { "No devices discovered yet." }
                        p { class: "discovery-hint", "Click \"Scan\" to discover BACnet devices on the network." }
                    }
                }
            }

            // Right pane — device detail
            div { class: "discovery-detail",
                if let Some(ref display) = detail_display {
                    div { class: "discovery-detail-header",
                        h3 { "{display}" }
                        div { class: "discovery-detail-meta",
                            if let Some(proto) = detail_proto {
                                span { "Protocol: {proto}" }
                            }
                            if let Some(ref addr) = detail_addr {
                                span { "Address: {addr}" }
                            }
                            if let Some(ref v) = detail_vendor {
                                span { "Vendor: {v}" }
                            }
                            if let Some(ref m) = detail_model {
                                span { "Model: {m}" }
                            }
                            if let Some(st) = detail_state_str {
                                span { "State: {st}" }
                            }
                        }

                        if detail_dev_state == Some(DeviceState::Discovered) {
                            div { class: "discovery-detail-actions",
                                {
                                    let accept_id = detail_dev_id.clone().unwrap_or_default();
                                    let ignore_id = accept_id.clone();
                                    let svc = state.discovery_service.clone();
                                    let svc2 = state.discovery_service.clone();
                                    rsx! {
                                        button {
                                            class: "discovery-action-btn accept primary",
                                            onclick: move |_| {
                                                let svc = svc.clone();
                                                let id = accept_id.clone();
                                                spawn(async move {
                                                    if let Err(e) = svc.accept_device(&id).await {
                                                        eprintln!("Accept failed: {e}");
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

                        // Device management actions for accepted BACnet devices
                        if detail_dev_state == Some(DeviceState::Accepted) {
                            div { class: "discovery-detail-actions",
                                {
                                    let mgmt_dev_id = detail_dev_id.clone().unwrap_or_default();
                                    let instance = extract_bacnet_instance(&mgmt_dev_id);
                                    let bridge_reboot = state.bacnet_bridge.clone();
                                    let bridge_sync = state.bacnet_bridge.clone();
                                    let bridge_disable = state.bacnet_bridge.clone();
                                    let bridge_enable = state.bacnet_bridge.clone();
                                    rsx! {
                                        if let Some(inst) = instance {
                                            button {
                                                class: "discovery-action-btn",
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
                                                "Sync Time"
                                            }
                                            button {
                                                class: "discovery-action-btn",
                                                onclick: move |_| {
                                                    let bridge = bridge_reboot.clone();
                                                    spawn(async move {
                                                        let guard = bridge.lock().await;
                                                        if let Some(ref b) = *guard {
                                                            if let Err(e) = b.reinitialize_device(inst, true).await {
                                                                eprintln!("Reboot failed: {e}");
                                                            }
                                                        }
                                                    });
                                                },
                                                "Reboot"
                                            }
                                            button {
                                                class: "discovery-action-btn",
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
                                                "Disable Comm"
                                            }
                                            button {
                                                class: "discovery-action-btn",
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
                                                "Enable Comm"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

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

fn protocol_badge(proto: crate::discovery::model::DiscoveryProtocol) -> &'static str {
    match proto {
        crate::discovery::model::DiscoveryProtocol::Bacnet => "B",
        crate::discovery::model::DiscoveryProtocol::Modbus => "M",
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
