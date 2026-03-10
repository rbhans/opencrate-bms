use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{mpsc, oneshot, watch};

use crate::discovery::model::{
    ConnStatus, DeviceState, DiscoveredDevice, DiscoveredPoint, DiscoveryProtocol, PointKindHint,
};
use crate::event::bus::{Event, EventBus};

// ----------------------------------------------------------------
// Error type
// ----------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("database error: {0}")]
    Db(String),
    #[error("channel closed")]
    ChannelClosed,
    #[error("not found")]
    NotFound,
}

// ----------------------------------------------------------------
// Commands
// ----------------------------------------------------------------

enum DiscoveryCmd {
    UpsertDevice {
        device: DiscoveredDevice,
        reply: oneshot::Sender<Result<(), DiscoveryError>>,
    },
    UpsertPoints {
        device_id: String,
        points: Vec<DiscoveredPoint>,
        reply: oneshot::Sender<Result<(), DiscoveryError>>,
    },
    ListDevices {
        state_filter: Option<DeviceState>,
        reply: oneshot::Sender<Vec<DiscoveredDevice>>,
    },
    GetDevice {
        id: String,
        reply: oneshot::Sender<Result<DiscoveredDevice, DiscoveryError>>,
    },
    GetPoints {
        device_id: String,
        reply: oneshot::Sender<Vec<DiscoveredPoint>>,
    },
    SetDeviceState {
        id: String,
        state: DeviceState,
        reply: oneshot::Sender<Result<(), DiscoveryError>>,
    },
    SetConnStatus {
        id: String,
        status: ConnStatus,
        reply: oneshot::Sender<Result<(), DiscoveryError>>,
    },
    RecordScan {
        protocol: String,
        reply: oneshot::Sender<i64>,
    },
    FinishScan {
        scan_id: i64,
        device_count: usize,
        reply: oneshot::Sender<()>,
    },
}

// ----------------------------------------------------------------
// DiscoveryStore handle
// ----------------------------------------------------------------

#[derive(Clone)]
pub struct DiscoveryStore {
    cmd_tx: mpsc::UnboundedSender<DiscoveryCmd>,
    version_tx: watch::Sender<u64>,
    version_rx: watch::Receiver<u64>,
    event_bus: Option<EventBus>,
}

impl DiscoveryStore {
    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    pub fn subscribe(&self) -> watch::Receiver<u64> {
        self.version_rx.clone()
    }

    fn bump_version(&self) {
        let v = *self.version_tx.borrow() + 1;
        let _ = self.version_tx.send(v);
    }

    pub async fn upsert_device(&self, device: DiscoveredDevice) -> Result<(), DiscoveryError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(DiscoveryCmd::UpsertDevice {
                device,
                reply: reply_tx,
            })
            .map_err(|_| DiscoveryError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| DiscoveryError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_version();
        }
        result
    }

    pub async fn upsert_points(
        &self,
        device_id: &str,
        points: Vec<DiscoveredPoint>,
    ) -> Result<(), DiscoveryError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(DiscoveryCmd::UpsertPoints {
                device_id: device_id.to_string(),
                points,
                reply: reply_tx,
            })
            .map_err(|_| DiscoveryError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| DiscoveryError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_version();
        }
        result
    }

    pub async fn list_devices(
        &self,
        state_filter: Option<DeviceState>,
    ) -> Vec<DiscoveredDevice> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(DiscoveryCmd::ListDevices {
            state_filter,
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn get_device(&self, id: &str) -> Result<DiscoveredDevice, DiscoveryError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(DiscoveryCmd::GetDevice {
                id: id.to_string(),
                reply: reply_tx,
            })
            .map_err(|_| DiscoveryError::ChannelClosed)?;
        reply_rx.await.map_err(|_| DiscoveryError::ChannelClosed)?
    }

    pub async fn get_points(&self, device_id: &str) -> Vec<DiscoveredPoint> {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(DiscoveryCmd::GetPoints {
            device_id: device_id.to_string(),
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or_default()
    }

    pub async fn set_device_state(
        &self,
        id: &str,
        state: DeviceState,
    ) -> Result<(), DiscoveryError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(DiscoveryCmd::SetDeviceState {
                id: id.to_string(),
                state,
                reply: reply_tx,
            })
            .map_err(|_| DiscoveryError::ChannelClosed)?;
        let result = reply_rx.await.map_err(|_| DiscoveryError::ChannelClosed)?;
        if result.is_ok() {
            self.bump_version();
        }
        result
    }

    pub async fn set_conn_status(
        &self,
        id: &str,
        status: ConnStatus,
    ) -> Result<(), DiscoveryError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.cmd_tx
            .send(DiscoveryCmd::SetConnStatus {
                id: id.to_string(),
                status,
                reply: reply_tx,
            })
            .map_err(|_| DiscoveryError::ChannelClosed)?;
        reply_rx.await.map_err(|_| DiscoveryError::ChannelClosed)?
    }

    pub async fn record_scan(&self, protocol: &str) -> i64 {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(DiscoveryCmd::RecordScan {
            protocol: protocol.to_string(),
            reply: reply_tx,
        });
        reply_rx.await.unwrap_or(0)
    }

    pub async fn finish_scan(&self, scan_id: i64, device_count: usize) {
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.cmd_tx.send(DiscoveryCmd::FinishScan {
            scan_id,
            device_count,
            reply: reply_tx,
        });
        let _ = reply_rx.await;
    }
}

