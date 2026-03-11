use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use crate::config::profile::PointValue;
use crate::event::bus::EventBus;
use crate::store::point_store::{PointKey, PointStore};

// ----------------------------------------------------------------
// Public types
// ----------------------------------------------------------------

pub type ScheduleId = i64;
pub type ExceptionGroupId = i64;
pub type AssignmentId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct TimeOfDay {
    pub hour: u8,
    pub minute: u8,
}

impl TimeOfDay {
    pub fn new(hour: u8, minute: u8) -> Self {
        Self { hour, minute }
    }

    pub fn total_minutes(&self) -> u16 {
        self.hour as u16 * 60 + self.minute as u16
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeSlot {
    pub time: TimeOfDay,
    pub value: PointValue,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DaySlots(pub Vec<TimeSlot>);

impl DaySlots {
    /// Ensure slots are sorted by time ascending.
    pub fn sort(&mut self) {
        self.0.sort_by_key(|s| s.time);
    }
}

/// 7-element array: 0=Monday .. 6=Sunday.
pub type WeeklySchedule = [DaySlots; 7];

pub fn empty_weekly() -> WeeklySchedule {
    std::array::from_fn(|_| DaySlots::default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleValueType {
    Binary,
    Analog,
    Multistate,
}

impl ScheduleValueType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Binary => "binary",
            Self::Analog => "analog",
            Self::Multistate => "multistate",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "binary" => Some(Self::Binary),
            "analog" => Some(Self::Analog),
            "multistate" => Some(Self::Multistate),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Binary => "Binary",
            Self::Analog => "Analog",
            Self::Multistate => "Multistate",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DateSpec {
    /// Recurring annual date (e.g. Jan 1).
    Fixed { month: u8, day: u8 },
    /// One-off date in a specific year.
    FixedYear { year: u16, month: u8, day: u8 },
    /// Relative date (e.g. fourth Thursday in November).
    Relative {
        ordinal: Ordinal,
        weekday: u8,
        month: u8,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ordinal {
    First,
    Second,
    Third,
    Fourth,
    Last,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub id: ScheduleId,
    pub name: String,
    pub description: String,
    pub value_type: ScheduleValueType,
    pub default_value: PointValue,
    pub enabled: bool,
    pub weekly: WeeklySchedule,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExceptionGroup {
    pub id: ExceptionGroupId,
    pub name: String,
    pub description: String,
    pub entries: Vec<DateSpec>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleException {
    pub id: i64,
    pub schedule_id: ScheduleId,
    pub group_id: Option<ExceptionGroupId>,
    pub name: String,
    pub date_spec: DateSpec,
    pub slots: DaySlots,
    pub use_default: bool,
    pub created_ms: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ScheduleAssignment {
    pub id: AssignmentId,
    pub schedule_id: ScheduleId,
    pub device_id: String,
    pub point_id: String,
    pub priority: i32,
    pub enabled: bool,
    pub created_ms: i64,
}

#[derive(Debug, Clone)]
pub struct ScheduleLogEntry {
    pub id: i64,
    pub assignment_id: AssignmentId,
    pub device_id: String,
    pub point_id: String,
    pub value_json: String,
    pub reason: String,
    pub timestamp_ms: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("database error: {0}")]
    Db(String),
    #[error("channel closed")]
    ChannelClosed,
    #[error("not found")]
    NotFound,
}

// ----------------------------------------------------------------
// Time helpers (no chrono dependency)
// ----------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct LocalTime {
    year: i32,
    month: u8,
    day: u8,
    weekday: u8, // 0=Monday .. 6=Sunday
    hour: u8,
    minute: u8,
}

#[repr(C)]
#[derive(Default)]
struct Tm {
    tm_sec: i32,
    tm_min: i32,
    tm_hour: i32,
    tm_mday: i32,
    tm_mon: i32,
    tm_year: i32,
    tm_wday: i32,
    tm_yday: i32,
    tm_isdst: i32,
    tm_gmtoff: i64,
    tm_zone: *const i8,
}

extern "C" {
    fn localtime_r(time: *const i64, result: *mut Tm) -> *mut Tm;
    fn mktime(tm: *mut Tm) -> i64;
}

fn local_time_now() -> LocalTime {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let mut tm = Tm::default();
    unsafe { localtime_r(&secs, &mut tm) };
    // tm_wday: 0=Sun, 1=Mon, ..., 6=Sat → convert to 0=Mon..6=Sun
    let weekday = if tm.tm_wday == 0 { 6 } else { (tm.tm_wday - 1) as u8 };
    LocalTime {
        year: tm.tm_year + 1900,
        month: (tm.tm_mon + 1) as u8,
        day: tm.tm_mday as u8,
        weekday,
        hour: tm.tm_hour as u8,
        minute: tm.tm_min as u8,
    }
}

/// Get the weekday (0=Mon..6=Sun) for a given date.
fn weekday_of(year: i32, month: u8, day: u8) -> u8 {
    let mut tm = Tm::default();
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32;
    tm.tm_hour = 12;
    unsafe { mktime(&mut tm) };
    if tm.tm_wday == 0 { 6 } else { (tm.tm_wday - 1) as u8 }
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 30,
    }
}

/// Resolve a DateSpec to (month, day) for the given year. Returns None if not applicable.
fn resolve_date_spec(spec: &DateSpec, year: i32) -> Option<(u8, u8)> {
    match spec {
        DateSpec::Fixed { month, day } => Some((*month, *day)),
        DateSpec::FixedYear {
            year: y,
            month,
            day,
        } => {
            if *y as i32 == year {
                Some((*month, *day))
            } else {
                None
            }
        }
        DateSpec::Relative {
            ordinal,
            weekday,
            month,
        } => {
            let dim = days_in_month(year, *month);
            let target_wd = *weekday; // 0=Mon..6=Sun

            match ordinal {
                Ordinal::Last => {
                    // Search backward from last day of month
                    for d in (1..=dim).rev() {
                        if weekday_of(year, *month, d) == target_wd {
                            return Some((*month, d));
                        }
                    }
                    None
                }
                _ => {
                    let n = match ordinal {
                        Ordinal::First => 1,
                        Ordinal::Second => 2,
                        Ordinal::Third => 3,
                        Ordinal::Fourth => 4,
                        Ordinal::Last => unreachable!(),
                    };
                    let mut count = 0;
                    for d in 1..=dim {
                        if weekday_of(year, *month, d) == target_wd {
                            count += 1;
                            if count == n {
                                return Some((*month, d));
                            }
                        }
                    }
                    None
                }
            }
        }
    }
}

// ----------------------------------------------------------------
// Commands sent to the SQLite thread
// ----------------------------------------------------------------

enum ScheduleCmd {
    // Schedule CRUD
    CreateSchedule {
        name: String,
        description: String,
        value_type: ScheduleValueType,
        default_value: PointValue,
        weekly: WeeklySchedule,
        reply: oneshot::Sender<Result<ScheduleId, ScheduleError>>,
    },
    UpdateSchedule {
        id: ScheduleId,
        name: String,
        description: String,
        default_value: PointValue,
        enabled: bool,
        weekly: WeeklySchedule,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    DeleteSchedule {
        id: ScheduleId,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    ListSchedules {
        reply: oneshot::Sender<Vec<Schedule>>,
    },
    GetSchedule {
        id: ScheduleId,
        reply: oneshot::Sender<Option<Schedule>>,
    },

    // Exception groups
    CreateExceptionGroup {
        name: String,
        description: String,
        entries: Vec<DateSpec>,
        reply: oneshot::Sender<Result<ExceptionGroupId, ScheduleError>>,
    },
    UpdateExceptionGroup {
        id: ExceptionGroupId,
        name: String,
        description: String,
        entries: Vec<DateSpec>,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    DeleteExceptionGroup {
        id: ExceptionGroupId,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    ListExceptionGroups {
        reply: oneshot::Sender<Vec<ExceptionGroup>>,
    },

    // Schedule exceptions
    AddException {
        schedule_id: ScheduleId,
        group_id: Option<ExceptionGroupId>,
        name: String,
        date_spec: DateSpec,
        slots: DaySlots,
        use_default: bool,
        reply: oneshot::Sender<Result<i64, ScheduleError>>,
    },
    RemoveException {
        id: i64,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    ListExceptions {
        schedule_id: ScheduleId,
        reply: oneshot::Sender<Vec<ScheduleException>>,
    },

    // Assignments
    CreateAssignment {
        schedule_id: ScheduleId,
        device_id: String,
        point_id: String,
        priority: i32,
        reply: oneshot::Sender<Result<AssignmentId, ScheduleError>>,
    },
    CreateAssignmentsBatch {
        schedule_id: ScheduleId,
        entries: Vec<(String, String)>,
        priority: i32,
        reply: oneshot::Sender<Result<Vec<AssignmentId>, ScheduleError>>,
    },
    DeleteAssignment {
        id: AssignmentId,
        reply: oneshot::Sender<Result<(), ScheduleError>>,
    },
    ListAssignmentsForSchedule {
        schedule_id: ScheduleId,
        reply: oneshot::Sender<Vec<ScheduleAssignment>>,
    },
    GetAssignmentsForPoint {
        device_id: String,
        point_id: String,
        reply: oneshot::Sender<Vec<ScheduleAssignment>>,
    },

    // Log
    InsertLog {
        assignment_id: AssignmentId,
        device_id: String,
        point_id: String,
        value_json: String,
        reason: String,
        timestamp_ms: i64,
    },
    QueryLog {
        device_id: String,
        point_id: String,
        limit: i64,
        reply: oneshot::Sender<Vec<ScheduleLogEntry>>,
    },

    // Engine queries (all schedules + assignments + exceptions in one shot)
    LoadEngineData {
        reply: oneshot::Sender<EngineData>,
    },
}

/// All data the engine needs, loaded in a single DB roundtrip.
#[derive(Debug, Clone)]
struct EngineData {
    schedules: Vec<Schedule>,
    assignments: Vec<ScheduleAssignment>,
    exceptions: Vec<ScheduleException>,
}

// ----------------------------------------------------------------
// ScheduleStore — async handle to the SQLite thread
// ----------------------------------------------------------------

#[derive(Clone)]
pub struct ScheduleStore {
    cmd_tx: mpsc::UnboundedSender<ScheduleCmd>,
    config_version_tx: watch::Sender<u64>,
    config_version_rx: watch::Receiver<u64>,
    event_bus: Option<EventBus>,
}

impl ScheduleStore {
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    pub async fn create_schedule(
        &self,
        name: &str,
        description: &str,
        value_type: ScheduleValueType,
        default_value: PointValue,
        weekly: WeeklySchedule,
    ) -> Result<ScheduleId, ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::CreateSchedule {
                name: name.to_string(),
                description: description.to_string(),
                value_type,
                default_value,
                weekly,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn update_schedule(
        &self,
        id: ScheduleId,
        name: &str,
        description: &str,
        default_value: PointValue,
        enabled: bool,
        weekly: WeeklySchedule,
    ) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::UpdateSchedule {
                id,
                name: name.to_string(),
                description: description.to_string(),
                default_value,
                enabled,
                weekly,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn delete_schedule(&self, id: ScheduleId) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::DeleteSchedule { id, reply: reply_tx })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn list_schedules(&self) -> Vec<Schedule> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::ListSchedules { reply: reply_tx });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn get_schedule(&self, id: ScheduleId) -> Option<Schedule> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::GetSchedule { id, reply: reply_tx });
        reply_rx.await.unwrap_or(None)
    }

    pub async fn create_exception_group(
        &self,
        name: &str,
        description: &str,
        entries: Vec<DateSpec>,
    ) -> Result<ExceptionGroupId, ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::CreateExceptionGroup {
                name: name.to_string(),
                description: description.to_string(),
                entries,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn update_exception_group(
        &self,
        id: ExceptionGroupId,
        name: &str,
        description: &str,
        entries: Vec<DateSpec>,
    ) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::UpdateExceptionGroup {
                id,
                name: name.to_string(),
                description: description.to_string(),
                entries,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn delete_exception_group(
        &self,
        id: ExceptionGroupId,
    ) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::DeleteExceptionGroup { id, reply: reply_tx })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn list_exception_groups(&self) -> Vec<ExceptionGroup> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::ListExceptionGroups { reply: reply_tx });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn add_exception(
        &self,
        schedule_id: ScheduleId,
        group_id: Option<ExceptionGroupId>,
        name: &str,
        date_spec: DateSpec,
        slots: DaySlots,
        use_default: bool,
    ) -> Result<i64, ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::AddException {
                schedule_id,
                group_id,
                name: name.to_string(),
                date_spec,
                slots,
                use_default,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn remove_exception(&self, id: i64) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::RemoveException { id, reply: reply_tx })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn list_exceptions(&self, schedule_id: ScheduleId) -> Vec<ScheduleException> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::ListExceptions {
            schedule_id,
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn create_assignment(
        &self,
        schedule_id: ScheduleId,
        device_id: &str,
        point_id: &str,
        priority: i32,
    ) -> Result<AssignmentId, ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::CreateAssignment {
                schedule_id,
                device_id: device_id.to_string(),
                point_id: point_id.to_string(),
                priority,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn create_assignments_batch(
        &self,
        schedule_id: ScheduleId,
        entries: &[(String, String)],
        priority: i32,
    ) -> Result<Vec<AssignmentId>, ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::CreateAssignmentsBatch {
                schedule_id,
                entries: entries.to_vec(),
                priority,
                reply: reply_tx,
            })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn delete_assignment(&self, id: AssignmentId) -> Result<(), ScheduleError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(ScheduleCmd::DeleteAssignment { id, reply: reply_tx })
            .map_err(|_| ScheduleError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| ScheduleError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_config_version();
        }
        result
    }

    pub async fn list_assignments_for_schedule(
        &self,
        schedule_id: ScheduleId,
    ) -> Vec<ScheduleAssignment> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::ListAssignmentsForSchedule {
            schedule_id,
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn get_assignments_for_point(
        &self,
        device_id: &str,
        point_id: &str,
    ) -> Vec<ScheduleAssignment> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::GetAssignmentsForPoint {
            device_id: device_id.to_string(),
            point_id: point_id.to_string(),
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn query_log(
        &self,
        device_id: &str,
        point_id: &str,
        limit: i64,
    ) -> Vec<ScheduleLogEntry> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::QueryLog {
            device_id: device_id.to_string(),
            point_id: point_id.to_string(),
            limit,
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub fn subscribe_config_changes(&self) -> watch::Receiver<u64> {
        self.config_version_rx.clone()
    }

    fn bump_config_version(&self) {
        let current = *self.config_version_rx.borrow();
        let _ = self.config_version_tx.send(current + 1);
    }

    // Internal: insert log entry
    fn insert_log(
        &self,
        assignment_id: AssignmentId,
        device_id: &str,
        point_id: &str,
        value_json: &str,
        reason: &str,
        timestamp_ms: i64,
    ) {
        let _ = self.cmd_tx.send(ScheduleCmd::InsertLog {
            assignment_id,
            device_id: device_id.to_string(),
            point_id: point_id.to_string(),
            value_json: value_json.to_string(),
            reason: reason.to_string(),
            timestamp_ms,
        });
    }

    // Internal: load engine data
    async fn load_engine_data(&self) -> EngineData {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(ScheduleCmd::LoadEngineData { reply: reply_tx });
        reply_rx.await.unwrap_or_else(|_| EngineData {
            schedules: Vec::new(),
            assignments: Vec::new(),
            exceptions: Vec::new(),
        })
    }
}

// ----------------------------------------------------------------
// SQLite schema & thread
// ----------------------------------------------------------------

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS schedule (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT NOT NULL UNIQUE,
    description   TEXT NOT NULL DEFAULT '',
    value_type    TEXT NOT NULL,
    default_value TEXT NOT NULL,
    enabled       INTEGER NOT NULL DEFAULT 1,
    weekly_json   TEXT NOT NULL,
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS exception_group (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT NOT NULL UNIQUE,
    description   TEXT NOT NULL DEFAULT '',
    entries_json  TEXT NOT NULL,
    created_ms    INTEGER NOT NULL,
    updated_ms    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS schedule_exception (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id     INTEGER NOT NULL REFERENCES schedule(id) ON DELETE CASCADE,
    group_id        INTEGER REFERENCES exception_group(id) ON DELETE SET NULL,
    name            TEXT NOT NULL DEFAULT '',
    date_spec_json  TEXT NOT NULL,
    slots_json      TEXT NOT NULL,
    use_default     INTEGER NOT NULL DEFAULT 0,
    created_ms      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sched_exc ON schedule_exception(schedule_id);

CREATE TABLE IF NOT EXISTS schedule_assignment (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    schedule_id   INTEGER NOT NULL REFERENCES schedule(id) ON DELETE CASCADE,
    device_id     TEXT NOT NULL,
    point_id      TEXT NOT NULL,
    priority      INTEGER NOT NULL DEFAULT 12,
    enabled       INTEGER NOT NULL DEFAULT 1,
    created_ms    INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_assign_unique ON schedule_assignment(schedule_id, device_id, point_id);
CREATE INDEX IF NOT EXISTS idx_assign_point ON schedule_assignment(device_id, point_id);

CREATE TABLE IF NOT EXISTS schedule_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    assignment_id   INTEGER NOT NULL,
    device_id       TEXT NOT NULL,
    point_id        TEXT NOT NULL,
    value_json      TEXT NOT NULL,
    reason          TEXT NOT NULL,
    timestamp_ms    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sched_log_time ON schedule_log(timestamp_ms);
";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

fn run_sqlite_thread(db_path: &Path, mut cmd_rx: mpsc::UnboundedReceiver<ScheduleCmd>) {
    let conn = rusqlite::Connection::open(db_path).expect("failed to open schedules database");
    conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;")
        .expect("failed to set pragmas");
    conn.execute_batch(SCHEMA)
        .expect("failed to create schedules schema");

    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            ScheduleCmd::CreateSchedule {
                name,
                description,
                value_type,
                default_value,
                weekly,
                reply,
            } => {
                let _ = reply.send(create_schedule_db(
                    &conn,
                    &name,
                    &description,
                    &value_type,
                    &default_value,
                    &weekly,
                ));
            }
            ScheduleCmd::UpdateSchedule {
                id,
                name,
                description,
                default_value,
                enabled,
                weekly,
                reply,
            } => {
                let _ = reply.send(update_schedule_db(
                    &conn,
                    id,
                    &name,
                    &description,
                    &default_value,
                    enabled,
                    &weekly,
                ));
            }
            ScheduleCmd::DeleteSchedule { id, reply } => {
                let _ = reply.send(delete_schedule_db(&conn, id));
            }
            ScheduleCmd::ListSchedules { reply } => {
                let _ = reply.send(list_schedules_db(&conn));
            }
            ScheduleCmd::GetSchedule { id, reply } => {
                let _ = reply.send(get_schedule_db(&conn, id));
            }
            ScheduleCmd::CreateExceptionGroup {
                name,
                description,
                entries,
                reply,
            } => {
                let _ = reply.send(create_exception_group_db(&conn, &name, &description, &entries));
            }
            ScheduleCmd::UpdateExceptionGroup {
                id,
                name,
                description,
                entries,
                reply,
            } => {
                let _ = reply.send(update_exception_group_db(
                    &conn,
                    id,
                    &name,
                    &description,
                    &entries,
                ));
            }
            ScheduleCmd::DeleteExceptionGroup { id, reply } => {
                let _ = reply.send(delete_exception_group_db(&conn, id));
            }
            ScheduleCmd::ListExceptionGroups { reply } => {
                let _ = reply.send(list_exception_groups_db(&conn));
            }
            ScheduleCmd::AddException {
                schedule_id,
                group_id,
                name,
                date_spec,
                slots,
                use_default,
                reply,
            } => {
                let _ = reply.send(add_exception_db(
                    &conn,
                    schedule_id,
                    group_id,
                    &name,
                    &date_spec,
                    &slots,
                    use_default,
                ));
            }
            ScheduleCmd::RemoveException { id, reply } => {
                let _ = reply.send(remove_exception_db(&conn, id));
            }
            ScheduleCmd::ListExceptions {
                schedule_id,
                reply,
            } => {
                let _ = reply.send(list_exceptions_db(&conn, schedule_id));
            }
            ScheduleCmd::CreateAssignment {
                schedule_id,
                device_id,
                point_id,
                priority,
                reply,
            } => {
                let _ = reply.send(create_assignment_db(
                    &conn,
                    schedule_id,
                    &device_id,
                    &point_id,
                    priority,
                ));
            }
            ScheduleCmd::CreateAssignmentsBatch {
                schedule_id,
                entries,
                priority,
                reply,
            } => {
                let _ = reply.send(create_assignments_batch_db(
                    &conn,
                    schedule_id,
                    &entries,
                    priority,
                ));
            }
            ScheduleCmd::DeleteAssignment { id, reply } => {
                let _ = reply.send(delete_assignment_db(&conn, id));
            }
            ScheduleCmd::ListAssignmentsForSchedule {
                schedule_id,
                reply,
            } => {
                let _ = reply.send(list_assignments_for_schedule_db(&conn, schedule_id));
            }
            ScheduleCmd::GetAssignmentsForPoint {
                device_id,
                point_id,
                reply,
            } => {
                let _ = reply.send(get_assignments_for_point_db(&conn, &device_id, &point_id));
            }
            ScheduleCmd::InsertLog {
                assignment_id,
                device_id,
                point_id,
                value_json,
                reason,
                timestamp_ms,
            } => {
                let _ = conn.execute(
                    "INSERT INTO schedule_log (assignment_id, device_id, point_id, value_json, reason, timestamp_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    rusqlite::params![assignment_id, device_id, point_id, value_json, reason, timestamp_ms],
                );
            }
            ScheduleCmd::QueryLog {
                device_id,
                point_id,
                limit,
                reply,
            } => {
                let _ = reply.send(query_log_db(&conn, &device_id, &point_id, limit));
            }
            ScheduleCmd::LoadEngineData { reply } => {
                let data = EngineData {
                    schedules: list_schedules_db(&conn),
                    assignments: list_all_assignments_db(&conn),
                    exceptions: list_all_exceptions_db(&conn),
                };
                let _ = reply.send(data);
            }
        }
    }
}

// ----------------------------------------------------------------
// DB helpers
// ----------------------------------------------------------------

fn create_schedule_db(
    conn: &rusqlite::Connection,
    name: &str,
    description: &str,
    value_type: &ScheduleValueType,
    default_value: &PointValue,
    weekly: &WeeklySchedule,
) -> Result<ScheduleId, ScheduleError> {
    let ts = now_ms();
    let default_json =
        serde_json::to_string(default_value).map_err(|e| ScheduleError::Db(e.to_string()))?;
    let weekly_json =
        serde_json::to_string(weekly).map_err(|e| ScheduleError::Db(e.to_string()))?;

    conn.execute(
        "INSERT INTO schedule (name, description, value_type, default_value, enabled, weekly_json, created_ms, updated_ms)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7)",
        rusqlite::params![
            name,
            description,
            value_type.as_str(),
            default_json,
            weekly_json,
            ts,
            ts,
        ],
    )
    .map_err(|e| ScheduleError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

fn update_schedule_db(
    conn: &rusqlite::Connection,
    id: ScheduleId,
    name: &str,
    description: &str,
    default_value: &PointValue,
    enabled: bool,
    weekly: &WeeklySchedule,
) -> Result<(), ScheduleError> {
    let ts = now_ms();
    let default_json =
        serde_json::to_string(default_value).map_err(|e| ScheduleError::Db(e.to_string()))?;
    let weekly_json =
        serde_json::to_string(weekly).map_err(|e| ScheduleError::Db(e.to_string()))?;

    let rows = conn
        .execute(
            "UPDATE schedule SET name = ?1, description = ?2, default_value = ?3, enabled = ?4, weekly_json = ?5, updated_ms = ?6 WHERE id = ?7",
            rusqlite::params![name, description, default_json, enabled as i32, weekly_json, ts, id],
        )
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn delete_schedule_db(conn: &rusqlite::Connection, id: ScheduleId) -> Result<(), ScheduleError> {
    // CASCADE handles assignments and exceptions
    let rows = conn
        .execute("DELETE FROM schedule WHERE id = ?1", rusqlite::params![id])
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn parse_schedule_row(row: &rusqlite::Row) -> rusqlite::Result<Schedule> {
    let id: i64 = row.get(0)?;
    let name: String = row.get(1)?;
    let description: String = row.get(2)?;
    let value_type_str: String = row.get(3)?;
    let default_json: String = row.get(4)?;
    let enabled: bool = row.get::<_, i32>(5)? != 0;
    let weekly_json: String = row.get(6)?;
    let created_ms: i64 = row.get(7)?;
    let updated_ms: i64 = row.get(8)?;

    let value_type =
        ScheduleValueType::from_str(&value_type_str).unwrap_or(ScheduleValueType::Analog);
    let default_value: PointValue =
        serde_json::from_str(&default_json).unwrap_or(PointValue::Float(0.0));
    let weekly: WeeklySchedule =
        serde_json::from_str(&weekly_json).unwrap_or_else(|_| empty_weekly());

    Ok(Schedule {
        id,
        name,
        description,
        value_type,
        default_value,
        enabled,
        weekly,
        created_ms,
        updated_ms,
    })
}

fn list_schedules_db(conn: &rusqlite::Connection) -> Vec<Schedule> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, name, description, value_type, default_value, enabled, weekly_json, created_ms, updated_ms FROM schedule ORDER BY id",
        )
        .unwrap();
    let rows = stmt.query_map([], parse_schedule_row).unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn get_schedule_db(conn: &rusqlite::Connection, id: ScheduleId) -> Option<Schedule> {
    conn.query_row(
        "SELECT id, name, description, value_type, default_value, enabled, weekly_json, created_ms, updated_ms FROM schedule WHERE id = ?1",
        rusqlite::params![id],
        parse_schedule_row,
    )
    .ok()
}

fn create_exception_group_db(
    conn: &rusqlite::Connection,
    name: &str,
    description: &str,
    entries: &[DateSpec],
) -> Result<ExceptionGroupId, ScheduleError> {
    let ts = now_ms();
    let entries_json =
        serde_json::to_string(entries).map_err(|e| ScheduleError::Db(e.to_string()))?;
    conn.execute(
        "INSERT INTO exception_group (name, description, entries_json, created_ms, updated_ms) VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![name, description, entries_json, ts, ts],
    )
    .map_err(|e| ScheduleError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

fn update_exception_group_db(
    conn: &rusqlite::Connection,
    id: ExceptionGroupId,
    name: &str,
    description: &str,
    entries: &[DateSpec],
) -> Result<(), ScheduleError> {
    let ts = now_ms();
    let entries_json =
        serde_json::to_string(entries).map_err(|e| ScheduleError::Db(e.to_string()))?;
    let rows = conn
        .execute(
            "UPDATE exception_group SET name = ?1, description = ?2, entries_json = ?3, updated_ms = ?4 WHERE id = ?5",
            rusqlite::params![name, description, entries_json, ts, id],
        )
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn delete_exception_group_db(
    conn: &rusqlite::Connection,
    id: ExceptionGroupId,
) -> Result<(), ScheduleError> {
    let rows = conn
        .execute(
            "DELETE FROM exception_group WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn list_exception_groups_db(conn: &rusqlite::Connection) -> Vec<ExceptionGroup> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, name, description, entries_json, created_ms, updated_ms FROM exception_group ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |row| {
            let entries_json: String = row.get(3)?;
            let entries: Vec<DateSpec> =
                serde_json::from_str(&entries_json).unwrap_or_default();
            Ok(ExceptionGroup {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                entries,
                created_ms: row.get(4)?,
                updated_ms: row.get(5)?,
            })
        })
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn add_exception_db(
    conn: &rusqlite::Connection,
    schedule_id: ScheduleId,
    group_id: Option<ExceptionGroupId>,
    name: &str,
    date_spec: &DateSpec,
    slots: &DaySlots,
    use_default: bool,
) -> Result<i64, ScheduleError> {
    let ts = now_ms();
    let date_spec_json =
        serde_json::to_string(date_spec).map_err(|e| ScheduleError::Db(e.to_string()))?;
    let slots_json =
        serde_json::to_string(slots).map_err(|e| ScheduleError::Db(e.to_string()))?;
    conn.execute(
        "INSERT INTO schedule_exception (schedule_id, group_id, name, date_spec_json, slots_json, use_default, created_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            schedule_id,
            group_id,
            name,
            date_spec_json,
            slots_json,
            use_default as i32,
            ts,
        ],
    )
    .map_err(|e| ScheduleError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

fn remove_exception_db(conn: &rusqlite::Connection, id: i64) -> Result<(), ScheduleError> {
    let rows = conn
        .execute(
            "DELETE FROM schedule_exception WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn parse_exception_row(row: &rusqlite::Row) -> rusqlite::Result<ScheduleException> {
    let date_spec_json: String = row.get(4)?;
    let slots_json: String = row.get(5)?;
    Ok(ScheduleException {
        id: row.get(0)?,
        schedule_id: row.get(1)?,
        group_id: row.get(2)?,
        name: row.get(3)?,
        date_spec: serde_json::from_str(&date_spec_json)
            .unwrap_or(DateSpec::Fixed { month: 1, day: 1 }),
        slots: serde_json::from_str(&slots_json).unwrap_or_default(),
        use_default: row.get::<_, i32>(6)? != 0,
        created_ms: row.get(7)?,
    })
}

fn list_exceptions_db(conn: &rusqlite::Connection, schedule_id: ScheduleId) -> Vec<ScheduleException> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, schedule_id, group_id, name, date_spec_json, slots_json, use_default, created_ms
             FROM schedule_exception WHERE schedule_id = ?1 ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![schedule_id], parse_exception_row)
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn list_all_exceptions_db(conn: &rusqlite::Connection) -> Vec<ScheduleException> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, schedule_id, group_id, name, date_spec_json, slots_json, use_default, created_ms
             FROM schedule_exception ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], parse_exception_row)
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn create_assignment_db(
    conn: &rusqlite::Connection,
    schedule_id: ScheduleId,
    device_id: &str,
    point_id: &str,
    priority: i32,
) -> Result<AssignmentId, ScheduleError> {
    let ts = now_ms();
    conn.execute(
        "INSERT INTO schedule_assignment (schedule_id, device_id, point_id, priority, enabled, created_ms)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)",
        rusqlite::params![schedule_id, device_id, point_id, priority, ts],
    )
    .map_err(|e| ScheduleError::Db(e.to_string()))?;
    Ok(conn.last_insert_rowid())
}

fn create_assignments_batch_db(
    conn: &rusqlite::Connection,
    schedule_id: ScheduleId,
    entries: &[(String, String)],
    priority: i32,
) -> Result<Vec<AssignmentId>, ScheduleError> {
    let ts = now_ms();
    let tx = conn
        .unchecked_transaction()
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    let mut ids = Vec::with_capacity(entries.len());
    {
        let mut stmt = tx
            .prepare_cached(
                "INSERT OR IGNORE INTO schedule_assignment (schedule_id, device_id, point_id, priority, enabled, created_ms)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            )
            .map_err(|e| ScheduleError::Db(e.to_string()))?;
        for (device_id, point_id) in entries {
            let rows = stmt
                .execute(rusqlite::params![schedule_id, device_id, point_id, priority, ts])
                .map_err(|e| ScheduleError::Db(e.to_string()))?;
            if rows > 0 {
                ids.push(tx.last_insert_rowid());
            }
        }
    }
    tx.commit().map_err(|e| ScheduleError::Db(e.to_string()))?;
    Ok(ids)
}

fn delete_assignment_db(
    conn: &rusqlite::Connection,
    id: AssignmentId,
) -> Result<(), ScheduleError> {
    let rows = conn
        .execute(
            "DELETE FROM schedule_assignment WHERE id = ?1",
            rusqlite::params![id],
        )
        .map_err(|e| ScheduleError::Db(e.to_string()))?;
    if rows == 0 {
        return Err(ScheduleError::NotFound);
    }
    Ok(())
}

fn parse_assignment_row(row: &rusqlite::Row) -> rusqlite::Result<ScheduleAssignment> {
    Ok(ScheduleAssignment {
        id: row.get(0)?,
        schedule_id: row.get(1)?,
        device_id: row.get(2)?,
        point_id: row.get(3)?,
        priority: row.get(4)?,
        enabled: row.get::<_, i32>(5)? != 0,
        created_ms: row.get(6)?,
    })
}

fn list_assignments_for_schedule_db(
    conn: &rusqlite::Connection,
    schedule_id: ScheduleId,
) -> Vec<ScheduleAssignment> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, schedule_id, device_id, point_id, priority, enabled, created_ms
             FROM schedule_assignment WHERE schedule_id = ?1 ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![schedule_id], parse_assignment_row)
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn get_assignments_for_point_db(
    conn: &rusqlite::Connection,
    device_id: &str,
    point_id: &str,
) -> Vec<ScheduleAssignment> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, schedule_id, device_id, point_id, priority, enabled, created_ms
             FROM schedule_assignment WHERE device_id = ?1 AND point_id = ?2 ORDER BY priority",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![device_id, point_id], parse_assignment_row)
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn list_all_assignments_db(conn: &rusqlite::Connection) -> Vec<ScheduleAssignment> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, schedule_id, device_id, point_id, priority, enabled, created_ms
             FROM schedule_assignment ORDER BY id",
        )
        .unwrap();
    let rows = stmt
        .query_map([], parse_assignment_row)
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

fn query_log_db(
    conn: &rusqlite::Connection,
    device_id: &str,
    point_id: &str,
    limit: i64,
) -> Vec<ScheduleLogEntry> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT id, assignment_id, device_id, point_id, value_json, reason, timestamp_ms
             FROM schedule_log WHERE device_id = ?1 AND point_id = ?2
             ORDER BY timestamp_ms DESC LIMIT ?3",
        )
        .unwrap();
    let rows = stmt
        .query_map(rusqlite::params![device_id, point_id, limit], |row| {
            Ok(ScheduleLogEntry {
                id: row.get(0)?,
                assignment_id: row.get(1)?,
                device_id: row.get(2)?,
                point_id: row.get(3)?,
                value_json: row.get(4)?,
                reason: row.get(5)?,
                timestamp_ms: row.get(6)?,
            })
        })
        .unwrap();
    rows.filter_map(|r| r.ok()).collect()
}

// ----------------------------------------------------------------
// Schedule evaluation helpers
// ----------------------------------------------------------------

/// Given a schedule's weekly slots, exceptions, and the current local time,
/// determine the active value for a point.
fn evaluate_point_value(
    schedule: &Schedule,
    exceptions: &[ScheduleException],
    now: &LocalTime,
) -> PointValue {
    // 1. Check if any exception matches today
    let effective_slots = get_effective_slots(schedule, exceptions, now);

    // 2. Find the active slot at the current time
    resolve_value_from_slots(&effective_slots, now, &schedule.default_value)
}

/// Get the effective day slots for today, considering exceptions.
fn get_effective_slots<'a>(
    schedule: &'a Schedule,
    exceptions: &'a [ScheduleException],
    now: &LocalTime,
) -> &'a DaySlots {
    // Check exceptions (later ones override earlier ones)
    for exc in exceptions.iter().rev() {
        if exc.use_default {
            // "use default" means use the schedule's default value for the whole day
            // We'll return empty slots so the default value applies
            if date_spec_matches_today(&exc.date_spec, now) {
                // Return a reference to empty slots — use_default means no slots active
                // Actually we need to check if it matches. If use_default, we still
                // want the "no slots" behavior which falls through to default_value.
                static EMPTY: DaySlots = DaySlots(Vec::new());
                return &EMPTY;
            }
        }
        if date_spec_matches_today(&exc.date_spec, now) {
            return &exc.slots;
        }
    }

    // No exception matches — use weekly schedule
    &schedule.weekly[now.weekday as usize]
}

/// Check if a DateSpec matches today.
fn date_spec_matches_today(spec: &DateSpec, now: &LocalTime) -> bool {
    match resolve_date_spec(spec, now.year) {
        Some((m, d)) => m == now.month && d == now.day,
        None => false,
    }
}

/// Given day slots and current time, find the active value.
/// Scans slots in reverse to find the most recent transition.
fn resolve_value_from_slots(
    slots: &DaySlots,
    now: &LocalTime,
    default_value: &PointValue,
) -> PointValue {
    let now_minutes = now.hour as u16 * 60 + now.minute as u16;

    // Find the last slot whose time is <= now
    let mut best: Option<&TimeSlot> = None;
    for slot in &slots.0 {
        if slot.time.total_minutes() <= now_minutes {
            best = Some(slot);
        } else {
            break; // slots are sorted ascending
        }
    }

    match best {
        Some(slot) => slot.value.clone(),
        None => default_value.clone(),
    }
}

// ----------------------------------------------------------------
// Schedule engine
// ----------------------------------------------------------------

/// Tracks the last value written by the engine for each point.
#[derive(Debug, Clone)]
struct LastWrite {
    assignment_id: AssignmentId,
    value: PointValue,
    #[allow(dead_code)]
    priority: i32,
}

async fn run_schedule_engine(store: PointStore, sched_store: ScheduleStore) {
    // Load initial data
    let mut data = sched_store.load_engine_data().await;
    let mut last_writes: HashMap<(String, String), LastWrite> = HashMap::new();

    // Startup recovery: evaluate all assignments immediately
    let now = local_time_now();
    evaluate_all_assignments(&data, &now, &store, &sched_store, &mut last_writes);

    let mut minute_ticker = tokio::time::interval(Duration::from_secs(60));
    minute_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut config_rx = sched_store.subscribe_config_changes();
    let mut last_day = now.day;

    loop {
        tokio::select! {
            _ = minute_ticker.tick() => {
                let now = local_time_now();
                if now.day != last_day {
                    // Day rollover — reload data to re-resolve exceptions
                    data = sched_store.load_engine_data().await;
                    last_day = now.day;
                }
                evaluate_all_assignments(&data, &now, &store, &sched_store, &mut last_writes);
            }
            Ok(_) = config_rx.changed() => {
                // Reload from DB, re-evaluate immediately
                data = sched_store.load_engine_data().await;
                let now = local_time_now();
                evaluate_all_assignments(&data, &now, &store, &sched_store, &mut last_writes);
            }
        }
    }
}

fn evaluate_all_assignments(
    data: &EngineData,
    now: &LocalTime,
    store: &PointStore,
    sched_store: &ScheduleStore,
    last_writes: &mut HashMap<(String, String), LastWrite>,
) {
    // Group assignments by point
    let mut point_assignments: HashMap<(String, String), Vec<(&ScheduleAssignment, &Schedule)>> =
        HashMap::new();

    for assignment in &data.assignments {
        if !assignment.enabled {
            continue;
        }
        // Find the schedule
        if let Some(schedule) = data.schedules.iter().find(|s| s.id == assignment.schedule_id) {
            if !schedule.enabled {
                continue;
            }
            let key = (assignment.device_id.clone(), assignment.point_id.clone());
            point_assignments
                .entry(key)
                .or_default()
                .push((assignment, schedule));
        }
    }

    for ((device_id, point_id), mut assignments) in point_assignments {
        // Sort by priority (lowest number = highest precedence)
        assignments.sort_by_key(|(a, _)| a.priority);

        // Evaluate the highest-priority assignment
        if let Some((assignment, schedule)) = assignments.first() {
            // Get exceptions for this schedule
            let exceptions: Vec<&ScheduleException> = data
                .exceptions
                .iter()
                .filter(|e| e.schedule_id == schedule.id)
                .collect();

            let exc_refs: Vec<ScheduleException> =
                exceptions.into_iter().cloned().collect();

            let value = evaluate_point_value(schedule, &exc_refs, now);

            let point_key = (device_id.clone(), point_id.clone());

            // Only write if value changed from last write
            let should_write = match last_writes.get(&point_key) {
                Some(lw) => lw.value != value || lw.assignment_id != assignment.id,
                None => true,
            };

            if should_write {
                let pk = PointKey {
                    device_instance_id: device_id.clone(),
                    point_id: point_id.clone(),
                };
                store.set(pk, value.clone());

                let value_json =
                    serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
                let reason = format!("schedule:{}", schedule.name);
                sched_store.insert_log(
                    assignment.id,
                    &device_id,
                    &point_id,
                    &value_json,
                    &reason,
                    now_ms(),
                );

                last_writes.insert(
                    point_key,
                    LastWrite {
                        assignment_id: assignment.id,
                        value,
                        priority: assignment.priority,
                    },
                );
            }
        }
    }
}

// ----------------------------------------------------------------
// Schedule templates
// ----------------------------------------------------------------

/// M-F 06:00-18:00 — standard office hours.
pub fn template_office_hours(on: PointValue, off: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..5 {
        // Mon-Fri
        weekly[day] = DaySlots(vec![
            TimeSlot {
                time: TimeOfDay::new(6, 0),
                value: on.clone(),
            },
            TimeSlot {
                time: TimeOfDay::new(18, 0),
                value: off.clone(),
            },
        ]);
    }
    weekly
}

/// M-F 05:00-22:00 — extended hours.
pub fn template_extended_hours(on: PointValue, off: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..5 {
        weekly[day] = DaySlots(vec![
            TimeSlot {
                time: TimeOfDay::new(5, 0),
                value: on.clone(),
            },
            TimeSlot {
                time: TimeOfDay::new(22, 0),
                value: off.clone(),
            },
        ]);
    }
    weekly
}

/// 24/7 — always on (every day starts with the "on" value at midnight).
pub fn template_24_7(on: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..7 {
        weekly[day] = DaySlots(vec![TimeSlot {
            time: TimeOfDay::new(0, 0),
            value: on.clone(),
        }]);
    }
    weekly
}

/// M-Sat 08:00-21:00, Sun 10:00-18:00 — retail hours.
pub fn template_retail(on: PointValue, off: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..6 {
        // Mon-Sat
        weekly[day] = DaySlots(vec![
            TimeSlot {
                time: TimeOfDay::new(8, 0),
                value: on.clone(),
            },
            TimeSlot {
                time: TimeOfDay::new(21, 0),
                value: off.clone(),
            },
        ]);
    }
    // Sunday
    weekly[6] = DaySlots(vec![
        TimeSlot {
            time: TimeOfDay::new(10, 0),
            value: on.clone(),
        },
        TimeSlot {
            time: TimeOfDay::new(18, 0),
            value: off.clone(),
        },
    ]);
    weekly
}

/// M-F 06:00-16:00 — school hours.
pub fn template_school(on: PointValue, off: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..5 {
        weekly[day] = DaySlots(vec![
            TimeSlot {
                time: TimeOfDay::new(6, 0),
                value: on.clone(),
            },
            TimeSlot {
                time: TimeOfDay::new(16, 0),
                value: off.clone(),
            },
        ]);
    }
    weekly
}

/// M-F 05:00-17:00 — warehouse hours.
pub fn template_warehouse(on: PointValue, off: PointValue) -> WeeklySchedule {
    let mut weekly = empty_weekly();
    for day in 0..5 {
        weekly[day] = DaySlots(vec![
            TimeSlot {
                time: TimeOfDay::new(5, 0),
                value: on.clone(),
            },
            TimeSlot {
                time: TimeOfDay::new(17, 0),
                value: off.clone(),
            },
        ]);
    }
    weekly
}

// ----------------------------------------------------------------
// Pre-built exception groups (holiday templates)
// ----------------------------------------------------------------

/// US Federal Holidays as DateSpec entries.
pub fn us_federal_holidays() -> Vec<DateSpec> {
    vec![
        // New Year's Day
        DateSpec::Fixed { month: 1, day: 1 },
        // MLK Day — 3rd Monday in January
        DateSpec::Relative {
            ordinal: Ordinal::Third,
            weekday: 0,
            month: 1,
        },
        // Presidents' Day — 3rd Monday in February
        DateSpec::Relative {
            ordinal: Ordinal::Third,
            weekday: 0,
            month: 2,
        },
        // Memorial Day — last Monday in May
        DateSpec::Relative {
            ordinal: Ordinal::Last,
            weekday: 0,
            month: 5,
        },
        // Juneteenth
        DateSpec::Fixed { month: 6, day: 19 },
        // Independence Day
        DateSpec::Fixed { month: 7, day: 4 },
        // Labor Day — 1st Monday in September
        DateSpec::Relative {
            ordinal: Ordinal::First,
            weekday: 0,
            month: 9,
        },
        // Columbus Day — 2nd Monday in October
        DateSpec::Relative {
            ordinal: Ordinal::Second,
            weekday: 0,
            month: 10,
        },
        // Veterans Day
        DateSpec::Fixed { month: 11, day: 11 },
        // Thanksgiving — 4th Thursday in November
        DateSpec::Relative {
            ordinal: Ordinal::Fourth,
            weekday: 3,
            month: 11,
        },
        // Christmas
        DateSpec::Fixed {
            month: 12,
            day: 25,
        },
    ]
}

/// UK Bank Holidays as DateSpec entries (approximation — some are fixed by proclamation).
pub fn uk_bank_holidays() -> Vec<DateSpec> {
    vec![
        // New Year's Day
        DateSpec::Fixed { month: 1, day: 1 },
        // Good Friday — not easily computed without Easter algorithm; skip for now
        // Easter Monday — same issue
        // Early May bank holiday — 1st Monday in May
        DateSpec::Relative {
            ordinal: Ordinal::First,
            weekday: 0,
            month: 5,
        },
        // Spring bank holiday — last Monday in May
        DateSpec::Relative {
            ordinal: Ordinal::Last,
            weekday: 0,
            month: 5,
        },
        // Summer bank holiday — last Monday in August
        DateSpec::Relative {
            ordinal: Ordinal::Last,
            weekday: 0,
            month: 8,
        },
        // Christmas Day
        DateSpec::Fixed {
            month: 12,
            day: 25,
        },
        // Boxing Day
        DateSpec::Fixed {
            month: 12,
            day: 26,
        },
    ]
}

// ----------------------------------------------------------------
// Public startup function
// ----------------------------------------------------------------

/// Start the schedule system. Returns a `ScheduleStore` handle.
pub fn start_schedule_engine(store: &PointStore) -> ScheduleStore {
    let db_dir = Path::new("data");
    if !db_dir.exists() {
        std::fs::create_dir_all(db_dir).expect("failed to create data directory");
    }
    start_schedule_engine_with_path(store, &db_dir.join("schedules.db"))
}

pub fn start_schedule_engine_with_path(store: &PointStore, db_path: &Path) -> ScheduleStore {
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (config_version_tx, config_version_rx) = watch::channel(0u64);

    let sched_store = ScheduleStore {
        cmd_tx,
        config_version_tx,
        config_version_rx,
        event_bus: None,
    };

    // Start SQLite thread
    let path_clone = db_path.to_path_buf();
    std::thread::Builder::new()
        .name("schedule-sqlite".into())
        .spawn(move || run_sqlite_thread(&path_clone, cmd_rx))
        .expect("failed to spawn schedule SQLite thread");

    // Start engine task
    let engine_store = store.clone();
    let engine_sched = sched_store.clone();
    tokio::spawn(async move {
        // Small delay to let SQLite thread initialize
        tokio::time::sleep(Duration::from_millis(100)).await;
        run_schedule_engine(engine_store, engine_sched).await;
    });

    sched_store
}

// ----------------------------------------------------------------
// Preview timeline (Phase 3a)
// ----------------------------------------------------------------

/// A block in the 7-day preview timeline.
#[derive(Debug, Clone)]
pub struct PreviewBlock {
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    pub value: PointValue,
    pub source: String,
}

/// Compute a 7-day preview starting from `start_date`.
/// Returns 7 vectors of blocks (one per day, starting from the given date).
pub fn compute_preview(
    schedule: &Schedule,
    exceptions: &[ScheduleException],
    start_year: i32,
    start_month: u8,
    start_day: u8,
) -> [Vec<PreviewBlock>; 7] {
    let mut result: [Vec<PreviewBlock>; 7] = Default::default();

    for day_offset in 0..7u8 {
        // Compute the actual date for this offset
        let (y, m, d) = add_days(start_year, start_month, start_day, day_offset as i32);
        let wd = weekday_of(y, m, d);

        let fake_now = LocalTime {
            year: y,
            month: m,
            day: d,
            weekday: wd,
            hour: 0,
            minute: 0,
        };

        // Determine which exception matches this day, if any
        let mut exc_match: Option<&ScheduleException> = None;
        for exc in exceptions.iter().rev() {
            if date_spec_matches_today(&exc.date_spec, &fake_now) {
                exc_match = Some(exc);
                break;
            }
        }

        let (slots, source) = if let Some(exc) = exc_match {
            if exc.use_default {
                (&DaySlots(Vec::new()) as *const DaySlots, format!("exception:{}", exc.name))
            } else {
                (&exc.slots as *const DaySlots, format!("exception:{}", exc.name))
            }
        } else {
            (
                &schedule.weekly[wd as usize] as *const DaySlots,
                format!("weekly:{}", day_label(wd)),
            )
        };

        // SAFETY: slots pointer is valid for the duration of this iteration
        let slots_ref = unsafe { &*slots };

        let day_blocks = build_day_blocks(slots_ref, &schedule.default_value, &source);
        result[day_offset as usize] = day_blocks;
    }

    result
}

fn build_day_blocks(slots: &DaySlots, default_value: &PointValue, source: &str) -> Vec<PreviewBlock> {
    if slots.0.is_empty() {
        // Whole day is default value
        return vec![PreviewBlock {
            start: TimeOfDay::new(0, 0),
            end: TimeOfDay::new(23, 59),
            value: default_value.clone(),
            source: source.to_string(),
        }];
    }

    let mut blocks = Vec::new();

    // If first slot doesn't start at midnight, add a default block
    if slots.0[0].time.total_minutes() > 0 {
        blocks.push(PreviewBlock {
            start: TimeOfDay::new(0, 0),
            end: TimeOfDay {
                hour: slots.0[0].time.hour,
                minute: if slots.0[0].time.minute > 0 {
                    slots.0[0].time.minute - 1
                } else {
                    59
                },
            },
            value: default_value.clone(),
            source: source.to_string(),
        });
    }

    for (i, slot) in slots.0.iter().enumerate() {
        let end = if i + 1 < slots.0.len() {
            let next = &slots.0[i + 1].time;
            TimeOfDay {
                hour: if next.minute > 0 {
                    next.hour
                } else if next.hour > 0 {
                    next.hour - 1
                } else {
                    0
                },
                minute: if next.minute > 0 {
                    next.minute - 1
                } else {
                    59
                },
            }
        } else {
            TimeOfDay::new(23, 59)
        };

        blocks.push(PreviewBlock {
            start: slot.time,
            end,
            value: slot.value.clone(),
            source: source.to_string(),
        });
    }

    blocks
}

fn day_label(weekday: u8) -> &'static str {
    match weekday {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        6 => "Sunday",
        _ => "Unknown",
    }
}

fn add_days(year: i32, month: u8, day: u8, offset: i32) -> (i32, u8, u8) {
    let mut tm = Tm::default();
    tm.tm_year = year - 1900;
    tm.tm_mon = month as i32 - 1;
    tm.tm_mday = day as i32 + offset;
    tm.tm_hour = 12;
    unsafe { mktime(&mut tm) };
    (
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
    )
}

// ----------------------------------------------------------------
// Conflict detection (Phase 3b)
// ----------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ScheduleConflict {
    pub device_id: String,
    pub point_id: String,
    pub assignments: Vec<ScheduleAssignment>,
}

impl ScheduleStore {
    /// Find points with multiple schedule assignments (potential conflicts).
    pub async fn get_conflicts(&self) -> Vec<ScheduleConflict> {
        // Load all assignments
        let data = self.load_engine_data().await;
        let mut by_point: HashMap<(String, String), Vec<ScheduleAssignment>> = HashMap::new();
        for a in data.assignments {
            if a.enabled {
                by_point
                    .entry((a.device_id.clone(), a.point_id.clone()))
                    .or_default()
                    .push(a);
            }
        }

        by_point
            .into_iter()
            .filter(|(_, v)| v.len() > 1)
            .map(|((device_id, point_id), assignments)| ScheduleConflict {
                device_id,
                point_id,
                assignments,
            })
            .collect()
    }
}

// ----------------------------------------------------------------
// Tests
// ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::profile::PointValue;
    use std::sync::atomic::{AtomicU32, Ordering};

    static TEST_COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_db_path() -> std::path::PathBuf {
        let n = TEST_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join("opencrate-schedule-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("schedule-test-{n}.db"))
    }

    #[tokio::test]
    async fn schedule_crud() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        // Create
        let weekly = template_office_hours(PointValue::Bool(true), PointValue::Bool(false));
        let id = sched_store
            .create_schedule(
                "Office Hours",
                "Standard M-F schedule",
                ScheduleValueType::Binary,
                PointValue::Bool(false),
                weekly.clone(),
            )
            .await
            .unwrap();
        assert!(id > 0);

        // List
        let schedules = sched_store.list_schedules().await;
        assert_eq!(schedules.len(), 1);
        assert_eq!(schedules[0].name, "Office Hours");
        assert_eq!(schedules[0].value_type, ScheduleValueType::Binary);

        // Get
        let sched = sched_store.get_schedule(id).await.unwrap();
        assert_eq!(sched.name, "Office Hours");

        // Update
        sched_store
            .update_schedule(
                id,
                "Updated Hours",
                "Changed name",
                PointValue::Bool(false),
                true,
                weekly,
            )
            .await
            .unwrap();
        let sched = sched_store.get_schedule(id).await.unwrap();
        assert_eq!(sched.name, "Updated Hours");

        // Delete
        sched_store.delete_schedule(id).await.unwrap();
        let schedules = sched_store.list_schedules().await;
        assert!(schedules.is_empty());

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn weekly_json_roundtrip() {
        let weekly = template_office_hours(PointValue::Bool(true), PointValue::Bool(false));
        let json = serde_json::to_string(&weekly).unwrap();
        let parsed: WeeklySchedule = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0].0.len(), 2); // Monday has 2 slots
        assert_eq!(parsed[5].0.len(), 0); // Saturday has 0 slots
    }

    #[test]
    fn time_of_day_ordering() {
        let t1 = TimeOfDay::new(6, 0);
        let t2 = TimeOfDay::new(18, 0);
        let t3 = TimeOfDay::new(6, 30);
        assert!(t1 < t2);
        assert!(t1 < t3);
        assert_eq!(t1.total_minutes(), 360);
        assert_eq!(t2.total_minutes(), 1080);
    }

    #[test]
    fn resolve_thanksgiving_2026() {
        // 4th Thursday in November 2026
        let spec = DateSpec::Relative {
            ordinal: Ordinal::Fourth,
            weekday: 3, // Thursday = 3 (0=Mon)
            month: 11,
        };
        let result = resolve_date_spec(&spec, 2026);
        assert_eq!(result, Some((11, 26))); // Nov 26, 2026
    }

    #[test]
    fn resolve_memorial_day_2026() {
        // Last Monday in May 2026
        let spec = DateSpec::Relative {
            ordinal: Ordinal::Last,
            weekday: 0, // Monday = 0
            month: 5,
        };
        let result = resolve_date_spec(&spec, 2026);
        assert_eq!(result, Some((5, 25))); // May 25, 2026
    }

    #[test]
    fn resolve_christmas() {
        let spec = DateSpec::Fixed {
            month: 12,
            day: 25,
        };
        assert_eq!(resolve_date_spec(&spec, 2026), Some((12, 25)));
        assert_eq!(resolve_date_spec(&spec, 2030), Some((12, 25)));
    }

    #[test]
    fn fixed_year_only_matches_its_year() {
        let spec = DateSpec::FixedYear {
            year: 2026,
            month: 4,
            day: 18,
        };
        assert_eq!(resolve_date_spec(&spec, 2026), Some((4, 18)));
        assert_eq!(resolve_date_spec(&spec, 2027), None);
    }

    #[test]
    fn evaluate_point_weekly() {
        let schedule = Schedule {
            id: 1,
            name: "Test".to_string(),
            description: String::new(),
            value_type: ScheduleValueType::Binary,
            default_value: PointValue::Bool(false),
            enabled: true,
            weekly: template_office_hours(PointValue::Bool(true), PointValue::Bool(false)),
            created_ms: 0,
            updated_ms: 0,
        };

        // Monday at 10:00 → occupied (true)
        let now = LocalTime {
            year: 2026,
            month: 3,
            day: 9, // Monday
            weekday: 0,
            hour: 10,
            minute: 0,
        };
        assert_eq!(
            evaluate_point_value(&schedule, &[], &now),
            PointValue::Bool(true)
        );

        // Monday at 20:00 → unoccupied (false)
        let now = LocalTime {
            year: 2026,
            month: 3,
            day: 9,
            weekday: 0,
            hour: 20,
            minute: 0,
        };
        assert_eq!(
            evaluate_point_value(&schedule, &[], &now),
            PointValue::Bool(false)
        );

        // Monday at 05:00 → before first slot → default (false)
        let now = LocalTime {
            year: 2026,
            month: 3,
            day: 9,
            weekday: 0,
            hour: 5,
            minute: 0,
        };
        assert_eq!(
            evaluate_point_value(&schedule, &[], &now),
            PointValue::Bool(false)
        );

        // Saturday → no slots → default (false)
        let now = LocalTime {
            year: 2026,
            month: 3,
            day: 14,
            weekday: 5,
            hour: 10,
            minute: 0,
        };
        assert_eq!(
            evaluate_point_value(&schedule, &[], &now),
            PointValue::Bool(false)
        );
    }

    #[test]
    fn exception_overrides_weekly() {
        let schedule = Schedule {
            id: 1,
            name: "Test".to_string(),
            description: String::new(),
            value_type: ScheduleValueType::Binary,
            default_value: PointValue::Bool(false),
            enabled: true,
            weekly: template_office_hours(PointValue::Bool(true), PointValue::Bool(false)),
            created_ms: 0,
            updated_ms: 0,
        };

        // Holiday exception: Christmas = no slots (use default)
        let exception = ScheduleException {
            id: 1,
            schedule_id: 1,
            group_id: None,
            name: "Christmas".to_string(),
            date_spec: DateSpec::Fixed {
                month: 12,
                day: 25,
            },
            slots: DaySlots::default(),
            use_default: true,
            created_ms: 0,
        };

        // Dec 25 (Thursday) at 10:00 — would normally be occupied, but exception overrides
        let now = LocalTime {
            year: 2025,
            month: 12,
            day: 25,
            weekday: 3,
            hour: 10,
            minute: 0,
        };
        assert_eq!(
            evaluate_point_value(&schedule, &[exception], &now),
            PointValue::Bool(false) // default
        );
    }

    #[tokio::test]
    async fn assignment_crud() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        let weekly = template_office_hours(PointValue::Bool(true), PointValue::Bool(false));
        let sched_id = sched_store
            .create_schedule(
                "Office",
                "",
                ScheduleValueType::Binary,
                PointValue::Bool(false),
                weekly,
            )
            .await
            .unwrap();

        // Create assignment
        let assign_id = sched_store
            .create_assignment(sched_id, "ahu-1", "occ_mode", 12)
            .await
            .unwrap();
        assert!(assign_id > 0);

        // List for schedule
        let assigns = sched_store.list_assignments_for_schedule(sched_id).await;
        assert_eq!(assigns.len(), 1);
        assert_eq!(assigns[0].device_id, "ahu-1");

        // Get for point
        let assigns = sched_store
            .get_assignments_for_point("ahu-1", "occ_mode")
            .await;
        assert_eq!(assigns.len(), 1);

        // Delete
        sched_store.delete_assignment(assign_id).await.unwrap();
        let assigns = sched_store.list_assignments_for_schedule(sched_id).await;
        assert!(assigns.is_empty());

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn priority_resolution() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        store.set(
            PointKey {
                device_instance_id: "ahu-1".into(),
                point_id: "occ_mode".into(),
            },
            PointValue::Bool(false),
        );
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        // Schedule A: priority 10, value = true (always on)
        let weekly_a = template_24_7(PointValue::Bool(true));
        let id_a = sched_store
            .create_schedule(
                "Always On",
                "",
                ScheduleValueType::Binary,
                PointValue::Bool(true),
                weekly_a,
            )
            .await
            .unwrap();
        sched_store
            .create_assignment(id_a, "ahu-1", "occ_mode", 10)
            .await
            .unwrap();

        // Schedule B: priority 14, value = false (always off)
        let weekly_b = template_24_7(PointValue::Bool(false));
        let id_b = sched_store
            .create_schedule(
                "Always Off",
                "",
                ScheduleValueType::Binary,
                PointValue::Bool(false),
                weekly_b,
            )
            .await
            .unwrap();
        sched_store
            .create_assignment(id_b, "ahu-1", "occ_mode", 14)
            .await
            .unwrap();

        // Wait for engine to evaluate
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Priority 10 (Always On) should win
        let val = store.get(&PointKey {
            device_instance_id: "ahu-1".into(),
            point_id: "occ_mode".into(),
        });
        assert_eq!(val.unwrap().value, PointValue::Bool(true));

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn engine_writes_on_startup() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        store.set(
            PointKey {
                device_instance_id: "ahu-1".into(),
                point_id: "occ_mode".into(),
            },
            PointValue::Integer(0),
        );

        // Create schedule with 24/7 value
        let sched_store = start_schedule_engine_with_path(&store, &db_path);
        let weekly = template_24_7(PointValue::Integer(1));
        let id = sched_store
            .create_schedule(
                "24/7 Occupied",
                "",
                ScheduleValueType::Multistate,
                PointValue::Integer(0),
                weekly,
            )
            .await
            .unwrap();
        sched_store
            .create_assignment(id, "ahu-1", "occ_mode", 12)
            .await
            .unwrap();

        // Wait for engine startup recovery
        tokio::time::sleep(Duration::from_millis(500)).await;

        let val = store.get(&PointKey {
            device_instance_id: "ahu-1".into(),
            point_id: "occ_mode".into(),
        });
        assert_eq!(val.unwrap().value, PointValue::Integer(1));

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn exception_group_crud() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        let entries = us_federal_holidays();
        let id = sched_store
            .create_exception_group("US Federal Holidays", "Standard US holidays", entries.clone())
            .await
            .unwrap();
        assert!(id > 0);

        let groups = sched_store.list_exception_groups().await;
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].name, "US Federal Holidays");
        assert_eq!(groups[0].entries.len(), entries.len());

        sched_store.delete_exception_group(id).await.unwrap();
        let groups = sched_store.list_exception_groups().await;
        assert!(groups.is_empty());

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn schedule_log_entries() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        store.set(
            PointKey {
                device_instance_id: "ahu-1".into(),
                point_id: "occ_mode".into(),
            },
            PointValue::Bool(false),
        );
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        let weekly = template_24_7(PointValue::Bool(true));
        let id = sched_store
            .create_schedule(
                "Test",
                "",
                ScheduleValueType::Binary,
                PointValue::Bool(false),
                weekly,
            )
            .await
            .unwrap();
        sched_store
            .create_assignment(id, "ahu-1", "occ_mode", 12)
            .await
            .unwrap();

        // Wait for engine to write
        tokio::time::sleep(Duration::from_millis(500)).await;

        let logs = sched_store
            .query_log("ahu-1", "occ_mode", 10)
            .await;
        assert!(!logs.is_empty(), "should have log entries");
        assert!(logs[0].reason.contains("schedule:"));

        let _ = std::fs::remove_file(&db_path);
    }

    #[tokio::test]
    async fn batch_assignment_create() {
        let db_path = temp_db_path();
        let store = PointStore::new();
        let sched_store = start_schedule_engine_with_path(&store, &db_path);

        let weekly = template_office_hours(PointValue::Bool(true), PointValue::Bool(false));
        let sched_id = sched_store
            .create_schedule(
                "Office",
                "",
                ScheduleValueType::Binary,
                PointValue::Bool(false),
                weekly,
            )
            .await
            .unwrap();

        let entries = vec![
            ("ahu-1".to_string(), "occ_mode".to_string()),
            ("ahu-2".to_string(), "occ_mode".to_string()),
            ("vav-1".to_string(), "occ_mode".to_string()),
        ];
        let ids = sched_store
            .create_assignments_batch(sched_id, &entries, 12)
            .await
            .unwrap();
        assert_eq!(ids.len(), 3);

        let assigns = sched_store.list_assignments_for_schedule(sched_id).await;
        assert_eq!(assigns.len(), 3);

        let _ = std::fs::remove_file(&db_path);
    }

    #[test]
    fn template_schedules_structure() {
        let on = PointValue::Bool(true);
        let off = PointValue::Bool(false);

        let office = template_office_hours(on.clone(), off.clone());
        assert_eq!(office[0].0.len(), 2); // Monday: 2 slots
        assert_eq!(office[5].0.len(), 0); // Saturday: empty

        let h24 = template_24_7(on.clone());
        assert_eq!(h24[0].0.len(), 1); // Every day: 1 slot at midnight
        assert_eq!(h24[6].0.len(), 1);

        let retail = template_retail(on.clone(), off.clone());
        assert_eq!(retail[5].0.len(), 2); // Saturday: 2 slots
        assert_eq!(retail[6].0.len(), 2); // Sunday: 2 slots (different hours)
    }

    #[test]
    fn date_spec_serde_roundtrip() {
        let specs = vec![
            DateSpec::Fixed { month: 1, day: 1 },
            DateSpec::FixedYear {
                year: 2026,
                month: 4,
                day: 18,
            },
            DateSpec::Relative {
                ordinal: Ordinal::Fourth,
                weekday: 3,
                month: 11,
            },
        ];
        let json = serde_json::to_string(&specs).unwrap();
        let parsed: Vec<DateSpec> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 3);
    }

    #[test]
    fn preview_basic() {
        let schedule = Schedule {
            id: 1,
            name: "Test".to_string(),
            description: String::new(),
            value_type: ScheduleValueType::Binary,
            default_value: PointValue::Bool(false),
            enabled: true,
            weekly: template_office_hours(PointValue::Bool(true), PointValue::Bool(false)),
            created_ms: 0,
            updated_ms: 0,
        };

        // Start from Monday March 9 2026
        let preview = compute_preview(&schedule, &[], 2026, 3, 9);
        // Monday should have 3 blocks: default (00:00-05:59), on (06:00-17:59), off (18:00-23:59)
        assert_eq!(preview[0].len(), 3);
        // Saturday (index 5) should have 1 block (all default)
        assert_eq!(preview[5].len(), 1);
    }
}
