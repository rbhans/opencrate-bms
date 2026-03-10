use std::collections::HashMap;
use std::sync::Arc;

use dioxus::prelude::*;
use tokio::sync::{mpsc::UnboundedSender, Mutex};

use crate::bridge::bacnet::BacnetBridge;
use crate::config::loader::LoadedScenario;
use crate::config::profile::PointValue;
use crate::event::bus::EventBus;
use crate::discovery::service::DiscoveryService;
use crate::store::alarm_store::AlarmStore;
use crate::store::discovery_store::DiscoveryStore;
use crate::store::entity_store::EntityStore;
use crate::store::history_store::HistoryStore;
use crate::store::node_store::NodeStore;
use crate::store::point_store::PointStore;
use crate::store::schedule_store::ScheduleStore;

// ----------------------------------------------------------------
// Floor plan / page canvas data model
// ----------------------------------------------------------------

/// All content for a single page canvas.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PageData {
    /// File path to the background image (floor plan).
    pub background: Option<String>,
    /// Zones drawn on the canvas.
    pub zones: Vec<Zone>,
    /// Equipment symbols placed on the canvas.
    pub equipment: Vec<Equipment>,
}

/// Where a zone gets its setpoint value.
#[derive(Debug, Clone, PartialEq)]
pub enum SetpointSource {
    /// Read from a device point.
    Point(String),
    /// Static numeric value.
    Static(f64),
}

/// Label display options for a zone on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneLabelConfig {
    pub show_label: bool,
    pub show_room_number: bool,
    pub show_temp: bool,
    pub tooltip_label: bool,
    pub tooltip_room_number: bool,
    pub tooltip_temp: bool,
    pub font_size: f64,
    pub font_color: String,
}

impl Default for ZoneLabelConfig {
    fn default() -> Self {
        Self {
            show_label: true,
            show_room_number: true,
            show_temp: true,
            tooltip_label: false,
            tooltip_room_number: false,
            tooltip_temp: false,
            font_size: 24.0,
            font_color: "#ffffff".into(),
        }
    }
}

/// A polygon zone on the floor plan (room, area, etc.).
/// Coordinates are in canvas space (default 1920×1080).
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
    pub id: String,
    pub label: String,
    pub room_number: String,
    /// Device serving this zone (optional).
    pub device_id: Option<String>,
    /// Polygon vertices in canvas coordinates.
    pub points: Vec<(f64, f64)>,
    pub color: String,
    /// Point ID for zone temperature reading (within device).
    pub temp_point_id: Option<String>,
    /// Setpoint: from a device point or a static value.
    pub setpoint_source: Option<SetpointSource>,
    /// Label display options.
    pub label_config: ZoneLabelConfig,
    /// Corresponding nav tree node ID (auto-created).
    pub nav_node_id: Option<String>,
}

/// Label placement relative to an equipment symbol.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LabelPlacement {
    Top,
    Bottom,
    Left,
    Right,
}

/// Equipment label display options.
#[derive(Debug, Clone, PartialEq)]
pub struct EquipLabelConfig {
    pub show_label: bool,
    pub tooltip: bool,
    pub font_color: String,
    pub placement: LabelPlacement,
}

impl Default for EquipLabelConfig {
    fn default() -> Self {
        Self {
            show_label: true,
            tooltip: false,
            font_color: "#e0e0e0".into(),
            placement: LabelPlacement::Bottom,
        }
    }
}

/// Dummy symbol choices for equipment.
#[derive(Debug, Clone, PartialEq)]
pub enum EquipSymbol {
    Gear,
    Fan,
    Thermometer,
    Valve,
    Pump,
}

