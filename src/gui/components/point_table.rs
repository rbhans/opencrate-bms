use dioxus::prelude::*;

use crate::config::profile::PointValue;
use crate::gui::state::AppState;
use crate::store::point_store::PointStatusFlags;

#[derive(Clone, Copy, PartialEq)]
enum SortCol {
    Name,
    Kind,
    Access,
    Value,
    Units,
}

#[derive(Clone, Copy, PartialEq)]
enum SortDir {
    Asc,
    Desc,
}

#[component]
pub fn PointTable() -> Element {
    let mut state = use_context::<AppState>();
    let mut sort_col = use_signal(|| SortCol::Name);
    let mut sort_dir = use_signal(|| SortDir::Asc);
    let mut pinned: Signal<Vec<String>> = use_signal(Vec::new);

    // Re-read when store changes
    let _version = state.store_version.read();

    let selected_device = state.selected_device.read().clone();
    let selected_point = state.selected_point.read().clone();

    let Some(device_id) = selected_device else {
        return rsx! {
            div { class: "point-table empty",
                p { class: "placeholder", "Select a device to view its points." }
            }
        };
    };

    let profile = state
        .loaded
        .devices
        .iter()
        .find(|d| d.instance_id == device_id)
        .map(|d| &d.profile);

    let live_points = state.store.get_all_for_device(&device_id);

    struct RowData {
        point_id: String,
        name: String,
        kind: String,
        access: String,
        units: Option<String>,
        value_str: String,
        status: PointStatusFlags,
    }

    let mut rows: Vec<RowData> = if let Some(profile) = profile {
        profile
            .points
            .iter()
            .map(|pt| {
                let live = live_points
                    .iter()
                    .find(|(k, _)| k.point_id == pt.id);
                let value = live.map(|(_, v)| v.value.clone());
                let status = live.map(|(_, v)| v.status).unwrap_or_default();
                let prec = pt.ui.as_ref().and_then(|u| u.precision).unwrap_or(1) as usize;
                let value_str = match &value {
                    Some(PointValue::Bool(b)) => if *b { "ON".into() } else { "OFF".into() },
                    Some(PointValue::Integer(i)) => i.to_string(),
                    Some(PointValue::Float(f)) => format!("{f:.prec$}"),
                    None => "—".into(),
                };

                RowData {
                    point_id: pt.id.clone(),
                    name: pt.name.clone(),
                    kind: format!("{:?}", pt.kind).to_lowercase(),
                    access: format!("{:?}", pt.access).to_lowercase(),
                    units: pt.units.clone(),
                    value_str,
                    status,
                }
            })
            .collect()
    } else {
        live_points
            .iter()
            .map(|(k, v)| {
                let value_str = match &v.value {
                    PointValue::Bool(b) => if *b { "ON".into() } else { "OFF".into() },
                    PointValue::Integer(i) => i.to_string(),
                    PointValue::Float(f) => format!("{f:.1}"),
                };
                RowData {
                    point_id: k.point_id.clone(),
                    name: k.point_id.clone(),
                    kind: String::new(),
                    access: String::new(),
                    units: None,
                    value_str,
                    status: v.status,
                }
            })
            .collect()
    };

    // Sort
    let col = *sort_col.read();
    let dir = *sort_dir.read();
    let cmp = |a: &RowData, b: &RowData| -> std::cmp::Ordering {
        let ord = match col {
            SortCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            SortCol::Kind => a.kind.cmp(&b.kind),
            SortCol::Access => a.access.cmp(&b.access),
            SortCol::Value => a.value_str.cmp(&b.value_str),
            SortCol::Units => {
                let au = a.units.as_deref().unwrap_or("");
                let bu = b.units.as_deref().unwrap_or("");
                au.cmp(bu)
            }
        };
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    };

    // Partition into pinned and unpinned, sort each group
    let pinned_set = pinned.read().clone();
    let mut pinned_rows: Vec<RowData> = Vec::new();
    let mut unpinned_rows: Vec<RowData> = Vec::new();
    for row in rows.drain(..) {
        if pinned_set.contains(&row.point_id) {
            pinned_rows.push(row);
        } else {
            unpinned_rows.push(row);
        }
    }
    pinned_rows.sort_by(&cmp);
    unpinned_rows.sort_by(&cmp);

    // Click column header to sort
    let mut on_header_click = move |clicked: SortCol| {
        let cur_col = *sort_col.read();
        if cur_col == clicked {
            let cur_dir = *sort_dir.read();
            sort_dir.set(if cur_dir == SortDir::Asc { SortDir::Desc } else { SortDir::Asc });
        } else {
            sort_col.set(clicked);
            sort_dir.set(SortDir::Asc);
        }
    };

    // Sort indicator
    let indicator = |c: SortCol| -> &'static str {
        if *sort_col.read() == c {
            if *sort_dir.read() == SortDir::Asc { " \u{25B2}" } else { " \u{25BC}" }
        } else {
            ""
        }
    };

    let has_pinned = !pinned_rows.is_empty();

    rsx! {
        div { class: "point-table",
            table {
                thead {
                    tr {
                        th { class: "col-pin-header" }
                        th { class: "col-status-header", "Status" }
                        th { class: "sortable", onclick: move |_| on_header_click(SortCol::Name),
                            "Point{indicator(SortCol::Name)}"
                        }
                        th { class: "sortable", onclick: move |_| on_header_click(SortCol::Kind),
                            "Kind{indicator(SortCol::Kind)}"
                        }
                        th { class: "sortable", onclick: move |_| on_header_click(SortCol::Access),
                            "Access{indicator(SortCol::Access)}"
                        }
                        th { class: "sortable", onclick: move |_| on_header_click(SortCol::Value),
                            "Value{indicator(SortCol::Value)}"
                        }
                        th { class: "sortable", onclick: move |_| on_header_click(SortCol::Units),
                            "Units{indicator(SortCol::Units)}"
                        }
                    }
                }
                tbody {
                    // Pinned rows
                    for row in &pinned_rows {
                        {
                            let is_selected = selected_point.as_deref() == Some(row.point_id.as_str());
                            let pid = row.point_id.clone();
                            let pid_unpin = row.point_id.clone();
                            let status_class = row.status.worst_status()
                                .map(|s| format!("status-dot status-{s}"))
                                .unwrap_or_default();
                            let status_title = row.status.active_flags().join(", ");

                            rsx! {
                                tr {
                                    key: "pin-{pid}",
                                    class: if is_selected { "point-row pinned selected" } else { "point-row pinned" },

                                    onclick: move |_| {
                                        state.selected_point.set(Some(pid.clone()));
                                        state.detail_open.set(true);
                                    },
                                    td { class: "col-pin",
                                        button {
                                            class: "pin-btn pinned",
                                            title: "Unpin",
                                            onclick: move |e: Event<MouseData>| {
                                                e.stop_propagation();
                                                pinned.write().retain(|p| p != &pid_unpin);
                                            },
                                            "\u{1F4CC}"
                                        }
                                    }
                                    td { class: "col-status",
                                        if !row.status.is_normal() {
                                            span {
                                                class: "{status_class}",
                                                title: "{status_title}",
                                            }
                                        }
                                    }
                                    td { class: "col-name", "{row.name}" }
                                    td { class: "col-kind", "{row.kind}" }
                                    td { class: "col-access", "{row.access}" }
                                    td { class: "col-value", "{row.value_str}" }
                                    td { class: "col-units", {row.units.as_deref().unwrap_or("")} }
                                }
                            }
                        }
                    }

                    // Divider between pinned and unpinned
                    if has_pinned {
                        tr { class: "pin-divider",
                            td { colspan: "7" }
                        }
                    }

                    // Unpinned rows
                    for row in &unpinned_rows {
                        {
                            let is_selected = selected_point.as_deref() == Some(row.point_id.as_str());
                            let pid = row.point_id.clone();
                            let pid_pin = row.point_id.clone();
                            let status_class = row.status.worst_status()
                                .map(|s| format!("status-dot status-{s}"))
                                .unwrap_or_default();
                            let status_title = row.status.active_flags().join(", ");

                            rsx! {
                                tr {
                                    key: "{pid}",
                                    class: if is_selected { "point-row selected" } else { "point-row" },

                                    onclick: move |_| {
                                        state.selected_point.set(Some(pid.clone()));
                                        state.detail_open.set(true);
                                    },
                                    td { class: "col-pin",
                                        button {
                                            class: "pin-btn",
                                            title: "Pin to top",
                                            onclick: move |e: Event<MouseData>| {
                                                e.stop_propagation();
                                                pinned.write().push(pid_pin.clone());
                                            },
                                            "\u{1F4CC}"
                                        }
                                    }
                                    td { class: "col-status",
                                        if !row.status.is_normal() {
                                            span {
                                                class: "{status_class}",
                                                title: "{status_title}",
                                            }
                                        }
                                    }
                                    td { class: "col-name", "{row.name}" }
                                    td { class: "col-kind", "{row.kind}" }
                                    td { class: "col-access", "{row.access}" }
                                    td { class: "col-value", "{row.value_str}" }
                                    td { class: "col-units", {row.units.as_deref().unwrap_or("")} }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