// ----------------------------------------------------------------
// Schema
// ----------------------------------------------------------------

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS discovered_device (
    id TEXT PRIMARY KEY,
    protocol TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'discovered',
    conn_status TEXT NOT NULL DEFAULT 'unknown',
    display_name TEXT NOT NULL,
    vendor TEXT,
    model TEXT,
    address TEXT NOT NULL,
    point_count INTEGER NOT NULL DEFAULT 0,
    discovered_at INTEGER NOT NULL,
    accepted_at INTEGER,
    protocol_meta TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS discovered_point (
    id TEXT NOT NULL,
    device_id TEXT NOT NULL REFERENCES discovered_device(id) ON DELETE CASCADE,
    display_name TEXT NOT NULL,
    description TEXT,
    units TEXT,
    point_kind TEXT NOT NULL DEFAULT 'analog',
    writable INTEGER NOT NULL DEFAULT 0,
    binding_json TEXT NOT NULL,
    protocol_meta TEXT NOT NULL DEFAULT '{}',
    PRIMARY KEY (device_id, id)
);

CREATE TABLE IF NOT EXISTS discovery_scan (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    protocol TEXT NOT NULL,
    started_at INTEGER NOT NULL,
    ended_at INTEGER,
    device_count INTEGER DEFAULT 0,
    status TEXT DEFAULT 'running'
);
";

// ----------------------------------------------------------------
// Start functions
// ----------------------------------------------------------------

pub fn start_discovery_store() -> DiscoveryStore {
    start_discovery_store_with_path(&PathBuf::from("data/discovery.db"))
}

pub fn start_discovery_store_with_path(db_path: &Path) -> DiscoveryStore {
    let db_dir = db_path.parent().unwrap_or(Path::new("."));
    if !db_dir.exists() {
        std::fs::create_dir_all(db_dir).expect("failed to create data directory");
    }

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (version_tx, version_rx) = watch::channel(0u64);

    let path_clone = db_path.to_path_buf();
    std::thread::Builder::new()
        .name("discovery-sqlite".into())
        .spawn(move || run_sqlite_thread(&path_clone, cmd_rx))
        .expect("failed to spawn discovery SQLite thread");

    DiscoveryStore {
        cmd_tx,
        version_tx,
        version_rx,
        event_bus: None,
    }
}

/// Spawn a background task that subscribes to the EventBus and updates
/// device connectivity status in the DiscoveryStore when DeviceDown or
/// DeviceDiscovered events are received.
pub fn start_conn_status_listener(store: DiscoveryStore, bus: EventBus) {
    tokio::spawn(async move {
        let mut rx = bus.subscribe();
        loop {
            match rx.recv().await {
                Ok(event) => match event.as_ref() {
                    Event::DeviceDown { device_key, .. } => {
                        let _ = store.set_conn_status(device_key, ConnStatus::Offline).await;
                    }
                    Event::DeviceDiscovered { device_key, .. } => {
                        let _ = store.set_conn_status(device_key, ConnStatus::Online).await;
                    }
                    _ => {}
                },
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    eprintln!("discovery conn_status listener lagged by {n} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

// ----------------------------------------------------------------
// SQLite thread
// ----------------------------------------------------------------

fn run_sqlite_thread(db_path: &Path, mut cmd_rx: mpsc::UnboundedReceiver<DiscoveryCmd>) {
    let conn =
        rusqlite::Connection::open(db_path).expect("failed to open discovery database");
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;",
    )
    .expect("failed to set pragmas");
    conn.execute_batch(SCHEMA)
        .expect("failed to create discovery schema");

    while let Some(cmd) = cmd_rx.blocking_recv() {
        match cmd {
            DiscoveryCmd::UpsertDevice { device, reply } => {
                let result = upsert_device_db(&conn, &device);
                let _ = reply.send(result);
            }
            DiscoveryCmd::UpsertPoints {
                device_id,
                points,
                reply,
            } => {
                let result = upsert_points_db(&conn, &device_id, &points);
                let _ = reply.send(result);
            }
            DiscoveryCmd::ListDevices {
                state_filter,
                reply,
            } => {
                let result = list_devices_db(&conn, state_filter);
                let _ = reply.send(result);
            }
            DiscoveryCmd::GetDevice { id, reply } => {
                let result = get_device_db(&conn, &id);
                let _ = reply.send(result);
            }
            DiscoveryCmd::GetPoints { device_id, reply } => {
                let result = get_points_db(&conn, &device_id);
                let _ = reply.send(result);
            }
            DiscoveryCmd::SetDeviceState { id, state, reply } => {
                let result = set_device_state_db(&conn, &id, state);
                let _ = reply.send(result);
            }
            DiscoveryCmd::SetConnStatus { id, status, reply } => {
                let result = set_conn_status_db(&conn, &id, status);
                let _ = reply.send(result);
            }
            DiscoveryCmd::RecordScan { protocol, reply } => {
                let scan_id = record_scan_db(&conn, &protocol);
                let _ = reply.send(scan_id);
            }
            DiscoveryCmd::FinishScan {
                scan_id,
                device_count,
                reply,
            } => {
                finish_scan_db(&conn, scan_id, device_count);
                let _ = reply.send(());
            }
        }
    }
}

// ----------------------------------------------------------------
// DB helpers
// ----------------------------------------------------------------

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn upsert_device_db(
    conn: &rusqlite::Connection,
    device: &DiscoveredDevice,
) -> Result<(), DiscoveryError> {
    let meta_str =
        serde_json::to_string(&device.protocol_meta).unwrap_or_else(|_| "{}".into());

    // On re-discovery, preserve state and accepted_at — only update connectivity and metadata
    conn.execute(
        "INSERT INTO discovered_device (id, protocol, state, conn_status, display_name, vendor, model, address, point_count, discovered_at, accepted_at, protocol_meta)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
         ON CONFLICT(id) DO UPDATE SET
             conn_status = excluded.conn_status,
             display_name = excluded.display_name,
             vendor = COALESCE(excluded.vendor, discovered_device.vendor),
             model = COALESCE(excluded.model, discovered_device.model),
             address = excluded.address,
             point_count = excluded.point_count,
             protocol_meta = excluded.protocol_meta",
        rusqlite::params![
            device.id,
            device.protocol.as_str(),
            device.state.as_str(),
            device.conn_status.as_str(),
            device.display_name,
            device.vendor,
            device.model,
            device.address,
            device.point_count as i64,
            device.discovered_at_ms,
            device.accepted_at_ms,
            meta_str,
        ],
    )
    .map_err(|e| DiscoveryError::Db(e.to_string()))?;

    Ok(())
}

fn upsert_points_db(
    conn: &rusqlite::Connection,
    device_id: &str,
    points: &[DiscoveredPoint],
) -> Result<(), DiscoveryError> {
    // Delete existing points for this device, then re-insert
    conn.execute(
        "DELETE FROM discovered_point WHERE device_id = ?1",
        rusqlite::params![device_id],
    )
    .map_err(|e| DiscoveryError::Db(e.to_string()))?;

    for pt in points {
        let binding_json =
            serde_json::to_string(&pt.binding).unwrap_or_else(|_| "{}".into());
        let meta_str =
            serde_json::to_string(&pt.protocol_meta).unwrap_or_else(|_| "{}".into());

        conn.execute(
            "INSERT INTO discovered_point (id, device_id, display_name, description, units, point_kind, writable, binding_json, protocol_meta)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            rusqlite::params![
                pt.id,
                device_id,
                pt.display_name,
                pt.description,
                pt.units,
                pt.point_kind.as_str(),
                pt.writable as i32,
                binding_json,
                meta_str,
            ],
        )
        .map_err(|e| DiscoveryError::Db(e.to_string()))?;
    }

    Ok(())
}

fn list_devices_db(
    conn: &rusqlite::Connection,
    state_filter: Option<DeviceState>,
) -> Vec<DiscoveredDevice> {
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match state_filter {
        Some(state) => (
            "SELECT id, protocol, state, conn_status, display_name, vendor, model, address, point_count, discovered_at, accepted_at, protocol_meta FROM discovered_device WHERE state = ?1 ORDER BY display_name".into(),
            vec![Box::new(state.as_str().to_string())],
        ),
        None => (
            "SELECT id, protocol, state, conn_status, display_name, vendor, model, address, point_count, discovered_at, accepted_at, protocol_meta FROM discovered_device ORDER BY display_name".into(),
            vec![],
        ),
    };

    let param_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return vec![],
    };
    let rows = match stmt.query_map(param_refs.as_slice(), |row| Ok(row_to_device(row))) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    rows.filter_map(|r| r.ok()).collect()
}

fn get_device_db(
    conn: &rusqlite::Connection,
    id: &str,
) -> Result<DiscoveredDevice, DiscoveryError> {
    let mut stmt = conn
        .prepare(
            "SELECT id, protocol, state, conn_status, display_name, vendor, model, address, point_count, discovered_at, accepted_at, protocol_meta FROM discovered_device WHERE id = ?1",
        )
        .map_err(|e| DiscoveryError::Db(e.to_string()))?;

    stmt.query_row(rusqlite::params![id], |row| Ok(row_to_device(row)))
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DiscoveryError::NotFound,
            other => DiscoveryError::Db(other.to_string()),
        })
}

fn row_to_device(row: &rusqlite::Row) -> DiscoveredDevice {
    let protocol_str: String = row.get(1).unwrap_or_default();
    let state_str: String = row.get(2).unwrap_or_default();
    let conn_str: String = row.get(3).unwrap_or_default();
    let meta_str: String = row.get(11).unwrap_or_default();

    DiscoveredDevice {
        id: row.get(0).unwrap_or_default(),
        protocol: DiscoveryProtocol::from_str(&protocol_str).unwrap_or(DiscoveryProtocol::Bacnet),
        state: DeviceState::from_str(&state_str).unwrap_or(DeviceState::Discovered),
        conn_status: ConnStatus::from_str(&conn_str).unwrap_or(ConnStatus::Unknown),
        display_name: row.get(4).unwrap_or_default(),
        vendor: row.get(5).unwrap_or_default(),
        model: row.get(6).unwrap_or_default(),
        address: row.get(7).unwrap_or_default(),
        point_count: row.get::<_, i64>(8).unwrap_or(0) as usize,
        discovered_at_ms: row.get(9).unwrap_or(0),
        accepted_at_ms: row.get(10).unwrap_or(None),
        protocol_meta: serde_json::from_str(&meta_str).unwrap_or(serde_json::Value::Object(Default::default())),
    }
}

fn get_points_db(conn: &rusqlite::Connection, device_id: &str) -> Vec<DiscoveredPoint> {
    let mut stmt = match conn.prepare(
        "SELECT id, device_id, display_name, description, units, point_kind, writable, binding_json, protocol_meta FROM discovered_point WHERE device_id = ?1 ORDER BY display_name",
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let rows = match stmt.query_map(rusqlite::params![device_id], |row| {
        let kind_str: String = row.get(5).unwrap_or_default();
        let writable_int: i32 = row.get(6).unwrap_or(0);
        let binding_str: String = row.get(7).unwrap_or_default();
        let meta_str: String = row.get(8).unwrap_or_default();

        Ok(DiscoveredPoint {
            id: row.get(0).unwrap_or_default(),
            device_id: row.get(1).unwrap_or_default(),
            display_name: row.get(2).unwrap_or_default(),
            description: row.get(3).unwrap_or_default(),
            units: row.get(4).unwrap_or_default(),
            point_kind: PointKindHint::from_str(&kind_str)
                .unwrap_or(PointKindHint::Analog),
            writable: writable_int != 0,
            binding: serde_json::from_str(&binding_str)
                .unwrap_or(crate::node::ProtocolBinding::Virtual),
            protocol_meta: serde_json::from_str(&meta_str)
                .unwrap_or(serde_json::Value::Object(Default::default())),
        })
    }) {
        Ok(r) => r,
        Err(_) => return vec![],
    };

    rows.filter_map(|r| r.ok()).collect()
}

fn set_device_state_db(
    conn: &rusqlite::Connection,
    id: &str,
    state: DeviceState,
) -> Result<(), DiscoveryError> {
    let accepted_at = if state == DeviceState::Accepted {
        Some(now_ms())
    } else {
        None
    };

    let rows = conn
        .execute(
            "UPDATE discovered_device SET state = ?1, accepted_at = COALESCE(?2, accepted_at) WHERE id = ?3",
            rusqlite::params![state.as_str(), accepted_at, id],
        )
        .map_err(|e| DiscoveryError::Db(e.to_string()))?;

    if rows == 0 {
        return Err(DiscoveryError::NotFound);
    }
    Ok(())
}

fn set_conn_status_db(
    conn: &rusqlite::Connection,
    id: &str,
    status: ConnStatus,
) -> Result<(), DiscoveryError> {
    let rows = conn
        .execute(
            "UPDATE discovered_device SET conn_status = ?1 WHERE id = ?2",
            rusqlite::params![status.as_str(), id],
        )
        .map_err(|e| DiscoveryError::Db(e.to_string()))?;

    if rows == 0 {
        return Err(DiscoveryError::NotFound);
    }
    Ok(())
}

fn record_scan_db(conn: &rusqlite::Connection, protocol: &str) -> i64 {
    let now = now_ms();
    conn.execute(
        "INSERT INTO discovery_scan (protocol, started_at, status) VALUES (?1, ?2, 'running')",
        rusqlite::params![protocol, now],
    )
    .unwrap_or(0);
    conn.last_insert_rowid()
}

fn finish_scan_db(conn: &rusqlite::Connection, scan_id: i64, device_count: usize) {
    let now = now_ms();
    let _ = conn.execute(
        "UPDATE discovery_scan SET ended_at = ?1, device_count = ?2, status = 'complete' WHERE id = ?3",
        rusqlite::params![now, device_count as i64, scan_id],
    );
}

// ----------------------------------------------------------------
// Tests
// ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::ProtocolBinding;

    fn test_store(path: &str) -> DiscoveryStore {
        let db_path = PathBuf::from(path);
        if db_path.exists() {
            std::fs::remove_file(&db_path).ok();
        }
        start_discovery_store_with_path(&db_path)
    }

    fn sample_device(id: &str) -> DiscoveredDevice {
        DiscoveredDevice {
            id: id.to_string(),
            protocol: DiscoveryProtocol::Bacnet,
            state: DeviceState::Discovered,
            conn_status: ConnStatus::Online,
            display_name: format!("BACnet Device {id}"),
            vendor: None,
            model: None,
            address: "192.168.1.100:47808".into(),
            point_count: 2,
            discovered_at_ms: 1000,
            accepted_at_ms: None,
            protocol_meta: serde_json::json!({}),
        }
    }

    fn sample_points(device_id: &str) -> Vec<DiscoveredPoint> {
        vec![
            DiscoveredPoint {
                id: "dat".into(),
                device_id: device_id.to_string(),
                display_name: "Discharge Air Temp".into(),
                description: Some("DAT sensor".into()),
                units: Some("°F".into()),
                point_kind: PointKindHint::Analog,
                writable: false,
                binding: ProtocolBinding::Bacnet {
                    device_instance: 1000,
                    object_type: "AnalogInput".into(),
                    object_instance: 1,
                },
                protocol_meta: serde_json::json!({}),
            },
            DiscoveredPoint {
                id: "fan-cmd".into(),
                device_id: device_id.to_string(),
                display_name: "Fan Run Command".into(),
                description: None,
                units: None,
                point_kind: PointKindHint::Binary,
                writable: true,
                binding: ProtocolBinding::Bacnet {
                    device_instance: 1000,
                    object_type: "BinaryOutput".into(),
                    object_instance: 2,
                },
                protocol_meta: serde_json::json!({}),
            },
        ]
    }

    #[tokio::test]
    async fn upsert_and_get_device() {
        let store = test_store("/tmp/test_discovery_upsert.db");
        let dev = sample_device("bacnet-1000");

        store.upsert_device(dev).await.unwrap();

        let fetched = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(fetched.id, "bacnet-1000");
        assert_eq!(fetched.protocol, DiscoveryProtocol::Bacnet);
        assert_eq!(fetched.state, DeviceState::Discovered);
        assert_eq!(fetched.conn_status, ConnStatus::Online);

        std::fs::remove_file("/tmp/test_discovery_upsert.db").ok();
    }

    #[tokio::test]
    async fn upsert_preserves_state_on_rediscovery() {
        let store = test_store("/tmp/test_discovery_preserve.db");
        let dev = sample_device("bacnet-1000");

        store.upsert_device(dev).await.unwrap();
        store
            .set_device_state("bacnet-1000", DeviceState::Accepted)
            .await
            .unwrap();

        // Re-discover same device
        let dev2 = sample_device("bacnet-1000");
        store.upsert_device(dev2).await.unwrap();

        // State should still be accepted (upsert preserves state)
        let fetched = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(fetched.state, DeviceState::Accepted);

        std::fs::remove_file("/tmp/test_discovery_preserve.db").ok();
    }

    #[tokio::test]
    async fn list_devices_with_filter() {
        let store = test_store("/tmp/test_discovery_filter.db");

        store.upsert_device(sample_device("bacnet-1000")).await.unwrap();
        store.upsert_device(sample_device("bacnet-2000")).await.unwrap();
        store
            .set_device_state("bacnet-1000", DeviceState::Accepted)
            .await
            .unwrap();

        let all = store.list_devices(None).await;
        assert_eq!(all.len(), 2);

        let accepted = store.list_devices(Some(DeviceState::Accepted)).await;
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].id, "bacnet-1000");

        let discovered = store.list_devices(Some(DeviceState::Discovered)).await;
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "bacnet-2000");

        std::fs::remove_file("/tmp/test_discovery_filter.db").ok();
    }

    #[tokio::test]
    async fn points_crud() {
        let store = test_store("/tmp/test_discovery_points.db");

        store.upsert_device(sample_device("bacnet-1000")).await.unwrap();
        store
            .upsert_points("bacnet-1000", sample_points("bacnet-1000"))
            .await
            .unwrap();

        let points = store.get_points("bacnet-1000").await;
        assert_eq!(points.len(), 2);

        // Verify first point
        let dat = points.iter().find(|p| p.id == "dat").unwrap();
        assert_eq!(dat.display_name, "Discharge Air Temp");
        assert_eq!(dat.units.as_deref(), Some("°F"));
        assert_eq!(dat.point_kind, PointKindHint::Analog);
        assert!(!dat.writable);

        // Verify second point
        let fan = points.iter().find(|p| p.id == "fan-cmd").unwrap();
        assert_eq!(fan.point_kind, PointKindHint::Binary);
        assert!(fan.writable);

        std::fs::remove_file("/tmp/test_discovery_points.db").ok();
    }

    #[tokio::test]
    async fn state_transitions() {
        let store = test_store("/tmp/test_discovery_states.db");
        store.upsert_device(sample_device("bacnet-1000")).await.unwrap();

        // Discovered → Ignored
        store
            .set_device_state("bacnet-1000", DeviceState::Ignored)
            .await
            .unwrap();
        let dev = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(dev.state, DeviceState::Ignored);

        // Ignored → Discovered (un-ignore)
        store
            .set_device_state("bacnet-1000", DeviceState::Discovered)
            .await
            .unwrap();
        let dev = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(dev.state, DeviceState::Discovered);

        // Discovered → Accepted
        store
            .set_device_state("bacnet-1000", DeviceState::Accepted)
            .await
            .unwrap();
        let dev = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(dev.state, DeviceState::Accepted);
        assert!(dev.accepted_at_ms.is_some());

        std::fs::remove_file("/tmp/test_discovery_states.db").ok();
    }

    #[tokio::test]
    async fn conn_status_update() {
        let store = test_store("/tmp/test_discovery_conn.db");
        store.upsert_device(sample_device("bacnet-1000")).await.unwrap();

        store
            .set_conn_status("bacnet-1000", ConnStatus::Offline)
            .await
            .unwrap();
        let dev = store.get_device("bacnet-1000").await.unwrap();
        assert_eq!(dev.conn_status, ConnStatus::Offline);

        std::fs::remove_file("/tmp/test_discovery_conn.db").ok();
    }

    #[tokio::test]
    async fn scan_tracking() {
        let store = test_store("/tmp/test_discovery_scan.db");

        let scan_id = store.record_scan("bacnet").await;
        assert!(scan_id > 0);

        store.finish_scan(scan_id, 3).await;
        // No assertion needed — just verifying it doesn't panic

        std::fs::remove_file("/tmp/test_discovery_scan.db").ok();
    }
}
