use dioxus::prelude::*;

use crate::gui::state::{ActiveView, AppState, TrendDashboard};

#[component]
pub fn Toolbar() -> Element {
    let state = use_context::<AppState>();
    let active = state.active_view.read().clone();
    let title = state.view_title();
    let is_history = matches!(active, ActiveView::History);

    let has_dash = is_history && state.active_dashboard_id.read().is_some();

    rsx! {
        div { class: "toolbar",
            div { class: "toolbar-left",
                // OpenCrate logo / file menu
                button {
                    class: "toolbar-btn logo-btn",
                    title: "Menu",
                    onclick: move |_| { /* TODO: file menu */ },
                    img {
                        src: asset!("/assets/opencrate_icon.svg"),
                        width: "20",
                        height: "20",
                    }
                }

                // Divider
                span { class: "toolbar-divider" }

                // Home
                NavButton {
                    view: ActiveView::Home,
                    active_view: active.clone(),
                    label: "Home",
                    icon_path: "M10 20v-6h4v6h5v-8h3L12 3 2 12h3v8z",
                }

                // Alarms (with badge)
                AlarmNavButton { active_view: active.clone() }

                // Schedules
                NavButton {
                    view: ActiveView::Schedules,
                    active_view: active.clone(),
                    label: "Schedules",
                    icon_path: "M19 3h-1V1h-2v2H8V1H6v2H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm0 16H5V8h14v11zM9 10H7v2h2v-2zm4 0h-2v2h2v-2zm4 0h-2v2h2v-2z",
                }

                // History/Trend
                NavButton {
                    view: ActiveView::History,
                    active_view: active.clone(),
                    label: "History",
                    icon_path: "M3.5 18.5l6-6 4 4L22 6.92l-1.41-1.41-7.09 7.97-4-4L2 16.99z",
                }

                // Divider before Config
                span { class: "toolbar-divider" }

                // Config mode (gear icon)
                NavButton {
                    view: ActiveView::Config,
                    active_view: active.clone(),
                    label: "Config",
                    icon_path: "M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 00.12-.61l-1.92-3.32a.49.49 0 00-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 00-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96a.49.49 0 00-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.07.62-.07.94s.02.64.07.94l-2.03 1.58a.49.49 0 00-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6A3.6 3.6 0 1112 8.4a3.6 3.6 0 010 7.2z",
                }

                // Dashboard tabs (shown when History is active)
                if is_history {
                    span { class: "toolbar-divider" }
                    DashboardTabs {}
                }
            }
            if !has_dash {
                div { class: "toolbar-center",
                    span { class: "toolbar-title", "{title}" }
                }
            }
            div { class: "toolbar-right" }
        }
    }
}

#[component]
fn NavButton(
    view: ActiveView,
    active_view: ActiveView,
    label: &'static str,
    icon_path: &'static str,
) -> Element {
    let mut state = use_context::<AppState>();
    let is_active = active_view == view;

    rsx! {
        button {
            class: if is_active { "toolbar-btn nav-btn active" } else { "toolbar-btn nav-btn" },
            title: "{label}",
            onclick: move |_| {
                state.active_view.set(view.clone());
                state.selected_point.set(None);
                state.detail_open.set(false);
            },
            svg {
                width: "18",
                height: "18",
                view_box: "0 0 24 24",
                fill: "currentColor",
                path { d: "{icon_path}" }
            }
        }
    }
}

/// Dashboard tabs shown in the toolbar when History view is active.
#[component]
fn DashboardTabs() -> Element {
    let mut state = use_context::<AppState>();
    let dashboards = state.dashboards.read().clone();
    let active_id = state.active_dashboard_id.read().clone();

    rsx! {
        div { class: "dash-tabs-scroll",
            for dash in &dashboards {
                {
                    let is_active = active_id.as_deref() == Some(&dash.id);
                    let dash_id = dash.id.clone();
                    rsx! {
                        button {
                            class: if is_active { "toolbar-btn dash-tab active" } else { "toolbar-btn dash-tab" },
                            onclick: move |_| {
                                state.active_dashboard_id.set(Some(dash_id.clone()));
                                state.selected_widget.set(None);
                            },
                            "{dash.name}"
                        }
                    }
                }
            }

            // "+" button to create new dashboard
            button {
                class: "toolbar-btn dash-add-btn",
                title: "New Dashboard",
                onclick: move |_| {
                    let id = format!("dash-{}", state.dashboards.read().len() + 1);
                    let name = format!("Dashboard {}", state.dashboards.read().len() + 1);
                    let new_dash = TrendDashboard {
                        id: id.clone(),
                        name,
                        widgets: Vec::new(),
                    };
                    let mut dashes = state.dashboards.read().clone();
                    dashes.push(new_dash);
                    state.dashboards.set(dashes);
                    state.active_dashboard_id.set(Some(id));
                    state.selected_widget.set(None);
                },
                "+"
            }
        }
    }
}

/// Alarm nav button with unacknowledged count badge.
#[component]
fn AlarmNavButton(active_view: ActiveView) -> Element {
    let mut state = use_context::<AppState>();
    let is_active = matches!(active_view, ActiveView::Alarms);

    // Poll active alarm count periodically
    let mut alarm_count = use_signal(|| 0u32);
    let alarm_store = state.alarm_store.clone();
    use_future(move || {
        let store = alarm_store.clone();
        async move {
            loop {
                let alarms = store.get_active_alarms().await;
                let unacked = alarms
                    .iter()
                    .filter(|a| a.state == crate::store::alarm_store::AlarmState::Offnormal)
                    .count() as u32;
                alarm_count.set(unacked);
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
            }
        }
    });

    let count = *alarm_count.read();
    let has_alarms = count > 0;

    rsx! {
        button {
            class: if is_active { "toolbar-btn nav-btn active" } else { "toolbar-btn nav-btn" },
            title: "Alarms",
            onclick: move |_| {
                state.active_view.set(ActiveView::Alarms);
                state.selected_point.set(None);
                state.detail_open.set(false);
            },
            div { class: "alarm-btn-wrap",
                svg {
                    width: "18",
                    height: "18",
                    view_box: "0 0 24 24",
                    fill: "currentColor",
                    path { d: "M12 22c1.1 0 2-.9 2-2h-4c0 1.1.9 2 2 2zm6-6v-5c0-3.07-1.63-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.64 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z" }
                }
                if has_alarms {
                    span { class: "alarm-badge", "{count}" }
                }
            }
        }
    }
}
