use std::collections::HashSet;

use dioxus::prelude::*;

use crate::config::profile::PointKind;
use crate::gui::state::AppState;
use crate::store::alarm_store::{
    ActiveAlarm, AlarmConfig, AlarmEvent, AlarmHistoryQuery,
    AlarmParams, AlarmSeverity, AlarmState,
};

// ----------------------------------------------------------------
// Tab state
// ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum AlarmTab {
    Active,
    History,
    Config,
}

// ----------------------------------------------------------------
// AlarmView — 3-pane layout (browser | tabs | template)
// ----------------------------------------------------------------

#[component]
pub fn AlarmView() -> Element {
    let mut tab = use_signal(|| AlarmTab::Active);
    let selected_points: Signal<HashSet<(String, String)>> = use_signal(HashSet::new);
    let search = use_signal(String::new);
    let bulk_status: Signal<Option<String>> = use_signal(|| None);
    let current_tab = *tab.read();

    rsx! {
        AlarmDeviceBrowser { selected_points, search }
        div { class: "main-content",
            div { class: "alarm-tabs",
                button {
                    class: if current_tab == AlarmTab::Active { "alarm-tab active" } else { "alarm-tab" },
                    onclick: move |_| tab.set(AlarmTab::Active),
                    "Active Alarms"
                }
                button {
                    class: if current_tab == AlarmTab::History { "alarm-tab active" } else { "alarm-tab" },
                    onclick: move |_| tab.set(AlarmTab::History),
                    "History"
                }
                button {
                    class: if current_tab == AlarmTab::Config { "alarm-tab active" } else { "alarm-tab" },
                    onclick: move |_| tab.set(AlarmTab::Config),
                    "Config"
                }
            }

            div { class: "alarm-tab-content",
                match current_tab {
                    AlarmTab::Active => rsx! { ActiveAlarmsTab {} },
                    AlarmTab::History => rsx! { AlarmHistoryTab {} },
                    AlarmTab::Config => rsx! { AlarmConfigTab {} },
                }
            }
        }
        AlarmTemplatePanel { selected_points, bulk_status }
    }
}

// ----------------------------------------------------------------
// Left pane: Device/Point Browser with checkboxes
// ----------------------------------------------------------------

