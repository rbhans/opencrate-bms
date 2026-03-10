use dioxus::prelude::*;

use crate::config::loader::LoadedDevice;
use crate::gui::state::{ActiveView, AppState};
use crate::store::point_store::PointStatusFlags;

/// Check if a device matches a search query (case-insensitive).
/// Matches against device ID, profile name, description, and all point fields.
fn device_matches(dev: &LoadedDevice, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();

    // Device-level fields
    if dev.instance_id.to_lowercase().contains(&q) {
        return true;
    }
    let profile = &dev.profile.profile;
    if profile.name.to_lowercase().contains(&q) {
        return true;
    }
    if let Some(ref desc) = profile.description {
        if desc.to_lowercase().contains(&q) {
            return true;
        }
    }
    if let Some(ref mfr) = profile.manufacturer {
        if mfr.to_lowercase().contains(&q) {
            return true;
        }
    }

    // Point-level fields
    for pt in &dev.profile.points {
        if pt.id.to_lowercase().contains(&q)
            || pt.name.to_lowercase().contains(&q)
        {
            return true;
        }
        if let Some(ref desc) = pt.description {
            if desc.to_lowercase().contains(&q) {
                return true;
            }
        }
        if let Some(ref units) = pt.units {
            if units.to_lowercase().contains(&q) {
                return true;
            }
        }
        if let Some(ref ui) = pt.ui {
            if let Some(ref group) = ui.group {
                if group.to_lowercase().contains(&q) {
                    return true;
                }
            }
        }
    }

    false
}

#[component]
pub fn DeviceTree(filter: String) -> Element {
    let mut state = use_context::<AppState>();

    let _version = state.store_version.read();

    // Merge configured devices + discovered devices from store
    let mut device_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .filter(|d| device_matches(d, &filter))
        .map(|d| d.instance_id.clone())
        .collect();

    // Discovered devices — filter by ID only (no profile data)
    let q_lower = filter.to_lowercase();
    for id in state.store.device_ids() {
        if !device_ids.contains(&id) {
            if filter.is_empty() || id.to_lowercase().contains(&q_lower) {
                device_ids.push(id);
            }
        }
    }

    let selected = state.selected_device.read().clone();

    rsx! {
        div { class: "device-tree",
            if device_ids.is_empty() && !filter.is_empty() {
                div { class: "tree-empty-search",
                    "No matches for \"{filter}\""
                }
            }
            ul { class: "tree-list",
                for device_id in device_ids {
                    {
                        let is_selected = selected.as_deref() == Some(device_id.as_str());
                        let device_points = state.store.get_all_for_device(&device_id);
                        let point_count = device_points.len();
                        let worst = device_points.iter()
                            .map(|(_, tv)| tv.status)
                            .fold(PointStatusFlags::default(), |acc, s| PointStatusFlags(acc.0 | s.0));
                        let worst_class = worst.worst_status()
                            .map(|s| format!("status-dot status-{s}"))
                            .unwrap_or_default();
                        let did = device_id.clone();
                        rsx! {
                            li {
                                class: if is_selected { "tree-node leaf selected" } else { "tree-node leaf" },
                                onclick: move |_| {
                                    state.selected_device.set(Some(did.clone()));
                                    state.selected_point.set(None);
                                    state.detail_open.set(false);
                                    state.active_view.set(ActiveView::Home);
                                },
                                span { class: "tree-label", "{device_id}" }
                                if !worst.is_normal() {
                                    span { class: "{worst_class}" }
                                }
                                span { class: "tree-badge", "{point_count}" }
                            }
                        }
                    }
                }
            }
        }
    }
}
