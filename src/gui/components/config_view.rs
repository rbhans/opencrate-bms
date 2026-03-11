use std::collections::{HashMap, HashSet};

use dioxus::prelude::*;

use crate::config::loader::LoadedDevice;
use crate::gui::state::AppState;
use crate::haystack::prototypes::{EQUIP_PROTOTYPES, POINT_PROTOTYPES};
use crate::haystack::tags::{self, TagKind};
use crate::store::entity_store::Entity;

use crate::auth::Permission;

use super::discovery_view::DiscoveryView;
use super::programming_view::ProgrammingView;
use super::user_management::UserManagementView;
use super::virtual_points_view::VirtualPointsView;

// ----------------------------------------------------------------
// Config sub-tabs
// ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConfigSection {
    Haystack,
    Discovery,
    Programming,
    VirtualPoints,
    Users,
}

impl ConfigSection {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Haystack => "Haystack",
            Self::Discovery => "Discovery",
            Self::Programming => "Programming",
            Self::VirtualPoints => "Virtual Points",
            Self::Users => "Users",
        }
    }

    /// Returns sections visible to the current user.
    pub fn visible_sections(is_admin: bool) -> Vec<ConfigSection> {
        let mut sections = vec![
            Self::Haystack,
            Self::Discovery,
            Self::Programming,
            Self::VirtualPoints,
        ];
        if is_admin {
            sections.push(Self::Users);
        }
        sections
    }
}

// ----------------------------------------------------------------
// ConfigView — sub-tabbed config mode
// ----------------------------------------------------------------