#[component]
fn AlarmDeviceBrowser(
    selected_points: Signal<HashSet<(String, String)>>,
    search: Signal<String>,
) -> Element {
    let state = use_context::<AppState>();
    let query = search.read().clone();

    rsx! {
        div { class: "sidebar dash-device-browser",
            div { class: "details-header", span { "Devices / Points" } }
            // Search bar
            div { class: "sidebar-search",
                input {
                    class: "sidebar-search-input",
                    r#type: "text",
                    placeholder: "Search points...",
                    value: "{query}",
                    oninput: move |evt| search.set(evt.value()),
                }
                if !query.is_empty() {
                    button {
                        class: "sidebar-search-clear",
                        onclick: move |_| search.set(String::new()),
                        "x"
                    }
                }
            }
            // Bulk action buttons
            div { class: "alarm-browser-actions",
                button {
                    class: "alarm-browser-action-btn",
                    onclick: {
                        let devices = state.loaded.devices.clone();
                        let q = query.clone();
                        move |_| {
                            selected_points.set(collect_visible_points(&devices, &q, None));
                        }
                    },
                    "All"
                }
                button {
                    class: "alarm-browser-action-btn",
                    onclick: {
                        let devices = state.loaded.devices.clone();
                        let q = query.clone();
                        move |_| {
                            selected_points.set(collect_visible_points(&devices, &q, Some(PointKind::Analog)));
                        }
                    },
                    "Analog"
                }
                button {
                    class: "alarm-browser-action-btn",
                    onclick: {
                        let devices = state.loaded.devices.clone();
                        let q = query.clone();
                        move |_| {
                            selected_points.set(collect_visible_points(&devices, &q, Some(PointKind::Binary)));
                        }
                    },
                    "Binary"
                }
                button {
                    class: "alarm-browser-action-btn",
                    onclick: {
                        let devices = state.loaded.devices.clone();
                        let q = query.clone();
                        move |_| {
                            selected_points.set(collect_visible_points(&devices, &q, Some(PointKind::Multistate)));
                        }
                    },
                    "Multi"
                }
                button {
                    class: "alarm-browser-action-btn",
                    onclick: move |_| {
                        selected_points.set(HashSet::new());
                    },
                    "Clear"
                }
            }
            div { class: "sidebar-content",
                {
                    let filtered: Vec<_> = state.loaded.devices.iter()
                        .filter(|d| device_matches_alarm(d, &query))
                        .collect();
                    if filtered.is_empty() && !query.is_empty() {
                        rsx! { div { class: "tree-empty-search", "No matches" } }
                    } else {
                        rsx! {
                            for dev in filtered {
                                AlarmDeviceNode {
                                    device_id: dev.instance_id.clone(),
                                    filter: query.clone(),
                                    selected_points,
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

fn device_matches_alarm(dev: &crate::config::loader::LoadedDevice, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if dev.instance_id.to_lowercase().contains(&q) {
        return true;
    }
    if dev.profile.profile.name.to_lowercase().contains(&q) {
        return true;
    }
    dev.profile.points.iter().any(|p| {
        p.id.to_lowercase().contains(&q)
            || p.name.to_lowercase().contains(&q)
    })
}

/// Collect all points that match the current search filter and optional kind filter.
fn collect_visible_points(
    devices: &[crate::config::loader::LoadedDevice],
    query: &str,
    kind_filter: Option<PointKind>,
) -> HashSet<(String, String)> {
    let mut set = HashSet::new();
    let has_filter = !query.is_empty();
    let q = query.to_lowercase();
    for dev in devices {
        if !device_matches_alarm(dev, query) {
            continue;
        }
        for pt in &dev.profile.points {
            if let Some(ref kind) = kind_filter {
                if pt.kind != *kind {
                    continue;
                }
            }
            if has_filter
                && !pt.id.to_lowercase().contains(&q)
                && !pt.name.to_lowercase().contains(&q)
                && !dev.instance_id.to_lowercase().contains(&q)
                && !dev.profile.profile.name.to_lowercase().contains(&q)
            {
                continue;
            }
            set.insert((dev.instance_id.clone(), pt.id.clone()));
        }
    }
    set
}

#[component]
fn AlarmDeviceNode(
    device_id: String,
    filter: String,
    selected_points: Signal<HashSet<(String, String)>>,
) -> Element {
    let has_filter = !filter.is_empty();
    let state = use_context::<AppState>();
    let mut expanded = use_signal(|| false);
    let is_open = *expanded.read() || has_filter;

    let device = state.loaded.devices.iter().find(|d| d.instance_id == device_id);
    let Some(dev) = device else { return rsx! {} };

    let profile_name = dev.profile.profile.name.clone();
    let q = filter.to_lowercase();
    let visible_points: Vec<_> = dev
        .profile
        .points
        .iter()
        .filter(|p| {
            !has_filter
                || p.id.to_lowercase().contains(&q)
                || p.name.to_lowercase().contains(&q)
        })
        .map(|p| (p.id.clone(), p.name.clone(), p.kind.clone()))
        .collect();

    rsx! {
        div { class: "dash-device-node",
            div {
                class: "tree-node-row",
                onclick: move |_| expanded.set(!is_open),
                span { class: if is_open { "tree-arrow open" } else { "tree-arrow" }, ">" }
                span { class: "tree-label", "{device_id}" }
                span { class: "tree-badge", "{profile_name}" }
            }
            if is_open {
                div { class: "dash-point-list",
                    for (pt_id, pt_name, pt_kind) in &visible_points {
                        AlarmPointItem {
                            device_id: device_id.clone(),
                            point_id: pt_id.clone(),
                            point_name: pt_name.clone(),
                            point_kind: pt_kind.clone(),
                            selected_points,
                        }
                    }
                    if visible_points.is_empty() {
                        div { class: "dash-point-item muted", "No points" }
                    }
                }
            }
        }
    }
}

#[component]
fn AlarmPointItem(
    device_id: String,
    point_id: String,
    point_name: String,
    point_kind: PointKind,
    selected_points: Signal<HashSet<(String, String)>>,
) -> Element {
    let key = (device_id.clone(), point_id.clone());
    let is_checked = selected_points.read().contains(&key);
    let kind_label = match point_kind {
        PointKind::Analog => "Ana",
        PointKind::Binary => "Bin",
        PointKind::Multistate => "Ms",
    };
    let kind_class = match point_kind {
        PointKind::Analog => "alarm-point-kind ana",
        PointKind::Binary => "alarm-point-kind bin",
        PointKind::Multistate => "alarm-point-kind ms",
    };

    rsx! {
        label { class: "alarm-browser-point",
            input {
                r#type: "checkbox",
                checked: is_checked,
                onchange: {
                    let key = key.clone();
                    move |_| {
                        let mut set = selected_points.read().clone();
                        if set.contains(&key) {
                            set.remove(&key);
                        } else {
                            set.insert(key.clone());
                        }
                        selected_points.set(set);
                    }
                },
            }
            span { class: "dash-point-name", "{point_name}" }
            span { class: "{kind_class}", "{kind_label}" }
        }
    }
}

// ----------------------------------------------------------------
// Right pane: Alarm Template Panel
// ----------------------------------------------------------------

#[component]
fn AlarmTemplatePanel(
    selected_points: Signal<HashSet<(String, String)>>,
    bulk_status: Signal<Option<String>>,
) -> Element {
    let state = use_context::<AppState>();
    let count = selected_points.read().len();

    let mut alarm_type = use_signal(|| "high_limit".to_string());
    let mut severity = use_signal(|| "warning".to_string());
    let limit = use_signal(|| "80.0".to_string());
    let deadband = use_signal(|| "2.0".to_string());
    let delay = use_signal(|| "0".to_string());
    let fault_value = use_signal(|| "1.0".to_string());
    let timeout = use_signal(|| "300".to_string());
    let alarm_value = use_signal(|| "true".to_string());
    let alarm_states = use_signal(|| "1".to_string());
    let fb_device = use_signal(String::new);
    let fb_point = use_signal(String::new);

    let current_type = alarm_type.read().clone();

    // Device/point lists for command mismatch feedback picker
    let device_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .map(|d| d.instance_id.clone())
        .collect();
    let selected_fb_dev = fb_device.read().clone();
    let fb_point_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == selected_fb_dev)
        .map(|d| d.profile.points.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();

    rsx! {
        div { class: "details-pane",
            div { class: "details-header", span { "Alarm Template" } }
            div { class: "alarm-template-body",
                if count == 0 {
                    div { class: "alarm-template-empty",
                        p { "Select points from the device browser to configure alarms in bulk." }
                    }
                } else {
                    div { class: "alarm-template-count", "{count} point(s) selected" }

                    div { class: "alarm-form-row",
                        label { "Type" }
                        select {
                            onchange: move |evt| alarm_type.set(evt.value()),
                            option { value: "high_limit", "High Limit" }
                            option { value: "low_limit", "Low Limit" }
                            option { value: "state_change", "State Change" }
                            option { value: "multi_state_alarm", "Multi-State" }
                            option { value: "command_mismatch", "Cmd Mismatch" }
                            option { value: "state_fault", "State Fault" }
                            option { value: "stale", "Stale" }
                        }
                    }
                    div { class: "alarm-form-row",
                        label { "Severity" }
                        select {
                            onchange: move |evt| severity.set(evt.value()),
                            option { value: "warning", "Warning" }
                            option { value: "critical", "Critical" }
                            option { value: "info", "Info" }
                            option { value: "life_safety", "Life Safety" }
                        }
                    }

                    // Type-specific params
                    {alarm_type_fields(
                        &current_type, limit, deadband, delay, fault_value, timeout,
                        alarm_value, alarm_states, fb_device, fb_point,
                        &device_ids, &fb_point_ids,
                    )}

                    if let Some(ref msg) = *bulk_status.read() {
                        div { class: "alarm-bulk-status", "{msg}" }
                    }

                    button {
                        class: "alarm-apply-btn",
                        onclick: move |_| {
                            let entries: Vec<(String, String)> = selected_points.read().iter().cloned().collect();
                            if entries.is_empty() {
                                return;
                            }
                            let typ = alarm_type.read().clone();
                            let sev_str = severity.read().clone();
                            let sev = AlarmSeverity::from_str(&sev_str).unwrap_or(AlarmSeverity::Warning);

                            let params = match build_alarm_params(
                                &typ, &limit, &deadband, &delay, &fault_value, &timeout,
                                &alarm_value, &alarm_states, &fb_device, &fb_point,
                            ) {
                                Ok(p) => p,
                                Err(e) => { bulk_status.set(Some(e)); return; }
                            };

                            let store = state.alarm_store.clone();
                            spawn(async move {
                                match store.create_configs_batch(&entries, sev, params).await {
                                    Ok(ids) => {
                                        let msg = if ids.is_empty() {
                                            "All alarms already exist (skipped duplicates)".to_string()
                                        } else {
                                            format!("Created {} alarm(s)", ids.len())
                                        };
                                        bulk_status.set(Some(msg));
                                        selected_points.set(HashSet::new());
                                    }
                                    Err(e) => {
                                        bulk_status.set(Some(format!("Error: {e}")));
                                    }
                                }
                            });
                        },
                        "Apply to {count} point(s)"
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Active Alarms Tab
// ----------------------------------------------------------------

#[component]
fn ActiveAlarmsTab() -> Element {
    let state = use_context::<AppState>();
    let mut alarms = use_signal(Vec::<ActiveAlarm>::new);
    let mut loading = use_signal(|| true);

    // Load active alarms
    let alarm_store = state.alarm_store.clone();
    use_effect(move || {
        let store = alarm_store.clone();
        spawn(async move {
            let result = store.get_active_alarms().await;
            alarms.set(result);
            loading.set(false);
        });
    });

    // Refresh on store version changes
    let _version = state.store_version.read();
    let alarm_store_refresh = state.alarm_store.clone();
    use_future(move || {
        let store = alarm_store_refresh.clone();
        async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                let result = store.get_active_alarms().await;
                alarms.set(result);
            }
        }
    });

    let alarm_list = alarms.read();
    let is_loading = *loading.read();

    rsx! {
        div { class: "alarm-active-tab",
            if is_loading {
                div { class: "alarm-loading", "Loading alarms..." }
            } else if alarm_list.is_empty() {
                div { class: "alarm-empty",
                    h3 { "No Active Alarms" }
                    p { "All systems normal." }
                }
            } else {
                div { class: "alarm-active-header",
                    span { class: "alarm-count", "{alarm_list.len()} active alarm(s)" }
                    AckAllButton {}
                }
                table { class: "alarm-table",
                    thead {
                        tr {
                            th { class: "col-severity", "" }
                            th { "Device" }
                            th { "Point" }
                            th { "Type" }
                            th { "Value" }
                            th { "Time" }
                            th { "State" }
                            th { "" }
                        }
                    }
                    tbody {
                        for alarm in alarm_list.iter() {
                            ActiveAlarmRow { alarm: alarm.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AckAllButton() -> Element {
    let state = use_context::<AppState>();
    let mut ack_result = use_signal(|| Option::<String>::None);

    rsx! {
        button {
            class: "alarm-ack-all-btn",
            onclick: move |_| {
                let store = state.alarm_store.clone();
                spawn(async move {
                    match store.acknowledge_all().await {
                        Ok(count) => ack_result.set(Some(format!("Acknowledged {count} alarm(s)"))),
                        Err(e) => ack_result.set(Some(format!("Error: {e}"))),
                    }
                });
            },
            "Ack All"
        }
        if let Some(ref msg) = *ack_result.read() {
            span { class: "alarm-ack-msg", "{msg}" }
        }
    }
}

#[component]
fn ActiveAlarmRow(alarm: ActiveAlarm) -> Element {
    let state = use_context::<AppState>();
    let sev_class = severity_class(alarm.severity);
    let time_str = format_time_ms(alarm.trigger_time_ms);
    let state_str = alarm.state.as_str();
    let is_offnormal = alarm.state == AlarmState::Offnormal;
    let config_id = alarm.config_id;

    rsx! {
        tr { class: "alarm-row {sev_class}",
            td { class: "col-severity",
                span { class: "severity-dot {sev_class}" }
            }
            td { "{alarm.device_id}" }
            td { "{alarm.point_id}" }
            td { "{alarm.alarm_type.label()}" }
            td { class: "col-value", "{alarm.trigger_value:.1}" }
            td { "{time_str}" }
            td { "{state_str}" }
            td {
                if is_offnormal {
                    button {
                        class: "alarm-ack-btn",
                        onclick: move |_| {
                            let store = state.alarm_store.clone();
                            spawn(async move {
                                let _ = store.acknowledge(config_id).await;
                            });
                        },
                        "Ack"
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// History Tab
// ----------------------------------------------------------------

#[component]
fn AlarmHistoryTab() -> Element {
    let state = use_context::<AppState>();
    let mut events = use_signal(Vec::<AlarmEvent>::new);
    let mut loading = use_signal(|| true);
    let mut filter_severity = use_signal(|| Option::<AlarmSeverity>::None);

    let alarm_store = state.alarm_store.clone();
    let sev_filter = filter_severity.read().clone();

    use_effect(move || {
        let store = alarm_store.clone();
        let sev = sev_filter;
        spawn(async move {
            let result = store
                .query_history(AlarmHistoryQuery {
                    severity: sev,
                    limit: Some(200),
                    ..Default::default()
                })
                .await
                .unwrap_or_default();
            events.set(result);
            loading.set(false);
        });
    });

    let event_list = events.read();
    let is_loading = *loading.read();

    rsx! {
        div { class: "alarm-history-tab",
            div { class: "alarm-history-filters",
                label { "Severity: " }
                select {
                    onchange: move |evt| {
                        let val = evt.value();
                        filter_severity.set(AlarmSeverity::from_str(&val));
                    },
                    option { value: "", "All" }
                    option { value: "info", "Info" }
                    option { value: "warning", "Warning" }
                    option { value: "critical", "Critical" }
                    option { value: "life_safety", "Life Safety" }
                }
            }
            if is_loading {
                div { class: "alarm-loading", "Loading history..." }
            } else if event_list.is_empty() {
                div { class: "alarm-empty",
                    p { "No alarm history." }
                }
            } else {
                table { class: "alarm-table",
                    thead {
                        tr {
                            th { "Time" }
                            th { "Device" }
                            th { "Point" }
                            th { class: "col-severity", "" }
                            th { "Transition" }
                            th { "Value" }
                        }
                    }
                    tbody {
                        for event in event_list.iter() {
                            AlarmEventRow { event: event.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AlarmEventRow(event: AlarmEvent) -> Element {
    let sev_class = severity_class(event.severity);
    let time_str = format_time_ms(event.timestamp_ms);
    let transition = format!("{} -> {}", event.from_state, event.to_state);

    rsx! {
        tr { class: "alarm-row",
            td { "{time_str}" }
            td { "{event.device_id}" }
            td { "{event.point_id}" }
            td { class: "col-severity",
                span { class: "severity-dot {sev_class}" }
            }
            td { "{transition}" }
            td { class: "col-value", "{event.value:.1}" }
        }
    }
}

// ----------------------------------------------------------------
// Config Tab
// ----------------------------------------------------------------

#[component]
fn AlarmConfigTab() -> Element {
    let state = use_context::<AppState>();
    let mut configs = use_signal(Vec::<AlarmConfig>::new);
    let mut loading = use_signal(|| true);
    let mut show_add_form = use_signal(|| false);

    let alarm_store = state.alarm_store.clone();
    use_effect(move || {
        let store = alarm_store.clone();
        spawn(async move {
            let result = store.list_configs().await;
            configs.set(result);
            loading.set(false);
        });
    });

    // Refresh when config changes
    let alarm_store_refresh = state.alarm_store.clone();
    let mut config_watch = use_signal(|| 0u64);
    use_future(move || {
        let store = alarm_store_refresh.clone();
        async move {
            let mut rx = store.subscribe_config_changes();
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let result = store.list_configs().await;
                configs.set(result);
                config_watch.set(*rx.borrow());
            }
        }
    });

    let config_list = configs.read();
    let is_loading = *loading.read();
    let adding = *show_add_form.read();

    rsx! {
        div { class: "alarm-config-tab",
            div { class: "alarm-config-header",
                button {
                    class: "alarm-add-btn",
                    onclick: move |_| show_add_form.set(!adding),
                    if adding { "Cancel" } else { "+ Add Alarm" }
                }
            }

            if adding {
                AddAlarmForm {
                    on_done: move || show_add_form.set(false),
                }
            }

            if is_loading {
                div { class: "alarm-loading", "Loading configs..." }
            } else if config_list.is_empty() {
                div { class: "alarm-empty",
                    p { "No alarms configured." }
                    p { class: "alarm-empty-hint", "Click \"+ Add Alarm\" or use the bulk template panel." }
                }
            } else {
                table { class: "alarm-table",
                    thead {
                        tr {
                            th { "Device" }
                            th { "Point" }
                            th { "Type" }
                            th { "Params" }
                            th { class: "col-severity", "Severity" }
                            th { "Enabled" }
                            th { "" }
                        }
                    }
                    tbody {
                        for cfg in config_list.iter() {
                            AlarmConfigRow { config: cfg.clone() }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AlarmConfigRow(config: AlarmConfig) -> Element {
    let state = use_context::<AppState>();
    let mut confirming = use_signal(|| false);
    let sev_class = severity_class(config.severity);
    let params_str = format_params(&config.params);
    let config_id = config.id;
    let enabled = config.enabled;
    let is_confirming = *confirming.read();

    rsx! {
        tr { class: "alarm-row",
            td { "{config.device_id}" }
            td { "{config.point_id}" }
            td { "{config.alarm_type.label()}" }
            td { class: "alarm-params-cell", "{params_str}" }
            td { class: "col-severity",
                span { class: "severity-dot {sev_class}" }
                " {config.severity.label()}"
            }
            td {
                span {
                    class: if enabled { "alarm-enabled-badge on" } else { "alarm-enabled-badge off" },
                    if enabled { "On" } else { "Off" }
                }
            }
            td {
                if is_confirming {
                    button {
                        class: "alarm-delete-btn confirm",
                        title: "Confirm delete",
                        onclick: move |_| {
                            let store = state.alarm_store.clone();
                            spawn(async move {
                                let _ = store.delete_config(config_id).await;
                            });
                        },
                        "Delete"
                    }
                    button {
                        class: "alarm-cancel-btn",
                        onclick: move |_| confirming.set(false),
                        "Cancel"
                    }
                } else {
                    button {
                        class: "alarm-delete-btn",
                        title: "Delete alarm",
                        onclick: move |_| confirming.set(true),
                        "x"
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Add Alarm Form (single-point, kept as fallback in Config tab)
// ----------------------------------------------------------------

#[component]
fn AddAlarmForm(on_done: EventHandler<()>) -> Element {
    let state = use_context::<AppState>();

    let mut device_id = use_signal(String::new);
    let mut point_id = use_signal(String::new);
    let mut alarm_type = use_signal(|| "high_limit".to_string());
    let mut severity = use_signal(|| "warning".to_string());
    let limit = use_signal(|| "80.0".to_string());
    let deadband = use_signal(|| "2.0".to_string());
    let delay = use_signal(|| "0".to_string());
    let fault_value = use_signal(|| "1.0".to_string());
    let timeout = use_signal(|| "300".to_string());
    let alarm_value = use_signal(|| "true".to_string());
    let alarm_states = use_signal(|| "1".to_string());
    let fb_device = use_signal(String::new);
    let fb_point = use_signal(String::new);
    let mut error_msg = use_signal(|| Option::<String>::None);

    let device_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .map(|d| d.instance_id.clone())
        .collect();

    let selected_dev = device_id.read().clone();
    let point_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == selected_dev)
        .map(|d| d.profile.points.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();

    // Feedback device/point lists for command mismatch
    let selected_fb_dev = fb_device.read().clone();
    let fb_point_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == selected_fb_dev)
        .map(|d| d.profile.points.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();

    let current_type = alarm_type.read().clone();

    rsx! {
        div { class: "alarm-add-form",
            div { class: "alarm-form-row",
                label { "Device" }
                select {
                    onchange: move |evt| device_id.set(evt.value()),
                    option { value: "", "Select device..." }
                    for dev in &device_ids {
                        option { value: "{dev}", "{dev}" }
                    }
                }
            }
            div { class: "alarm-form-row",
                label { "Point" }
                select {
                    onchange: move |evt| point_id.set(evt.value()),
                    option { value: "", "Select point..." }
                    for pt in &point_ids {
                        option { value: "{pt}", "{pt}" }
                    }
                }
            }
            div { class: "alarm-form-row",
                label { "Type" }
                select {
                    onchange: move |evt| alarm_type.set(evt.value()),
                    option { value: "high_limit", "High Limit" }
                    option { value: "low_limit", "Low Limit" }
                    option { value: "state_change", "State Change" }
                    option { value: "multi_state_alarm", "Multi-State" }
                    option { value: "command_mismatch", "Cmd Mismatch" }
                    option { value: "state_fault", "State Fault" }
                    option { value: "stale", "Stale" }
                }
            }
            div { class: "alarm-form-row",
                label { "Severity" }
                select {
                    onchange: move |evt| severity.set(evt.value()),
                    option { value: "info", "Info" }
                    option { value: "warning", selected: true, "Warning" }
                    option { value: "critical", "Critical" }
                    option { value: "life_safety", "Life Safety" }
                }
            }

            {alarm_type_fields(
                &current_type, limit, deadband, delay, fault_value, timeout,
                alarm_value, alarm_states, fb_device, fb_point,
                &device_ids, &fb_point_ids,
            )}

            if let Some(ref err) = *error_msg.read() {
                div { class: "alarm-form-error", "{err}" }
            }

            div { class: "alarm-form-actions",
                button {
                    class: "alarm-save-btn",
                    onclick: move |_| {
                        let dev = device_id.read().clone();
                        let pt = point_id.read().clone();
                        let typ = alarm_type.read().clone();
                        let sev_str = severity.read().clone();

                        if dev.is_empty() || pt.is_empty() {
                            error_msg.set(Some("Select a device and point.".into()));
                            return;
                        }

                        let sev = AlarmSeverity::from_str(&sev_str).unwrap_or(AlarmSeverity::Warning);
                        let params = match build_alarm_params(
                            &typ, &limit, &deadband, &delay, &fault_value, &timeout,
                            &alarm_value, &alarm_states, &fb_device, &fb_point,
                        ) {
                            Ok(p) => p,
                            Err(e) => { error_msg.set(Some(e)); return; }
                        };

                        let store = state.alarm_store.clone();
                        let done = on_done.clone();
                        spawn(async move {
                            match store.create_config(&dev, &pt, sev, params).await {
                                Ok(_) => done.call(()),
                                Err(e) => error_msg.set(Some(format!("Error: {e}"))),
                            }
                        });
                    },
                    "Create"
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Point Detail: Alarm section (exported for use in point_detail.rs)
// ----------------------------------------------------------------

#[component]
pub fn PointAlarmSection(device_id: String, point_id: String) -> Element {
    let state = use_context::<AppState>();
    let mut configs = use_signal(Vec::<AlarmConfig>::new);
    let mut show_add = use_signal(|| false);

    let alarm_store = state.alarm_store.clone();
    let dev = device_id.clone();
    let pt = point_id.clone();
    use_effect(move || {
        let store = alarm_store.clone();
        let d = dev.clone();
        let p = pt.clone();
        spawn(async move {
            let result = store.get_configs_for_point(&d, &p).await;
            configs.set(result);
        });
    });

    // Refresh on config version changes
    let alarm_store_watch = state.alarm_store.clone();
    let dev2 = device_id.clone();
    let pt2 = point_id.clone();
    use_future(move || {
        let store = alarm_store_watch.clone();
        let d = dev2.clone();
        let p = pt2.clone();
        async move {
            let mut rx = store.subscribe_config_changes();
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                let result = store.get_configs_for_point(&d, &p).await;
                configs.set(result);
            }
        }
    });

    let config_list = configs.read();
    let adding = *show_add.read();

    rsx! {
        div { class: "point-alarm-section",
            h4 { class: "point-alarm-title", "Alarms" }

            if config_list.is_empty() && !adding {
                p { class: "point-alarm-empty", "No alarms configured." }
            }

            for cfg in config_list.iter() {
                PointAlarmItem { config: cfg.clone() }
            }

            if adding {
                PointAddAlarmForm {
                    device_id: device_id.clone(),
                    point_id: point_id.clone(),
                    on_done: move || show_add.set(false),
                }
            } else {
                button {
                    class: "point-alarm-add-btn",
                    onclick: move |_| show_add.set(true),
                    "+ Add Alarm"
                }
            }
        }
    }
}

#[component]
fn PointAlarmItem(config: AlarmConfig) -> Element {
    let state = use_context::<AppState>();
    let mut confirming = use_signal(|| false);
    let is_confirming = *confirming.read();
    let config_id = config.id;

    rsx! {
        div { class: "point-alarm-item",
            span { class: "severity-dot {severity_class(config.severity)}" }
            span { "{config.alarm_type.label()}" }
            span { class: "point-alarm-params", "{format_params(&config.params)}" }
            if is_confirming {
                button {
                    class: "alarm-delete-btn confirm",
                    title: "Confirm delete",
                    onclick: move |_| {
                        let store = state.alarm_store.clone();
                        spawn(async move {
                            let _ = store.delete_config(config_id).await;
                        });
                    },
                    "Delete"
                }
                button {
                    class: "alarm-cancel-btn",
                    onclick: move |_| confirming.set(false),
                    "Cancel"
                }
            } else {
                button {
                    class: "alarm-delete-btn",
                    title: "Delete alarm",
                    onclick: move |_| confirming.set(true),
                    "x"
                }
            }
        }
    }
}

/// Simplified add form for point detail panel.
#[component]
fn PointAddAlarmForm(device_id: String, point_id: String, on_done: EventHandler<()>) -> Element {
    let state = use_context::<AppState>();
    let mut error_msg = use_signal(|| Option::<String>::None);

    let mut alarm_type = use_signal(|| "high_limit".to_string());
    let mut severity = use_signal(|| "warning".to_string());
    let limit = use_signal(|| "80.0".to_string());
    let deadband = use_signal(|| "2.0".to_string());
    let delay = use_signal(|| "0".to_string());
    let fault_value = use_signal(|| "1.0".to_string());
    let timeout = use_signal(|| "300".to_string());
    let alarm_value = use_signal(|| "true".to_string());
    let alarm_states = use_signal(|| "1".to_string());
    let fb_device = use_signal(String::new);
    let fb_point = use_signal(String::new);

    let current_type = alarm_type.read().clone();

    let device_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .map(|d| d.instance_id.clone())
        .collect();
    let selected_fb_dev = fb_device.read().clone();
    let fb_point_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == selected_fb_dev)
        .map(|d| d.profile.points.iter().map(|p| p.id.clone()).collect())
        .unwrap_or_default();

    rsx! {
        div { class: "point-alarm-add-form",
            div { class: "alarm-form-row",
                label { "Type" }
                select {
                    onchange: move |evt| alarm_type.set(evt.value()),
                    option { value: "high_limit", "High Limit" }
                    option { value: "low_limit", "Low Limit" }
                    option { value: "state_change", "State Change" }
                    option { value: "multi_state_alarm", "Multi-State" }
                    option { value: "command_mismatch", "Cmd Mismatch" }
                    option { value: "state_fault", "State Fault" }
                    option { value: "stale", "Stale" }
                }
            }
            div { class: "alarm-form-row",
                label { "Severity" }
                select {
                    onchange: move |evt| severity.set(evt.value()),
                    option { value: "warning", "Warning" }
                    option { value: "critical", "Critical" }
                    option { value: "info", "Info" }
                    option { value: "life_safety", "Life Safety" }
                }
            }

            {alarm_type_fields(
                &current_type, limit, deadband, delay, fault_value, timeout,
                alarm_value, alarm_states, fb_device, fb_point,
                &device_ids, &fb_point_ids,
            )}

            if let Some(ref err) = *error_msg.read() {
                div { class: "alarm-form-error", "{err}" }
            }

            div { class: "alarm-form-actions",
                button {
                    class: "alarm-save-btn",
                    onclick: {
                        let device_id = device_id.clone();
                        let point_id = point_id.clone();
                        move |_| {
                            let typ = alarm_type.read().clone();
                            let sev_str = severity.read().clone();
                            let sev = AlarmSeverity::from_str(&sev_str).unwrap_or(AlarmSeverity::Warning);

                            let params = match build_alarm_params(
                                &typ, &limit, &deadband, &delay, &fault_value, &timeout,
                                &alarm_value, &alarm_states, &fb_device, &fb_point,
                            ) {
                                Ok(p) => p,
                                Err(e) => { error_msg.set(Some(e)); return; }
                            };

                            let store = state.alarm_store.clone();
                            let dev = device_id.clone();
                            let pt = point_id.clone();
                            let done = on_done.clone();
                            spawn(async move {
                                match store.create_config(&dev, &pt, sev, params).await {
                                    Ok(_) => done.call(()),
                                    Err(e) => error_msg.set(Some(format!("Error: {e}"))),
                                }
                            });
                        }
                    },
                    "Add"
                }
                button {
                    class: "alarm-cancel-btn",
                    onclick: move |_| on_done.call(()),
                    "Cancel"
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Shared form helpers
// ----------------------------------------------------------------

/// Renders type-specific alarm parameter fields.
#[allow(clippy::too_many_arguments)]
fn alarm_type_fields(
    current_type: &str,
    mut limit: Signal<String>,
    mut deadband: Signal<String>,
    mut delay: Signal<String>,
    mut fault_value: Signal<String>,
    mut timeout: Signal<String>,
    mut alarm_value: Signal<String>,
    mut alarm_states: Signal<String>,
    mut fb_device: Signal<String>,
    mut fb_point: Signal<String>,
    device_ids: &[String],
    fb_point_ids: &[String],
) -> Element {
    match current_type {
        "high_limit" | "low_limit" => rsx! {
            div { class: "alarm-form-row",
                label { "Limit" }
                input {
                    r#type: "number",
                    step: "0.1",
                    value: "{limit}",
                    onchange: move |evt| limit.set(evt.value()),
                }
            }
            div { class: "alarm-form-row",
                label { "Deadband" }
                input {
                    r#type: "number",
                    step: "0.1",
                    value: "{deadband}",
                    onchange: move |evt| deadband.set(evt.value()),
                }
            }
            div { class: "alarm-form-row",
                label { "Delay (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{delay}",
                    onchange: move |evt| delay.set(evt.value()),
                }
            }
        },
        "state_fault" => rsx! {
            div { class: "alarm-form-row",
                label { "Fault Value" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{fault_value}",
                    onchange: move |evt| fault_value.set(evt.value()),
                }
            }
            div { class: "alarm-form-row",
                label { "Delay (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{delay}",
                    onchange: move |evt| delay.set(evt.value()),
                }
            }
        },
        "stale" => rsx! {
            div { class: "alarm-form-row",
                label { "Timeout (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{timeout}",
                    onchange: move |evt| timeout.set(evt.value()),
                }
            }
        },
        "state_change" => rsx! {
            div { class: "alarm-form-row",
                label { "Alarm When" }
                select {
                    onchange: move |evt| alarm_value.set(evt.value()),
                    option { value: "true", "ON (true)" }
                    option { value: "false", "OFF (false)" }
                }
            }
            div { class: "alarm-form-row",
                label { "Delay (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{delay}",
                    onchange: move |evt| delay.set(evt.value()),
                }
            }
        },
        "multi_state_alarm" => rsx! {
            div { class: "alarm-form-row",
                label { "States" }
                input {
                    r#type: "text",
                    placeholder: "1, 3, 4",
                    value: "{alarm_states}",
                    onchange: move |evt| alarm_states.set(evt.value()),
                }
            }
            div { class: "alarm-form-hint", "Comma-separated state numbers that trigger the alarm." }
            div { class: "alarm-form-row",
                label { "Delay (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{delay}",
                    onchange: move |evt| delay.set(evt.value()),
                }
            }
        },
        "command_mismatch" => rsx! {
            div { class: "alarm-form-row",
                label { "Fb Device" }
                select {
                    onchange: move |evt| fb_device.set(evt.value()),
                    option { value: "", "Select device..." }
                    for dev in device_ids {
                        option { value: "{dev}", "{dev}" }
                    }
                }
            }
            div { class: "alarm-form-row",
                label { "Fb Point" }
                select {
                    onchange: move |evt| fb_point.set(evt.value()),
                    option { value: "", "Select point..." }
                    for pt in fb_point_ids {
                        option { value: "{pt}", "{pt}" }
                    }
                }
            }
            div { class: "alarm-form-row",
                label { "Delay (s)" }
                input {
                    r#type: "number",
                    step: "1",
                    value: "{delay}",
                    onchange: move |evt| delay.set(evt.value()),
                }
            }
            div { class: "alarm-form-hint", "Alarms when command and feedback differ for the delay period." }
        },
        _ => rsx! {},
    }
}

/// Build AlarmParams from form signal values.
#[allow(clippy::too_many_arguments)]
fn build_alarm_params(
    typ: &str,
    limit: &Signal<String>,
    deadband: &Signal<String>,
    delay: &Signal<String>,
    fault_value: &Signal<String>,
    timeout: &Signal<String>,
    alarm_value: &Signal<String>,
    alarm_states: &Signal<String>,
    fb_device: &Signal<String>,
    fb_point: &Signal<String>,
) -> Result<AlarmParams, String> {
    let parse_f64 = |s: &Signal<String>, name: &str| -> Result<f64, String> {
        s.read().parse::<f64>().map_err(|_| format!("Invalid {name}: must be a number"))
    };
    let parse_u64 = |s: &Signal<String>, name: &str| -> Result<u64, String> {
        s.read().parse::<u64>().map_err(|_| format!("Invalid {name}: must be a positive integer"))
    };

    match typ {
        "high_limit" => {
            let l = parse_f64(limit, "limit")?;
            let d = parse_f64(deadband, "deadband")?;
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::HighLimit { limit: l, deadband: d, delay_secs: dl })
        }
        "low_limit" => {
            let l = parse_f64(limit, "limit")?;
            let d = parse_f64(deadband, "deadband")?;
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::LowLimit { limit: l, deadband: d, delay_secs: dl })
        }
        "state_fault" => {
            let fv = parse_f64(fault_value, "fault value")?;
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::StateFault { fault_value: fv, delay_secs: dl })
        }
        "stale" => {
            let t = parse_u64(timeout, "timeout")?;
            Ok(AlarmParams::Stale { timeout_secs: t })
        }
        "state_change" => {
            let av = alarm_value.read().as_str() == "true";
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::StateChange { alarm_value: av, delay_secs: dl })
        }
        "multi_state_alarm" => {
            let states: Vec<i64> = alarm_states
                .read()
                .split(',')
                .filter_map(|s| s.trim().parse::<i64>().ok())
                .collect();
            if states.is_empty() {
                return Err("Enter at least one alarm state number.".into());
            }
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::MultiStateAlarm { alarm_states: states, delay_secs: dl })
        }
        "command_mismatch" => {
            let fd = fb_device.read().clone();
            let fp = fb_point.read().clone();
            if fd.is_empty() || fp.is_empty() {
                return Err("Select feedback device and point.".into());
            }
            let dl = parse_u64(delay, "delay")?;
            Ok(AlarmParams::CommandMismatch {
                feedback_device_id: fd,
                feedback_point_id: fp,
                delay_secs: dl,
            })
        }
        _ => Err("Unknown alarm type.".into()),
    }
}

// ----------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------

fn severity_class(severity: AlarmSeverity) -> &'static str {
    match severity {
        AlarmSeverity::Info => "sev-info",
        AlarmSeverity::Warning => "sev-warning",
        AlarmSeverity::Critical => "sev-critical",
        AlarmSeverity::LifeSafety => "sev-life-safety",
    }
}

fn format_params(params: &AlarmParams) -> String {
    match params {
        AlarmParams::HighLimit {
            limit,
            deadband,
            delay_secs,
        } => {
            let mut s = format!("limit: {limit}");
            if *deadband > 0.0 {
                s.push_str(&format!(", db: {deadband}"));
            }
            if *delay_secs > 0 {
                s.push_str(&format!(", delay: {delay_secs}s"));
            }
            s
        }
        AlarmParams::LowLimit {
            limit,
            deadband,
            delay_secs,
        } => {
            let mut s = format!("limit: {limit}");
            if *deadband > 0.0 {
                s.push_str(&format!(", db: {deadband}"));
            }
            if *delay_secs > 0 {
                s.push_str(&format!(", delay: {delay_secs}s"));
            }
            s
        }
        AlarmParams::StateFault {
            fault_value,
            delay_secs,
        } => {
            let mut s = format!("fault: {fault_value}");
            if *delay_secs > 0 {
                s.push_str(&format!(", delay: {delay_secs}s"));
            }
            s
        }
        AlarmParams::Stale { timeout_secs } => format!("timeout: {timeout_secs}s"),
        AlarmParams::Deviation {
            threshold,
            deadband,
            ..
        } => {
            let mut s = format!("threshold: {threshold}");
            if *deadband > 0.0 {
                s.push_str(&format!(", db: {deadband}"));
            }
            s
        }
        AlarmParams::StateChange { alarm_value, delay_secs } => {
            let state_label = if *alarm_value { "ON" } else { "OFF" };
            let mut s = format!("alarm when: {state_label}");
            if *delay_secs > 0 {
                s.push_str(&format!(", delay: {delay_secs}s"));
            }
            s
        }
        AlarmParams::MultiStateAlarm { alarm_states, delay_secs } => {
            let states_str: Vec<String> = alarm_states.iter().map(|s| s.to_string()).collect();
            let mut s = format!("states: [{}]", states_str.join(", "));
            if *delay_secs > 0 {
                s.push_str(&format!(", delay: {delay_secs}s"));
            }
            s
        }
        AlarmParams::CommandMismatch { feedback_device_id, feedback_point_id, delay_secs } => {
            format!("fb: {feedback_device_id}/{feedback_point_id}, delay: {delay_secs}s")
        }
    }
}

fn format_time_ms(ms: i64) -> String {
    #[repr(C)]
    #[derive(Default)]
    struct Tm {
        tm_sec: i32, tm_min: i32, tm_hour: i32, tm_mday: i32,
        tm_mon: i32, tm_year: i32, tm_wday: i32, tm_yday: i32,
        tm_isdst: i32, tm_gmtoff: i64, tm_zone: *const i8,
    }
    extern "C" {
        fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    }

    let epoch_secs = ms / 1000;
    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut tm = Tm::default();
    unsafe { localtime_r(&epoch_secs, &mut tm) };

    let mut now_tm = Tm::default();
    unsafe { localtime_r(&now_secs, &mut now_tm) };

    let hour = tm.tm_hour;
    let min = tm.tm_min;
    let sec = tm.tm_sec;

    if tm.tm_year == now_tm.tm_year && tm.tm_yday == now_tm.tm_yday {
        format!("{hour:02}:{min:02}:{sec:02}")
    } else {
        const MONTHS: [&str; 12] = ["Jan","Feb","Mar","Apr","May","Jun","Jul","Aug","Sep","Oct","Nov","Dec"];
        let mon = MONTHS.get(tm.tm_mon as usize).unwrap_or(&"???");
        let day = tm.tm_mday;
        format!("{mon} {day} {hour:02}:{min:02}:{sec:02}")
    }
}
