use dioxus::prelude::*;

use crate::gui::state::{ActiveView, AppState, NavNode, NavNodeKind, insert_nav_child, remove_nav_node};

#[component]
pub fn NavTree() -> Element {
    let state = use_context::<AppState>();
    let tree = state.nav_tree.read().clone();

    rsx! {
        div { class: "nav-tree",
            if tree.is_empty() {
                div { class: "nav-empty",
                    p { "No items yet." }
                    p { class: "nav-empty-hint", "Click + to add a node." }
                }
            }
            ul { class: "tree-list",
                for node in &tree {
                    NavNodeView { node: node.clone(), depth: 0 }
                }
            }
            // Root-level add button
            AddNodeButton { parent_id: None, depth: 0 }
        }
    }
}

/// Inline add-node button + form.
#[component]
fn AddNodeButton(parent_id: Option<String>, depth: u32) -> Element {
    let mut state = use_context::<AppState>();
    let mut adding = use_signal(|| false);
    let mut name_input = use_signal(|| String::new());
    let mut kind_choice = use_signal(|| "folder".to_string());
    let mut device_choice = use_signal(|| String::new());

    let is_adding = *adding.read();
    let is_child = parent_id.is_some();

    // Pre-collect device list so we don't need state in the rsx after the closure
    let device_ids: Vec<String> = state
        .loaded
        .devices
        .iter()
        .map(|d| d.instance_id.clone())
        .collect();
    let first_device = device_ids.first().cloned().unwrap_or_default();

    if !is_adding {
        let label = if is_child { "+ Child" } else { "+ Add" };
        let btn_class = if is_child { "nav-add-btn nav-add-child" } else { "nav-add-btn" };
        return rsx! {
            button {
                class: btn_class,
                onclick: move |_| {
                    adding.set(true);
                    name_input.set(String::new());
                    kind_choice.set("folder".into());
                    device_choice.set(first_device.clone());
                },
                "{label}"
            }
        };
    }

    let kind_str = kind_choice.read().clone();
    let pid = parent_id.clone();

    let confirm = move |_| {
        let label = name_input.read().trim().to_string();
        if label.is_empty() {
            return;
        }

        let node_id = state.alloc_node_id();
        let kind = match kind_choice.read().as_str() {
            "page" => NavNodeKind::Page,
            "device" => NavNodeKind::Device {
                device_id: device_choice.read().clone(),
            },
            _ => NavNodeKind::Folder,
        };

        let new_node = NavNode {
            id: node_id.clone(),
            label,
            kind: kind.clone(),
            children: Vec::new(),
        };

        let mut tree = state.nav_tree.write();
        if let Some(ref parent) = pid {
            insert_nav_child(&mut tree, parent, new_node);
        } else {
            tree.push(new_node);
        }
        drop(tree);

        match kind {
            NavNodeKind::Page => {
                state.active_view.set(ActiveView::Page(node_id));
            }
            NavNodeKind::Device { device_id } => {
                state.selected_device.set(Some(device_id.clone()));
                state.active_view.set(ActiveView::Device { node_id, device_id });
            }
            NavNodeKind::Folder => {}
        }

        adding.set(false);
    };

    rsx! {
        div { class: "nav-add-form",
            input {
                r#type: "text",
                placeholder: "Name",
                value: "{name_input}",
                oninput: move |e| name_input.set(e.value()),
            }
            select {
                value: "{kind_choice}",
                onchange: move |e| kind_choice.set(e.value()),
                option { value: "folder", "Folder" }
                option { value: "page", "Page" }
                option { value: "device", "Device" }
            }
            if kind_str == "device" {
                select {
                    value: "{device_choice}",
                    onchange: move |e| device_choice.set(e.value()),
                    for did in &device_ids {
                        option { value: "{did}", "{did}" }
                    }
                }
            }
            div { class: "nav-add-actions",
                button {
                    class: "nav-confirm-btn",
                    onclick: confirm,
                    "Add"
                }
                button {
                    class: "nav-cancel-btn",
                    onclick: move |_| adding.set(false),
                    "Cancel"
                }
            }
        }
    }
}

#[component]
fn NavNodeView(node: NavNode, depth: u32) -> Element {
    let mut state = use_context::<AppState>();
    let mut expanded = use_signal(|| true);
    let is_open = *expanded.read();
    let has_children = !node.children.is_empty();
    let active_view = state.active_view.read().clone();
    let node_id = node.id.clone();

    let is_active = match (&active_view, &node.kind) {
        (ActiveView::Page(id), NavNodeKind::Page) => id == &node.id,
        (ActiveView::Device { node_id: nid, .. }, NavNodeKind::Device { .. }) => nid == &node.id,
        _ => false,
    };

    let icon = match &node.kind {
        NavNodeKind::Folder => if is_open && has_children { "\u{1F4C2}" } else { "\u{1F4C1}" },
        NavNodeKind::Page => "\u{1F4C4}",
        NavNodeKind::Device { .. } => "\u{2699}\u{FE0F}",
    };

    // For delete — need the node id
    let delete_id = node.id.clone();
    let child_depth = depth + 1;

    rsx! {
        li { class: "tree-node",
            div {
                class: if is_active { "tree-node-row active" } else { "tree-node-row" },
                onclick: {
                    let nid = node_id.clone();
                    let kind = node.kind.clone();
                    move |_| {
                        match &kind {
                            NavNodeKind::Folder => {
                                expanded.set(!is_open);
                            }
                            NavNodeKind::Page => {
                                state.active_view.set(ActiveView::Page(nid.clone()));
                            }
                            NavNodeKind::Device { device_id } => {
                                state.selected_device.set(Some(device_id.clone()));
                                state.selected_point.set(None);
                                state.detail_open.set(false);
                                state.active_view.set(ActiveView::Device {
                                    node_id: nid.clone(),
                                    device_id: device_id.clone(),
                                });
                            }
                        }
                    }
                },

                if has_children || matches!(node.kind, NavNodeKind::Folder) {
                    span {
                        class: if is_open { "tree-arrow open" } else { "tree-arrow" },
                        onclick: move |e| {
                            e.stop_propagation();
                            expanded.set(!is_open);
                        },
                        "\u{25B6}"
                    }
                } else {
                    span { class: "tree-arrow-spacer" }
                }

                span { class: "nav-icon", "{icon}" }

                span { class: "tree-label", "{node.label}" }

                // Delete button (appears on hover via CSS)
                button {
                    class: "nav-delete-btn",
                    title: "Delete",
                    onclick: {
                        let did = delete_id.clone();
                        move |e: Event<MouseData>| {
                            e.stop_propagation();
                            // If deleting the active page/device, go Home
                            let is_active = match &*state.active_view.read() {
                                ActiveView::Page(id) => id == &did,
                                ActiveView::Device { node_id, .. } => node_id == &did,
                                _ => false,
                            };
                            let mut tree = state.nav_tree.write();
                            remove_nav_node(&mut tree, &did);
                            drop(tree);
                            if is_active {
                                state.active_view.set(ActiveView::Home);
                            }
                        }
                    },
                    "\u{00D7}"
                }
            }

            if is_open {
                ul { class: "tree-list",
                    for child in &node.children {
                        NavNodeView { node: child.clone(), depth: child_depth }
                    }
                }
                // Any node type can have children
                AddNodeButton { parent_id: Some(node_id.clone()), depth: child_depth }
            }
        }
    }
}