#[component]
pub fn ConfigView() -> Element {
    let state = use_context::<AppState>();
    let can_manage_users = state.has_permission(Permission::ManageUsers);
    let sections = ConfigSection::visible_sections(can_manage_users);

    let mut section = use_signal(|| ConfigSection::Haystack);
    let current = *section.read();

    rsx! {
        div { class: "config-view",
            // Sub-tab bar
            div { class: "config-section-bar",
                for s in &sections {
                    {
                        let s_val = *s;
                        rsx! {
                            button {
                                class: if current == s_val { "config-section-btn active" } else { "config-section-btn" },
                                onclick: move |_| section.set(s_val),
                                "{s_val.label()}"
                            }
                        }
                    }
                }
            }

            // Section content
            div { class: "config-section-body",
                match current {
                    ConfigSection::Haystack => rsx! { HaystackView {} },
                    ConfigSection::Discovery => rsx! { DiscoveryView {} },
                    ConfigSection::Programming => rsx! { ProgrammingView {} },
                    ConfigSection::VirtualPoints => rsx! { VirtualPointsView {} },
                    ConfigSection::Users => rsx! { UserManagementView {} },
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Haystack View — 3-pane (device/point browser | tag editor | properties)
// ----------------------------------------------------------------

#[component]
fn HaystackView() -> Element {
    let selected_entity_id: Signal<Option<String>> = use_signal(|| None);
    let mut entity_version = use_signal(|| 0u64);
    let batch_selected: Signal<HashSet<String>> = use_signal(HashSet::new);
    let batch_mode = use_signal(|| false);

    // Watch entity store version for reactivity
    let state = use_context::<AppState>();
    let es = state.entity_store.clone();
    use_future(move || {
        let store = es.clone();
        async move {
            let mut rx = store.subscribe();
            loop {
                if rx.changed().await.is_err() {
                    break;
                }
                entity_version.set(*rx.borrow());
            }
        }
    });

    rsx! {
        HaystackDeviceBrowser {
            selected_entity_id,
            entity_version,
            batch_selected,
            batch_mode,
        }
        div { class: "main-content",
            if *batch_mode.read() {
                BatchTagEditor {
                    batch_selected,
                    entity_version,
                }
            } else {
                TagEditor {
                    selected_entity_id,
                    entity_version,
                }
            }
        }
        EntityProperties {
            selected_entity_id,
            entity_version,
            batch_mode,
            batch_selected,
        }
    }
}

// ----------------------------------------------------------------
// Left pane: Device/Point Browser (from loaded scenario)
// ----------------------------------------------------------------

#[component]
fn HaystackDeviceBrowser(
    selected_entity_id: Signal<Option<String>>,
    entity_version: Signal<u64>,
    batch_selected: Signal<HashSet<String>>,
    batch_mode: Signal<bool>,
) -> Element {
    let state = use_context::<AppState>();
    let mut search = use_signal(String::new);
    let query = search.read().clone();
    let is_batch = *batch_mode.read();
    let _ver = *entity_version.read();

    let filtered: Vec<&LoadedDevice> = state
        .loaded
        .devices
        .iter()
        .filter(|d| device_matches(d, &query))
        .collect();

    rsx! {
        div { class: "sidebar config-device-browser",
            div { class: "details-header",
                span { "Equipment / Points" }
            }

            // Search bar
            div { class: "sidebar-search",
                input {
                    class: "sidebar-search-input",
                    r#type: "text",
                    placeholder: "Search equipment & points...",
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

            // Batch mode toggle
            div { class: "config-browser-actions",
                button {
                    class: if is_batch { "config-batch-toggle active" } else { "config-batch-toggle" },
                    onclick: move |_| {
                        let new_val = !*batch_mode.read();
                        batch_mode.set(new_val);
                        if !new_val {
                            batch_selected.set(HashSet::new());
                        }
                    },
                    "Batch Edit"
                }
                if is_batch {
                    span { class: "config-batch-count",
                        "{batch_selected.read().len()} selected"
                    }
                    button {
                        class: "config-browser-action-btn",
                        onclick: move |_| batch_selected.set(HashSet::new()),
                        "Clear"
                    }
                }
            }

            // Device/point list
            div { class: "sidebar-content",
                if filtered.is_empty() && !query.is_empty() {
                    div { class: "tree-empty-search", "No matches" }
                }
                for dev in &filtered {
                    HaystackDeviceNode {
                        device_id: dev.instance_id.clone(),
                        filter: query.clone(),
                        selected_entity_id,
                        entity_version,
                        batch_selected,
                        batch_mode,
                    }
                }
            }
        }
    }
}

fn device_matches(dev: &LoadedDevice, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let q = query.to_lowercase();
    if dev.instance_id.to_lowercase().contains(&q)
        || dev.profile.profile.name.to_lowercase().contains(&q)
    {
        return true;
    }
    dev.profile
        .points
        .iter()
        .any(|p| p.id.to_lowercase().contains(&q) || p.name.to_lowercase().contains(&q))
}

#[component]
fn HaystackDeviceNode(
    device_id: String,
    filter: String,
    selected_entity_id: Signal<Option<String>>,
    entity_version: Signal<u64>,
    batch_selected: Signal<HashSet<String>>,
    batch_mode: Signal<bool>,
) -> Element {
    let state = use_context::<AppState>();
    let mut expanded = use_signal(|| false);
    let is_batch = *batch_mode.read();
    let _ver = *entity_version.read();

    // Look up device from loaded scenario
    let device = state.loaded.devices.iter().find(|d| d.instance_id == device_id);
    let Some(device) = device else {
        return rsx! {};
    };

    let equip_entity_id = device.instance_id.clone();
    let is_selected = selected_entity_id.read().as_deref() == Some(&equip_entity_id);

    // Check if entity exists in entity store
    let es = state.entity_store.clone();
    let eid = equip_entity_id.clone();
    let entity_exists = use_resource(move || {
        let store = es.clone();
        let id = eid.clone();
        let _v = *entity_version.read();
        async move { store.get_entity(&id).await.ok() }
    });

    let has_entity = entity_exists.read().as_ref().map(|e| e.is_some()).unwrap_or(false);
    let tag_count = entity_exists
        .read()
        .as_ref()
        .and_then(|e| e.as_ref().map(|ent| ent.tags.len()))
        .unwrap_or(0);

    let profile_name = device.profile.profile.name.clone();
    let q_lower = filter.to_lowercase();

    // Pre-collect filtered points for rendering
    let visible_points: Vec<_> = device.profile.points.iter().filter(|pt| {
        filter.is_empty()
            || pt.id.to_lowercase().contains(&q_lower)
            || pt.name.to_lowercase().contains(&q_lower)
            || device.instance_id.to_lowercase().contains(&q_lower)
    }).collect();

    let click_eid = equip_entity_id.clone();
    let dev_id_display = device.instance_id.clone();

    rsx! {
        div {
            class: if is_selected && !is_batch { "config-device-node selected" } else { "config-device-node" },
            onclick: move |_| {
                if !is_batch {
                    selected_entity_id.set(Some(click_eid.clone()));
                }
            },

            if is_batch {
                {
                    let eid_check = equip_entity_id.clone();
                    let sel = batch_selected.read().clone();
                    let device_checked = sel.contains(&eid_check);
                    // Check if any child points are selected
                    let point_ids: Vec<String> = device.profile.points.iter()
                        .map(|pt| format!("{}/{}", device_id, pt.id))
                        .collect();
                    let all_points_checked = !point_ids.is_empty() && point_ids.iter().all(|pid| sel.contains(pid));

                    // Tri-state: ✓ = device + all points, — = device only, unchecked = none
                    let check_state = if device_checked && all_points_checked {
                        "all" // ✓
                    } else if device_checked {
                        "partial" // —
                    } else {
                        "none" // unchecked
                    };

                    rsx! {
                        span {
                            class: "config-tristate-check",
                            onclick: move |evt| {
                                evt.stop_propagation();
                                let mut set = batch_selected.read().clone();
                                match check_state {
                                    "all" => {
                                        // ✓ → — (device only, remove points)
                                        for pid in &point_ids {
                                            set.remove(pid);
                                        }
                                        // Keep device selected
                                        set.insert(eid_check.clone());
                                    }
                                    "partial" => {
                                        // — → unchecked (remove device + any remaining points)
                                        set.remove(&eid_check);
                                        for pid in &point_ids {
                                            set.remove(pid);
                                        }
                                    }
                                    _ => {
                                        // unchecked → ✓ (select device + all points)
                                        set.insert(eid_check.clone());
                                        for pid in &point_ids {
                                            set.insert(pid.clone());
                                        }
                                    }
                                }
                                batch_selected.set(set);
                            },
                            match check_state {
                                "all" => "☑",
                                "partial" => "▣",
                                _ => "☐",
                            }
                        }
                    }
                }
            }

            span {
                class: "config-tree-toggle",
                onclick: move |evt| {
                    evt.stop_propagation();
                    expanded.set(!expanded());
                },
                if *expanded.read() { "\u{25BE}" } else { "\u{25B8}" }
            }

            div {
                class: "config-device-info",
                span { class: "config-type-badge config-type-equip", "E" }
                span { class: "config-device-name", "{dev_id_display}" }
                span { class: "config-device-profile", "{profile_name}" }
                if has_entity && tag_count > 0 {
                    span { class: "config-tag-count", "{tag_count}" }
                }
                if !has_entity {
                    span { class: "config-no-entity", "untagged" }
                }
            }
        }

        // Expanded: show points
        if *expanded.read() {
            for pt in &visible_points {
                {
                    let point_entity_id = format!("{}/{}", device_id, pt.id);
                    let is_pt_selected = selected_entity_id.read().as_deref() == Some(&point_entity_id);
                    let pt_name = pt.name.clone();
                    let pt_units = pt.units.clone().unwrap_or_default();

                    // Check entity exists
                    let es2 = state.entity_store.clone();
                    let peid = point_entity_id.clone();
                    let pt_entity = use_resource(move || {
                        let store = es2.clone();
                        let id = peid.clone();
                        let _v = *entity_version.read();
                        async move { store.get_entity(&id).await.ok() }
                    });

                    let pt_has_entity = pt_entity.read().as_ref().map(|e| e.is_some()).unwrap_or(false);
                    let pt_tag_count = pt_entity
                        .read()
                        .as_ref()
                        .and_then(|e| e.as_ref().map(|ent| ent.tags.len()))
                        .unwrap_or(0);

                    let click_peid = point_entity_id.clone();
                    let batch_peid = point_entity_id.clone();
                    let check_peid = point_entity_id.clone();

                    rsx! {
                        div {
                            class: if is_pt_selected && !is_batch { "config-point-node selected" } else { "config-point-node" },
                            onclick: move |_| {
                                if !is_batch {
                                    selected_entity_id.set(Some(click_peid.clone()));
                                }
                            },

                            if is_batch {
                                {
                                    let is_checked = batch_selected.read().contains(&check_peid);
                                    rsx! {
                                        input {
                                            r#type: "checkbox",
                                            checked: is_checked,
                                            onclick: move |evt| {
                                                evt.stop_propagation();
                                                let mut set = batch_selected.read().clone();
                                                if is_checked {
                                                    set.remove(&batch_peid);
                                                } else {
                                                    set.insert(batch_peid.clone());
                                                }
                                                batch_selected.set(set);
                                            },
                                        }
                                    }
                                }
                            }

                            div {
                                class: "config-point-info",
                                span { class: "config-type-badge config-type-point", "P" }
                                span { class: "config-point-name", "{pt_name}" }
                                if !pt_units.is_empty() {
                                    span { class: "config-point-units", "{pt_units}" }
                                }
                                if pt_has_entity && pt_tag_count > 0 {
                                    span { class: "config-tag-count", "{pt_tag_count}" }
                                }
                                if !pt_has_entity {
                                    span { class: "config-no-entity", "untagged" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Center pane: Tag Editor (single entity)
// ----------------------------------------------------------------

#[component]
fn TagEditor(
    selected_entity_id: Signal<Option<String>>,
    entity_version: Signal<u64>,
) -> Element {
    let state = use_context::<AppState>();

    // Read signals inside resource so it re-runs when selection changes
    let es = state.entity_store.clone();
    let entity_res = use_resource(move || {
        let store = es.clone();
        let id = selected_entity_id.read().clone();
        let _v = *entity_version.read();
        async move {
            match id {
                Some(eid) => store.get_entity(&eid).await.ok(),
                None => None,
            }
        }
    });

    let sel_id = selected_entity_id.read().clone();
    let Some(entity_id) = sel_id else {
        return rsx! {
            div { class: "config-tag-editor-empty",
                p { class: "placeholder", "Select an equipment or point to view and edit tags." }
            }
        };
    };

    let is_point = entity_id.contains('/');
    let entity_type = if is_point { "point" } else { "equip" };

    // Extract the entity if found
    let entity_opt: Option<Entity> = entity_res
        .read()
        .as_ref()
        .and_then(|e: &Option<Entity>| e.clone());

    match entity_opt {
        Some(entity) => rsx! {
            EntityTagEditor {
                entity,
                entity_version,
            }
        },
        None => rsx! {
            CreateEntityPrompt {
                entity_id: entity_id.clone(),
                entity_type: entity_type.to_string(),
            }
        },
    }
}

/// Shown when a device/point is selected but has no entity in the store yet.
#[component]
fn CreateEntityPrompt(entity_id: String, entity_type: String) -> Element {
    let state = use_context::<AppState>();
    let display_name = if entity_id.contains('/') {
        entity_id.split('/').last().unwrap_or(&entity_id).to_string()
    } else {
        entity_id.clone()
    };

    // For equipment, gather point info so we can auto-tag all points too
    let device = if entity_type == "equip" {
        state
            .loaded
            .devices
            .iter()
            .find(|d| d.instance_id == entity_id)
    } else {
        None
    };
    let device_points: Vec<(String, String, Option<String>)> = device
        .map(|d| {
            d.profile
                .points
                .iter()
                .map(|pt| (pt.id.clone(), pt.name.clone(), pt.units.clone()))
                .collect()
        })
        .unwrap_or_default();
    let profile_name = device
        .map(|d| d.profile.profile.name.clone())
        .unwrap_or_default();
    let point_count = device_points.len();

    rsx! {
        div { class: "config-tag-editor config-create-prompt",
            div { class: "config-tag-header",
                span { class: "config-type-badge config-type-{entity_type}",
                    if entity_type == "point" { "P" } else { "E" }
                }
                h3 { "{display_name}" }
            }
            p { class: "config-hint", "This item has no Haystack entity yet." }
            p { class: "config-hint", "Create one to start adding tags." }

            {
                let eid = entity_id.clone();
                let etype = entity_type.clone();
                let dname = display_name.clone();
                let es = state.entity_store.clone();
                let pts = device_points.clone();
                // Derive parent_id for points: device entity ID
                let parent = if eid.contains('/') {
                    Some(eid.split('/').next().unwrap_or("").to_string())
                } else {
                    None
                };

                rsx! {
                    div { class: "config-create-actions",
                        button {
                            class: "config-btn config-btn-primary",
                            onclick: move |_| {
                                let store = es.clone();
                                let id = eid.clone();
                                let et = etype.clone();
                                let dn = dname.clone();
                                let pid = parent.clone();
                                let points = pts.clone();

                                let provider = crate::haystack::provider::Haystack4Provider;
                                let pname = profile_name.clone();

                                // Build equip tags
                                let mut initial_tags = vec![(et.clone(), None)];
                                let equip_tags_map: HashMap<String, Option<String>>;
                                if et == "equip" {
                                    let suggested = crate::haystack::auto_tag::suggest_equip_tags(
                                        &pname,
                                        &provider,
                                    );
                                    equip_tags_map = suggested.iter().cloned().collect();
                                    for (name, val) in &suggested {
                                        if !initial_tags.iter().any(|(n, _)| n == name) {
                                            initial_tags.push((name.clone(), val.clone()));
                                        }
                                    }
                                } else {
                                    equip_tags_map = HashMap::new();
                                    // For single point, auto-tag using both ID and display name
                                    let point_id_part = id.split('/').last().unwrap_or(&id);
                                    let suggested = crate::haystack::auto_tag::suggest_point_tags_multi(
                                        &[point_id_part, &dn],
                                        None,
                                        &equip_tags_map,
                                        &provider,
                                    );
                                    for (name, val) in suggested {
                                        if !initial_tags.iter().any(|(n, _)| n == &name) {
                                            initial_tags.push((name, val));
                                        }
                                    }
                                }

                                spawn(async move {
                                    // Create the main entity
                                    let _ = store.create_entity(
                                        &id,
                                        &et,
                                        &dn,
                                        pid.as_deref(),
                                        initial_tags,
                                    ).await;

                                    // For equipment: also create all point entities with auto-tags
                                    if et == "equip" {
                                        for (pt_id, pt_name, pt_units) in &points {
                                            let point_entity_id = format!("{}/{}", id, pt_id);
                                            // Use both ID and display name for better tag matching
                                            let suggested = crate::haystack::auto_tag::suggest_point_tags_multi(
                                                &[pt_id, pt_name],
                                                pt_units.as_deref(),
                                                &equip_tags_map,
                                                &provider,
                                            );
                                            let _ = store.create_entity(
                                                &point_entity_id,
                                                "point",
                                                pt_name,
                                                Some(&id),
                                                suggested,
                                            ).await;
                                        }
                                    }
                                });
                            },
                            if entity_type == "equip" && point_count > 0 {
                                "Auto-Tag Equipment + {point_count} Points"
                            } else {
                                "Create with Auto-Tags"
                            }
                        }
                        button {
                            class: "config-btn",
                            onclick: {
                                let store = state.entity_store.clone();
                                let id = entity_id.clone();
                                let et = entity_type.clone();
                                let dn = display_name.clone();
                                let pid = if id.contains('/') {
                                    Some(id.split('/').next().unwrap_or("").to_string())
                                } else {
                                    None
                                };
                                move |_| {
                                    let s = store.clone();
                                    let i = id.clone();
                                    let e = et.clone();
                                    let d = dn.clone();
                                    let p = pid.clone();
                                    spawn(async move {
                                        let _ = s.create_entity(
                                            &i,
                                            &e,
                                            &d,
                                            p.as_deref(),
                                            vec![(e.clone(), None)],
                                        ).await;
                                    });
                                }
                            },
                            "Create Empty Entity"
                        }
                    }
                }
            }
        }
    }
}

/// Tag editor for an existing entity.
#[component]
fn EntityTagEditor(entity: Entity, entity_version: Signal<u64>) -> Element {
    let state = use_context::<AppState>();
    let etype = entity.entity_type.clone();

    // Sort current tags
    let mut sorted_tags: Vec<_> = entity.tags.iter().collect();
    sorted_tags.sort_by_key(|(name, _)| (*name).clone());

    rsx! {
        div { class: "config-tag-editor",
            // Header
            div { class: "config-tag-header",
                span { class: "config-type-badge config-type-{etype}",
                    match etype.as_str() {
                        "equip" => "E",
                        "point" => "P",
                        "site" => "S",
                        "space" => "Sp",
                        _ => "?",
                    }
                }
                h3 { "{entity.dis}" }
                span { class: "config-entity-id-label", "{entity.id}" }
            }

            // Current tags
            div { class: "config-tag-list",
                h4 { class: "config-section-title", "Applied Tags ({sorted_tags.len()})" }
                if sorted_tags.is_empty() {
                    p { class: "config-hint", "No tags applied yet." }
                }
                div { class: "config-tag-chips",
                    for (tag_name, tag_value) in &sorted_tags {
                        {
                            let tn = tag_name.to_string();
                            let tv = (*tag_value).clone();
                            let remove_tn = tn.clone();
                            let remove_eid = entity.id.clone();
                            let es_remove = state.entity_store.clone();

                            rsx! {
                                div { class: "config-tag-chip",
                                    span { class: "config-tag-name", "{tn}" }
                                    if let Some(ref val) = tv {
                                        span { class: "config-tag-value", "= {val}" }
                                    }
                                    button {
                                        class: "config-tag-remove",
                                        title: "Remove tag",
                                        onclick: move |_| {
                                            let store = es_remove.clone();
                                            let eid = remove_eid.clone();
                                            let tname = remove_tn.clone();
                                            spawn(async move {
                                                let _ = store.remove_tag(&eid, &tname).await;
                                            });
                                        },
                                        "x"
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Add tag dropdown
            AddTagDropdown {
                entity_id: entity.id.clone(),
                entity_type: etype.clone(),
                current_tags: entity.tags.clone(),
            }

            // Apply prototype
            ApplyPrototype {
                entity_id: entity.id.clone(),
                entity_type: etype.clone(),
            }
        }
    }
}

// ----------------------------------------------------------------
// Batch Tag Editor
// ----------------------------------------------------------------

#[component]
fn BatchTagEditor(
    batch_selected: Signal<HashSet<String>>,
    entity_version: Signal<u64>,
) -> Element {
    let state = use_context::<AppState>();
    let selected = batch_selected.read().clone();
    let count = selected.len();
    let _ver = *entity_version.read();

    if count == 0 {
        return rsx! {
            div { class: "config-tag-editor-empty",
                p { class: "placeholder", "Select items using checkboxes to batch edit tags." }
            }
        };
    }

    // Load all selected entities to find common tags
    let es = state.entity_store.clone();
    let ids: Vec<String> = selected.iter().cloned().collect();
    let entities_res = use_resource(move || {
        let store = es.clone();
        let entity_ids = ids.clone();
        let _v = *entity_version.read();
        async move {
            let mut entities = Vec::new();
            for id in &entity_ids {
                if let Ok(e) = store.get_entity(id).await {
                    entities.push(e);
                }
            }
            entities
        }
    });

    let entities = entities_res.read();
    let entities = entities.as_ref().map(|v| v.as_slice()).unwrap_or(&[]);

    // Find common tags (tags present in ALL selected entities)
    let common_tags: Vec<(String, Option<String>)> = if entities.is_empty() {
        vec![]
    } else {
        let first_tags: HashSet<&String> = entities[0].tags.keys().collect();
        let common_keys: Vec<&String> = first_tags
            .into_iter()
            .filter(|k| entities.iter().all(|e| e.tags.contains_key(*k)))
            .collect();
        let mut result: Vec<_> = common_keys
            .iter()
            .map(|k| (k.to_string(), entities[0].tags.get(*k).cloned().flatten()))
            .collect();
        result.sort_by(|a, b| a.0.cmp(&b.0));
        result
    };

    // Count how many actually have entities vs untagged
    let entity_count = entities.len();
    let untagged_count = count.saturating_sub(entity_count);

    let mut batch_tag_search = use_signal(String::new);
    let mut batch_show_dropdown = use_signal(|| false);

    let query = batch_tag_search.read().to_lowercase();
    // Show all tags (equip + point combined) for batch
    let all_tags = tags::tags_for_entity("equip");
    let point_tags = tags::tags_for_entity("point");
    let mut combined: Vec<_> = all_tags;
    for t in point_tags {
        if !combined.iter().any(|c| c.name == t.name) {
            combined.push(t);
        }
    }
    combined.sort_by_key(|t| t.name);

    let filtered: Vec<_> = combined
        .iter()
        .filter(|t| query.is_empty() || t.name.to_lowercase().contains(&query) || t.doc.to_lowercase().contains(&query))
        .collect();

    rsx! {
        div { class: "config-tag-editor config-batch-editor",
            div { class: "config-tag-header",
                h3 { "Batch Edit — {count} items" }
                if untagged_count > 0 {
                    span { class: "config-hint", "({untagged_count} untagged)" }
                }
            }

            // Auto-tag all selected items
            {
                let es_auto = state.entity_store.clone();
                let sel_auto = selected.clone();
                let loaded = state.loaded.clone();
                rsx! {
                    div { class: "config-create-actions",
                        button {
                            class: "config-btn config-btn-primary",
                            onclick: move |_| {
                                let store = es_auto.clone();
                                let ids: Vec<String> = sel_auto.iter().cloned().collect();
                                let devices = loaded.devices.clone();
                                spawn(async move {
                                    let provider = crate::haystack::provider::Haystack4Provider;
                                    for id in &ids {
                                        let is_point = id.contains('/');
                                        if is_point {
                                            // Point entity
                                            let parts: Vec<&str> = id.splitn(2, '/').collect();
                                            let device_id = parts[0];
                                            let point_id = parts.get(1).unwrap_or(&"");
                                            // Find device and point info
                                            let dev = devices.iter().find(|d| d.instance_id == device_id);
                                            let (pt_name, pt_units) = dev
                                                .and_then(|d| d.profile.points.iter().find(|p| p.id == *point_id))
                                                .map(|p| (p.name.clone(), p.units.clone()))
                                                .unwrap_or_else(|| (point_id.to_string(), None));

                                            // Get parent equip tags for context
                                            let equip_tags_map: HashMap<String, Option<String>> = dev
                                                .map(|d| {
                                                    crate::haystack::auto_tag::suggest_equip_tags(
                                                        &d.profile.profile.name,
                                                        &provider,
                                                    ).into_iter().collect()
                                                })
                                                .unwrap_or_default();

                                            let suggested = crate::haystack::auto_tag::suggest_point_tags_multi(
                                                &[point_id, &pt_name],
                                                pt_units.as_deref(),
                                                &equip_tags_map,
                                                &provider,
                                            );

                                            // Create entity if it doesn't exist, or add tags
                                            if store.get_entity(id).await.ok().is_some() {
                                                // Entity exists — add missing tags
                                                for (name, val) in &suggested {
                                                    let _ = store.set_tag(id, name, val.as_deref()).await;
                                                }
                                            } else {
                                                let _ = store.create_entity(
                                                    id,
                                                    "point",
                                                    &pt_name,
                                                    Some(device_id),
                                                    suggested,
                                                ).await;
                                            }
                                        } else {
                                            // Equipment entity
                                            let dev = devices.iter().find(|d| d.instance_id == *id);
                                            let profile_name = dev
                                                .map(|d| d.profile.profile.name.clone())
                                                .unwrap_or_default();
                                            let suggested = crate::haystack::auto_tag::suggest_equip_tags(
                                                &profile_name,
                                                &provider,
                                            );
                                            let mut tags = vec![("equip".to_string(), None)];
                                            for (name, val) in &suggested {
                                                if !tags.iter().any(|(n, _)| n == name) {
                                                    tags.push((name.clone(), val.clone()));
                                                }
                                            }

                                            if store.get_entity(id).await.ok().is_some() {
                                                for (name, val) in &tags {
                                                    let _ = store.set_tag(id, name, val.as_deref()).await;
                                                }
                                            } else {
                                                let dis = dev.map(|d| d.instance_id.clone()).unwrap_or_else(|| id.clone());
                                                let _ = store.create_entity(
                                                    id,
                                                    "equip",
                                                    &dis,
                                                    None,
                                                    tags,
                                                ).await;
                                            }
                                        }
                                    }
                                });
                            },
                            "Auto-Tag Selected ({count})"
                        }
                    }
                }
            }

            // Common tags section
            if !common_tags.is_empty() {
                div { class: "config-tag-list",
                    h4 { class: "config-section-title", "Common Tags (all {entity_count} entities)" }
                    div { class: "config-tag-chips",
                        for (tag_name, tag_value) in &common_tags {
                            {
                                let tn = tag_name.clone();
                                let tv = tag_value.clone();
                                let remove_tn = tn.clone();
                                let es = state.entity_store.clone();
                                let sel = selected.clone();

                                rsx! {
                                    div { class: "config-tag-chip",
                                        span { class: "config-tag-name", "{tn}" }
                                        if let Some(ref val) = tv {
                                            span { class: "config-tag-value", "= {val}" }
                                        }
                                        button {
                                            class: "config-tag-remove",
                                            title: "Remove from all",
                                            onclick: move |_| {
                                                let store = es.clone();
                                                let tname = remove_tn.clone();
                                                let ids: Vec<String> = sel.iter().cloned().collect();
                                                spawn(async move {
                                                    for id in &ids {
                                                        let _ = store.remove_tag(id, &tname).await;
                                                    }
                                                });
                                            },
                                            "x"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Add tag to all
            div { class: "config-add-tag-section",
                h4 { class: "config-section-title", "Add Tag to All" }
                div { class: "config-tag-search-wrap",
                    input {
                        class: "config-input config-tag-search",
                        r#type: "text",
                        placeholder: "Search tags to add...",
                        value: "{batch_tag_search}",
                        oninput: move |evt| {
                            batch_tag_search.set(evt.value());
                            batch_show_dropdown.set(true);
                        },
                        onfocus: move |_| batch_show_dropdown.set(true),
                    }
                }

                if *batch_show_dropdown.read() && !filtered.is_empty() {
                    div { class: "config-tag-dropdown",
                        for tag_def in filtered.iter().take(20) {
                            {
                                let tname = tag_def.name.to_string();
                                let tkind = tag_def.kind.clone();
                                let tdoc = tag_def.doc;
                                let es = state.entity_store.clone();
                                let sel = selected.clone();

                                rsx! {
                                    div {
                                        class: "config-tag-option",
                                        onclick: move |_| {
                                            batch_show_dropdown.set(false);
                                            batch_tag_search.set(String::new());

                                            if tkind == TagKind::Marker {
                                                let store = es.clone();
                                                let ids: Vec<String> = sel.iter().cloned().collect();
                                                let name = tname.clone();
                                                spawn(async move {
                                                    for id in &ids {
                                                        let _ = store.set_tag(id, &name, None).await;
                                                    }
                                                });
                                            }
                                            // TODO: value tags in batch mode need input
                                        },
                                        span { class: "config-tag-opt-name", "{tname}" }
                                        span { class: "config-tag-opt-doc", "{tdoc}" }
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

// ----------------------------------------------------------------
// Add Tag Dropdown (single entity)
// ----------------------------------------------------------------

#[component]
fn AddTagDropdown(
    entity_id: String,
    entity_type: String,
    current_tags: HashMap<String, Option<String>>,
) -> Element {
    let state = use_context::<AppState>();
    let mut search = use_signal(String::new);
    let mut show_dropdown = use_signal(|| false);
    let mut pending_value = use_signal(String::new);
    let mut pending_tag: Signal<Option<String>> = use_signal(|| None);

    let available = tags::tags_for_entity(&entity_type);
    let query = search.read().to_lowercase();
    let filtered: Vec<_> = available
        .iter()
        .filter(|t| !current_tags.contains_key(t.name))
        .filter(|t| {
            query.is_empty()
                || t.name.to_lowercase().contains(&query)
                || t.doc.to_lowercase().contains(&query)
        })
        .collect();

    rsx! {
        div { class: "config-add-tag-section",
            h4 { class: "config-section-title", "Add Tag" }

            if let Some(ref tag_name) = *pending_tag.read() {
                {
                    let tn = tag_name.clone();
                    let eid = entity_id.clone();
                    let es = state.entity_store.clone();
                    rsx! {
                        div { class: "config-value-input",
                            span { class: "config-tag-name", "{tn}" }
                            input {
                                class: "config-input",
                                r#type: "text",
                                placeholder: "Enter value...",
                                value: "{pending_value}",
                                oninput: move |evt| pending_value.set(evt.value()),
                            }
                            button {
                                class: "config-btn config-btn-primary",
                                onclick: move |_| {
                                    let store = es.clone();
                                    let entity = eid.clone();
                                    let name = tn.clone();
                                    let val = pending_value.read().clone();
                                    spawn(async move {
                                        let v = if val.is_empty() { None } else { Some(val.as_str()) };
                                        let _ = store.set_tag(&entity, &name, v).await;
                                    });
                                    pending_tag.set(None);
                                    pending_value.set(String::new());
                                },
                                "Set"
                            }
                            button {
                                class: "config-btn",
                                onclick: move |_| {
                                    pending_tag.set(None);
                                    pending_value.set(String::new());
                                },
                                "Cancel"
                            }
                        }
                    }
                }
            } else {
                div { class: "config-tag-search-wrap",
                    input {
                        class: "config-input config-tag-search",
                        r#type: "text",
                        placeholder: "Search tags...",
                        value: "{search}",
                        oninput: move |evt| {
                            search.set(evt.value());
                            show_dropdown.set(true);
                        },
                        onfocus: move |_| show_dropdown.set(true),
                    }
                }

                if *show_dropdown.read() && !filtered.is_empty() {
                    div { class: "config-tag-dropdown",
                        for tag_def in filtered.iter().take(20) {
                            {
                                let tname = tag_def.name.to_string();
                                let tkind = tag_def.kind.clone();
                                let tdoc = tag_def.doc;
                                let eid = entity_id.clone();
                                let es = state.entity_store.clone();

                                rsx! {
                                    div {
                                        class: "config-tag-option",
                                        onclick: move |_| {
                                            show_dropdown.set(false);
                                            search.set(String::new());

                                            if tkind == TagKind::Marker {
                                                let store = es.clone();
                                                let entity = eid.clone();
                                                let name = tname.clone();
                                                spawn(async move {
                                                    let _ = store.set_tag(&entity, &name, None).await;
                                                });
                                            } else {
                                                pending_tag.set(Some(tname.clone()));
                                                pending_value.set(String::new());
                                            }
                                        },
                                        span { class: "config-tag-opt-name", "{tname}" }
                                        span { class: "config-tag-opt-doc", "{tdoc}" }
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

// ----------------------------------------------------------------
// Apply Prototype
// ----------------------------------------------------------------

#[component]
fn ApplyPrototype(entity_id: String, entity_type: String) -> Element {
    let state = use_context::<AppState>();
    let mut show_protos = use_signal(|| false);

    let prototypes = match entity_type.as_str() {
        "equip" => EQUIP_PROTOTYPES.iter().collect::<Vec<_>>(),
        "point" => POINT_PROTOTYPES.iter().collect::<Vec<_>>(),
        _ => vec![],
    };

    if prototypes.is_empty() {
        return rsx! {};
    }

    rsx! {
        div { class: "config-prototype-section",
            h4 { class: "config-section-title", "Prototypes" }
            button {
                class: "config-btn",
                onclick: move |_| show_protos.set(!show_protos()),
                if *show_protos.read() { "Hide Prototypes" } else { "Apply Prototype..." }
            }

            if *show_protos.read() {
                div { class: "config-proto-list",
                    for proto in &prototypes {
                        {
                            let pname = proto.name;
                            let pdoc = proto.doc;
                            let ptags: Vec<(String, Option<String>)> = proto
                                .tags
                                .iter()
                                .map(|&(n, v)| (n.to_string(), v.map(|s| s.to_string())))
                                .collect();
                            let eid = entity_id.clone();
                            let es = state.entity_store.clone();

                            rsx! {
                                div {
                                    class: "config-proto-card",
                                    onclick: move |_| {
                                        let store = es.clone();
                                        let entity = eid.clone();
                                        let tags = ptags.clone();
                                        spawn(async move {
                                            let _ = store.set_tags(&entity, tags).await;
                                        });
                                        show_protos.set(false);
                                    },
                                    div { class: "config-proto-name", "{pname}" }
                                    div { class: "config-proto-doc", "{pdoc}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Right pane: Entity Properties + Relationships
// ----------------------------------------------------------------

#[component]
fn EntityProperties(
    selected_entity_id: Signal<Option<String>>,
    entity_version: Signal<u64>,
    batch_mode: Signal<bool>,
    batch_selected: Signal<HashSet<String>>,
) -> Element {
    let state = use_context::<AppState>();

    // Always call hooks unconditionally — read signal inside resource
    let es = state.entity_store.clone();
    let entity_res: Resource<Option<Entity>> = use_resource(move || {
        let store = es.clone();
        let id = selected_entity_id.read().clone();
        let _v = *entity_version.read();
        async move {
            match id {
                Some(eid) => store.get_entity(&eid).await.ok(),
                None => None,
            }
        }
    });

    let mut edit_name = use_signal(String::new);

    if *batch_mode.read() {
        return rsx! {
            div { class: "details-pane config-properties",
                div { class: "details-header", span { "Batch Info" } }
                div { class: "point-detail-body",
                    p { class: "config-hint",
                        "{batch_selected.read().len()} items selected for batch editing."
                    }
                    p { class: "config-hint",
                        "Use the center pane to add or remove tags across all selected items."
                    }
                }
            }
        };
    }

    let sel_id = selected_entity_id.read().clone();

    let Some(entity_id) = sel_id else {
        return rsx! {
            div { class: "details-pane config-properties",
                div { class: "details-header", span { "Properties" } }
                div { class: "point-detail-body",
                    p { class: "placeholder", "Select an item." }
                }
            }
        };
    };

    let entity_opt = entity_res.read().as_ref().and_then(|e: &Option<Entity>| e.clone());

    let Some(entity) = entity_opt else {
        return rsx! {
            div { class: "details-pane config-properties",
                div { class: "details-header", span { "Properties" } }
                div { class: "point-detail-body",
                    p { class: "config-hint", "No entity created yet for this item." }
                    p { class: "config-hint", "Select it and create an entity to edit properties." }
                }
            }
        };
    };

    // Sync edit_name when entity changes
    if *edit_name.read() != entity.dis && edit_name.read().is_empty() || entity_id != entity.id {
        edit_name.set(entity.dis.clone());
    }
    let name_changed = *edit_name.read() != entity.dis;

    rsx! {
        div { class: "details-pane config-properties",
            div { class: "details-header", span { "Properties" } }

            div { class: "point-detail-body",
                // Display name
                div { class: "config-prop-group",
                    label { class: "config-prop-label", "Display Name" }
                    div { class: "config-prop-row",
                        input {
                            class: "config-input",
                            r#type: "text",
                            value: "{edit_name}",
                            oninput: move |evt| edit_name.set(evt.value()),
                        }
                        if name_changed {
                            {
                                let eid = entity.id.clone();
                                let es = state.entity_store.clone();
                                rsx! {
                                    button {
                                        class: "config-btn config-btn-primary",
                                        onclick: move |_| {
                                            let store = es.clone();
                                            let id = eid.clone();
                                            let new_name = edit_name.read().clone();
                                            spawn(async move {
                                                let _ = store.update_entity(&id, &new_name).await;
                                            });
                                        },
                                        "Save"
                                    }
                                }
                            }
                        }
                    }
                }

                // Entity type
                div { class: "config-prop-group",
                    label { class: "config-prop-label", "Type" }
                    span { class: "config-prop-value", "{entity.entity_type}" }
                }

                // ID
                div { class: "config-prop-group",
                    label { class: "config-prop-label", "ID" }
                    span { class: "config-prop-value config-prop-id", "{entity.id}" }
                }

                // Parent
                if entity.parent_id.is_some() {
                    div { class: "config-prop-group",
                        label { class: "config-prop-label", "Parent" }
                        span { class: "config-prop-value",
                            {entity.parent_id.as_deref().unwrap_or("—")}
                        }
                    }
                }

                // Refs
                if !entity.refs.is_empty() {
                    div { class: "config-prop-group",
                        label { class: "config-prop-label", "References" }
                        for (ref_tag, target_id) in entity.refs.iter() {
                            div { class: "config-ref-row",
                                span { class: "config-ref-tag", "{ref_tag}" }
                                span { class: "config-ref-target", "{target_id}" }
                            }
                        }
                    }
                }

                // Add ref
                AddRefSection { entity_id: entity.id.clone() }

                // Delete entity
                div { class: "config-prop-group config-danger-zone",
                    {
                        let eid = entity.id.clone();
                        let es = state.entity_store.clone();
                        rsx! {
                            button {
                                class: "config-btn config-btn-danger",
                                onclick: move |_| {
                                    let store = es.clone();
                                    let id = eid.clone();
                                    spawn(async move {
                                        let _ = store.delete_entity(&id).await;
                                    });
                                    selected_entity_id.set(None);
                                },
                                "Delete Entity"
                            }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn AddRefSection(entity_id: String) -> Element {
    let state = use_context::<AppState>();
    let mut ref_tag = use_signal(|| "siteRef".to_string());
    let mut target_id = use_signal(String::new);
    let mut show_form = use_signal(|| false);

    let ref_options = vec!["siteRef", "equipRef", "spaceRef"];

    rsx! {
        div { class: "config-prop-group",
            label { class: "config-prop-label", "Add Reference" }
            if !*show_form.read() {
                button {
                    class: "config-btn",
                    onclick: move |_| show_form.set(true),
                    "+ Add Ref"
                }
            } else {
                div { class: "config-ref-form",
                    select {
                        class: "config-select",
                        value: "{ref_tag}",
                        onchange: move |evt| ref_tag.set(evt.value()),
                        for opt in &ref_options {
                            option { value: "{opt}", "{opt}" }
                        }
                    }
                    input {
                        class: "config-input",
                        r#type: "text",
                        placeholder: "Target entity ID...",
                        value: "{target_id}",
                        oninput: move |evt| target_id.set(evt.value()),
                    }
                    div { class: "config-add-actions",
                        {
                            let eid = entity_id.clone();
                            let es = state.entity_store.clone();
                            rsx! {
                                button {
                                    class: "config-btn config-btn-primary",
                                    disabled: target_id.read().trim().is_empty(),
                                    onclick: move |_| {
                                        let store = es.clone();
                                        let src = eid.clone();
                                        let tag = ref_tag.read().clone();
                                        let tgt = target_id.read().trim().to_string();
                                        spawn(async move {
                                            let _ = store.set_ref(&src, &tag, &tgt).await;
                                        });
                                        target_id.set(String::new());
                                        show_form.set(false);
                                    },
                                    "Set"
                                }
                            }
                        }
                        button {
                            class: "config-btn",
                            onclick: move |_| show_form.set(false),
                            "Cancel"
                        }
                    }
                }
            }
        }
    }
}