impl EquipSymbol {
    pub fn all() -> &'static [EquipSymbol] {
        &[Self::Gear, Self::Fan, Self::Thermometer, Self::Valve, Self::Pump]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Gear => "Gear",
            Self::Fan => "Fan",
            Self::Thermometer => "Thermometer",
            Self::Valve => "Valve",
            Self::Pump => "Pump",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Self::Gear => "gear",
            Self::Fan => "fan",
            Self::Thermometer => "thermometer",
            Self::Valve => "valve",
            Self::Pump => "pump",
        }
    }

    pub fn from_id(id: &str) -> Self {
        match id {
            "fan" => Self::Fan,
            "thermometer" => Self::Thermometer,
            "valve" => Self::Valve,
            "pump" => Self::Pump,
            _ => Self::Gear,
        }
    }
}

/// An equipment symbol placed on the floor plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Equipment {
    pub id: String,
    pub label: String,
    pub device_id: Option<String>,
    pub x: f64,
    pub y: f64,
    pub label_config: EquipLabelConfig,
    pub symbol: EquipSymbol,
}

/// Which canvas tool is currently active.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CanvasTool {
    Select,
    Pan,
    DrawZone,
    PlaceEquipment,
}

/// What's selected on the canvas.
#[derive(Debug, Clone, PartialEq)]
pub enum CanvasSelection {
    None,
    Zone(String),
    Equipment(String),
}

#[derive(Debug, Clone)]
pub struct WriteCommand {
    pub device_id: String,
    pub point_id: String,
    pub value: PointValue,
    pub priority: Option<u8>,
}

/// What's shown in the main content area.
#[derive(Debug, Clone, PartialEq)]
pub enum ActiveView {
    Home,
    Alarms,
    Schedules,
    History,
    Config,
    /// A graphic page canvas, keyed by node id.
    Page(String),
    /// A device view (point table), keyed by node id. Carries the device_id to look up.
    Device { node_id: String, device_id: String },
}

// ----------------------------------------------------------------
// Trend dashboard data model
// ----------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrendRange {
    Hour1,
    Hour4,
    Hour24,
    Day7,
    Day30,
}

impl TrendRange {
    pub fn millis(&self) -> i64 {
        match self {
            TrendRange::Hour1 => 3_600_000,
            TrendRange::Hour4 => 14_400_000,
            TrendRange::Hour24 => 86_400_000,
            TrendRange::Day7 => 604_800_000,
            TrendRange::Day30 => 2_592_000_000,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            TrendRange::Hour1 => "1h",
            TrendRange::Hour4 => "4h",
            TrendRange::Hour24 => "24h",
            TrendRange::Day7 => "7d",
            TrendRange::Day30 => "30d",
        }
    }

    pub fn all() -> &'static [TrendRange] {
        &[
            TrendRange::Hour1,
            TrendRange::Hour4,
            TrendRange::Hour24,
            TrendRange::Day7,
            TrendRange::Day30,
        ]
    }
}

/// A data source for a dashboard widget — one device/point pair.
#[derive(Debug, Clone, PartialEq)]
pub struct WidgetSource {
    pub device_id: String,
    pub point_id: String,
    pub label: String,
    pub color: String,
}

/// What kind of visualization a widget renders.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WidgetKind {
    Chart,
    Gauge,
    Table,
    Value,
}

impl WidgetKind {
    pub fn all() -> &'static [WidgetKind] {
        &[Self::Chart, Self::Gauge, Self::Table, Self::Value]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Chart => "Chart",
            Self::Gauge => "Gauge",
            Self::Table => "Table",
            Self::Value => "Value",
        }
    }

    pub fn icon_path(&self) -> &'static str {
        match self {
            // Trend line
            Self::Chart => "M3.5 18.5l6-6 4 4L22 6.92l-1.41-1.41-7.09 7.97-4-4L2 16.99z",
            // Speed gauge
            Self::Gauge => "M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm0 18c-4.42 0-8-3.58-8-8s3.58-8 8-8 8 3.58 8 8-3.58 8-8 8zm3.5-9c.83 0 1.5-.67 1.5-1.5S16.33 8 15.5 8 14 8.67 14 9.5s.67 1.5 1.5 1.5zm-7 0c.83 0 1.5-.67 1.5-1.5S9.33 8 8.5 8 7 8.67 7 9.5 7.67 11 8.5 11zm3.5 6.5c2.33 0 4.31-1.46 5.11-3.5H6.89c.8 2.04 2.78 3.5 5.11 3.5z",
            // Table grid
            Self::Table => "M3 3v18h18V3H3zm8 16H5v-6h6v6zm0-8H5V5h6v6zm8 8h-6v-6h6v6zm0-8h-6V5h6v6z",
            // Single number
            Self::Value => "M19 3H5c-1.1 0-2 .9-2 2v14c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V5c0-1.1-.9-2-2-2zm-7 14H7v-2h5v2zm5-4H7v-2h10v2zm0-4H7V7h10v2z",
        }
    }
}

/// A widget placed on a dashboard canvas (absolute pixel positioning).
#[derive(Debug, Clone, PartialEq)]
pub struct DashboardWidget {
    pub id: String,
    pub kind: WidgetKind,
    /// X position in pixels.
    pub x: f64,
    /// Y position in pixels.
    pub y: f64,
    /// Width in pixels.
    pub w: f64,
    /// Height in pixels.
    pub h: f64,
    /// Data sources (multiple device/point pairs).
    pub sources: Vec<WidgetSource>,
    /// Time range for chart widgets.
    pub range: TrendRange,
}

/// A saved trend dashboard.
#[derive(Debug, Clone, PartialEq)]
pub struct TrendDashboard {
    pub id: String,
    pub name: String,
    pub widgets: Vec<DashboardWidget>,
}

/// Grid snap size in pixels for dashboard widget positioning.
pub const GRID_SNAP: f64 = 20.0;

/// Snap a value to the nearest grid unit.
pub fn snap(val: f64) -> f64 {
    (val / GRID_SNAP).round() * GRID_SNAP
}

/// Active drag operation on a widget (all coordinates in page space).
#[derive(Debug, Clone, PartialEq)]
pub enum DragOp {
    /// Moving the widget — stores page coords at drag start + original widget position.
    Move {
        widget_id: String,
        start_page_x: f64,
        start_page_y: f64,
        orig_x: f64,
        orig_y: f64,
    },
    /// Resizing from bottom-right corner — stores page coords at drag start + original size.
    Resize {
        widget_id: String,
        start_page_x: f64,
        start_page_y: f64,
        orig_w: f64,
        orig_h: f64,
    },
}

/// What tool is active on the dashboard canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DashboardTool {
    Select,
    AddWidget(WidgetKind),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SidebarTab {
    Devices,
    Nav,
}

/// What kind of content a nav node represents.
#[derive(Debug, Clone, PartialEq)]
pub enum NavNodeKind {
    /// Container only — just holds children, no content of its own.
    Folder,
    /// Blank graphic page / canvas.
    Page,
    /// Links to a device (shows its point table in main content).
    Device { device_id: String },
}

/// A node in the navigation hierarchy (user-built).
#[derive(Debug, Clone, PartialEq)]
pub struct NavNode {
    pub id: String,
    pub label: String,
    pub kind: NavNodeKind,
    pub children: Vec<NavNode>,
}

#[derive(Clone)]
pub struct AppState {
    pub store: PointStore,
    pub node_store: NodeStore,
    pub event_bus: EventBus,
    pub loaded: LoadedScenario,
    pub active_view: Signal<ActiveView>,
    pub sidebar_tab: Signal<SidebarTab>,
    pub selected_device: Signal<Option<String>>,
    pub selected_point: Signal<Option<String>>,
    pub detail_open: Signal<bool>,
    pub store_version: Signal<u64>,
    pub nav_tree: Signal<Vec<NavNode>>,
    pub write_tx: UnboundedSender<WriteCommand>,
    pub write_error: Signal<Option<String>>,
    /// Counter for generating unique node IDs.
    pub next_node_id: Signal<u32>,
    /// Page canvas data, keyed by page node ID.
    pub pages: Signal<HashMap<String, PageData>>,
    /// History query handle.
    pub history_store: HistoryStore,
    /// Saved dashboards.
    pub dashboards: Signal<Vec<TrendDashboard>>,
    /// ID of the currently active dashboard (None = no dashboard open).
    pub active_dashboard_id: Signal<Option<String>>,
    /// Currently selected widget ID on the dashboard.
    pub selected_widget: Signal<Option<String>>,
    /// Active dashboard tool.
    pub dashboard_tool: Signal<DashboardTool>,
    /// Counter for widget IDs.
    pub next_widget_id: Signal<u32>,
    /// Active drag operation.
    pub drag_op: Signal<Option<DragOp>>,
    /// Quick-trend: device + point shown inline on default history page.
    pub quick_trend_device: Signal<Option<String>>,
    pub quick_trend_point: Signal<Option<String>>,
    pub quick_trend_range: Signal<TrendRange>,
    /// Alarm system handle.
    pub alarm_store: AlarmStore,
    /// Schedule system handle.
    pub schedule_store: ScheduleStore,
    /// Entity store for Haystack semantic tagging.
    pub entity_store: EntityStore,
    /// Discovery store for device/point discovery.
    pub discovery_store: DiscoveryStore,
    /// Discovery service for scan + accept operations.
    pub discovery_service: Arc<DiscoveryService>,
    /// BACnet bridge handle for discovery scans.
    pub bacnet_bridge: Arc<Mutex<Option<BacnetBridge>>>,
}

impl AppState {
    pub fn view_title(&self) -> String {
        match &*self.active_view.read() {
            ActiveView::Home => "Home".into(),
            ActiveView::Alarms => "Alarms".into(),
            ActiveView::Schedules => "Schedules".into(),
            ActiveView::Config => "Configuration".into(),
            ActiveView::History => {
                if let Some(ref dash_id) = *self.active_dashboard_id.read() {
                    self.dashboards
                        .read()
                        .iter()
                        .find(|d| d.id == *dash_id)
                        .map(|d| d.name.clone())
                        .unwrap_or_else(|| "History".into())
                } else {
                    "History".into()
                }
            }
            ActiveView::Page(id) | ActiveView::Device { node_id: id, .. } => {
                find_node_label(&self.nav_tree.read(), id)
                    .unwrap_or_else(|| "Untitled".into())
            }
        }
    }

    pub fn alloc_node_id(&mut self) -> String {
        let id = *self.next_node_id.read();
        self.next_node_id.set(id + 1);
        format!("node-{id}")
    }
}

/// Insert a child node under the given parent ID in the nav tree.
pub fn insert_nav_child(nodes: &mut [NavNode], parent_id: &str, child: NavNode) -> bool {
    for node in nodes.iter_mut() {
        if node.id == parent_id {
            node.children.push(child);
            return true;
        }
        if insert_nav_child(&mut node.children, parent_id, child.clone()) {
            return true;
        }
    }
    false
}

/// Remove a node by ID from the nav tree.
pub fn remove_nav_node(nodes: &mut Vec<NavNode>, target_id: &str) -> bool {
    if let Some(pos) = nodes.iter().position(|n| n.id == target_id) {
        nodes.remove(pos);
        return true;
    }
    for node in nodes.iter_mut() {
        if remove_nav_node(&mut node.children, target_id) {
            return true;
        }
    }
    false
}

/// Update a node's label and kind by ID.
pub fn update_nav_node(nodes: &mut [NavNode], target_id: &str, label: String, kind: NavNodeKind) -> bool {
    for node in nodes.iter_mut() {
        if node.id == target_id {
            node.label = label;
            node.kind = kind;
            return true;
        }
        if update_nav_node(&mut node.children, target_id, label.clone(), kind.clone()) {
            return true;
        }
    }
    false
}

fn find_node_label(nodes: &[NavNode], node_id: &str) -> Option<String> {
    for node in nodes {
        if node.id == node_id {
            return Some(node.label.clone());
        }
        if let Some(label) = find_node_label(&node.children, node_id) {
            return Some(label);
        }
    }
    None
}
