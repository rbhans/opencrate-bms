use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustbac_bacnet_sc::BacnetScTransport;
use rustbac_client::{
    schedule::{self, CalendarEntry, TimeValue},
    walk::walk_device,
    BacnetClient, ClientDataValue, CovManagerBuilder, CovSubscriptionSpec, EventNotification,
    ReadRangeResult,
};
use rustbac_core::services::device_management::{DeviceCommunicationState, ReinitializeState};
use rustbac_core::types::{ObjectId, ObjectType, PropertyId};
use rustbac_datalink::{BacnetIpTransport, DataLink};
use tokio::task::JoinHandle;

use crate::config::profile::PointValue;
use crate::config::scenario::ScenarioSettings;
use crate::event::bus::{Event, EventBus};
use crate::store::history_store::HistoryStore;
use crate::store::point_store::{PointKey, PointStatusFlags, PointStore};

use super::traits::BridgeError;

/// Convert scenario-level BACnet network settings into a `BacnetConfig`.
pub fn bacnet_config_from_scenario(settings: &Option<ScenarioSettings>) -> BacnetConfig {
    let bacnet_net = settings
        .as_ref()
        .and_then(|s| s.bacnet.as_ref());

    let bacnet_net = match bacnet_net {
        Some(b) => b,
        None => return BacnetConfig::default(),
    };

    let mode_str = bacnet_net.mode.as_deref().unwrap_or("normal");
    let mode = match mode_str {
        "foreign" => {
            let addr_str = bacnet_net
                .bbmd_addr
                .as_deref()
                .unwrap_or("255.255.255.255:47808");
            let addr = addr_str
                .parse()
                .unwrap_or_else(|_| "255.255.255.255:47808".parse().unwrap());
            let ttl = bacnet_net.ttl.unwrap_or(60);
            BacnetMode::Foreign {
                bbmd_addr: addr,
                ttl,
            }
        }
        "sc" => {
            let hub = bacnet_net
                .hub_endpoint
                .clone()
                .unwrap_or_default();
            BacnetMode::SecureConnect { hub_endpoint: hub }
        }
        _ => BacnetMode::Normal,
    };

    BacnetConfig { mode }
}

// ---------------------------------------------------------------------------
// BACnet network configuration
// ---------------------------------------------------------------------------

/// How the BACnet client connects to the network.
#[derive(Debug, Clone)]
pub enum BacnetMode {
    /// Standard BACnet/IP on the local subnet.
    Normal,
    /// Register as a foreign device with a BBMD for cross-subnet communication.
    Foreign {
        bbmd_addr: std::net::SocketAddr,
        ttl: u16,
    },
    /// BACnet Secure Connect — tunnel over WebSocket to a BACnet/SC hub.
    SecureConnect {
        hub_endpoint: String,
    },
}

/// Configuration for the BACnet network transport.
#[derive(Debug, Clone)]
pub struct BacnetConfig {
    pub mode: BacnetMode,
}

impl Default for BacnetConfig {
    fn default() -> Self {
        Self {
            mode: BacnetMode::Normal,
        }
    }
}

// ---------------------------------------------------------------------------
// Discovered device/object model
// ---------------------------------------------------------------------------

/// A BACnet device discovered on the network.
#[derive(Debug, Clone)]
pub struct BacnetDevice {
    pub device_id: ObjectId,
    pub address: rustbac_datalink::DataLinkAddress,
    pub vendor: Option<String>,
    pub model: Option<String>,
    pub firmware_revision: Option<String>,
    pub objects: Vec<BacnetObject>,
    pub trend_logs: Vec<TrendLogRef>,
}

/// Reference to a TrendLog object on a remote device.
#[derive(Debug, Clone)]
pub struct TrendLogRef {
    pub object_id: ObjectId,
    pub object_name: Option<String>,
}

/// A BACnet object discovered via device walk.
#[derive(Debug, Clone)]
pub struct BacnetObject {
    pub object_id: ObjectId,
    pub object_name: Option<String>,
    pub description: Option<String>,
    pub units: Option<u32>,
    pub present_value: Option<ClientDataValue>,
    pub writable: bool,
}

// ---------------------------------------------------------------------------
// Transport-agnostic client wrapper
// ---------------------------------------------------------------------------

/// Internal enum that wraps both BACnet/IP and BACnet/SC client types,
/// allowing the bridge to be non-generic while supporting both transports.
#[derive(Clone)]
enum TransportClient {
    Ip(Arc<BacnetClient<BacnetIpTransport>>),
    Sc(Arc<BacnetClient<BacnetScTransport>>),
}

/// Helper macro to dispatch a method call on TransportClient to the inner Arc.
macro_rules! with_client {
    ($self:expr, |$c:ident| $body:expr) => {
        match $self {
            TransportClient::Ip($c) => $body,
            TransportClient::Sc($c) => $body,
        }
    };
}

// ---------------------------------------------------------------------------
// BacnetBridge — client-side BACnet integration (IP + SC)
// ---------------------------------------------------------------------------

pub struct BacnetBridge {
    discovery_timeout: Duration,
    poll_interval: Duration,
    cov_lifetime: u32,
    bacnet_config: BacnetConfig,
    transport: Option<TransportClient>,
    devices: Vec<BacnetDevice>,
    /// Maps (device_instance, object_instance) → PointKey for fast lookup
    point_map: HashMap<(u32, u32), ObjectId>,
    store: Option<PointStore>,
    history_store: Option<HistoryStore>,
    event_bus: Option<EventBus>,
    cov_handle: Option<JoinHandle<()>>,
    poll_handle: Option<JoinHandle<()>>,
    time_sync_handle: Option<JoinHandle<()>>,
    event_poll_handle: Option<JoinHandle<()>>,
    trend_log_handle: Option<JoinHandle<()>>,
}

impl Default for BacnetBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl BacnetBridge {
    pub fn new() -> Self {
        BacnetBridge {
            discovery_timeout: Duration::from_secs(5),
            poll_interval: Duration::from_secs(30),
            cov_lifetime: 300, // 5 minutes
            bacnet_config: BacnetConfig::default(),
            transport: None,
            devices: Vec::new(),
            point_map: HashMap::new(),
            store: None,
            history_store: None,
            event_bus: None,
            cov_handle: None,
            poll_handle: None,
            time_sync_handle: None,
            event_poll_handle: None,
            trend_log_handle: None,
        }
    }

    pub fn with_discovery_timeout(mut self, timeout: Duration) -> Self {
        self.discovery_timeout = timeout;
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    pub fn with_bacnet_config(mut self, config: BacnetConfig) -> Self {
        self.bacnet_config = config;
        self
    }

    pub fn with_history_store(mut self, store: HistoryStore) -> Self {
        self.history_store = Some(store);
        self
    }

    pub fn discovered_devices(&self) -> &[BacnetDevice] {
        &self.devices
    }

    // -----------------------------------------------------------------------
    // Device management operations
    // -----------------------------------------------------------------------

    fn require_transport(&self) -> Result<&TransportClient, BridgeError> {
        self.transport.as_ref().ok_or_else(|| {
            BridgeError::ConnectionFailed("BACnet bridge not started".into())
        })
    }

    /// Reboot a BACnet device (coldstart or warmstart).
    pub async fn reinitialize_device(
        &self,
        device_instance: u32,
        warmstart: bool,
    ) -> Result<(), BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let state = if warmstart {
            ReinitializeState::Warmstart
        } else {
            ReinitializeState::Coldstart
        };
        with_client!(tc, |c| c
            .reinitialize_device(dev.address, state, None)
            .await
            .map_err(|e| BridgeError::Protocol(format!("ReinitializeDevice failed: {e}"))))?;
        Ok(())
    }

    /// Enable or disable communication on a BACnet device.
    pub async fn device_communication_control(
        &self,
        device_instance: u32,
        enable: bool,
        duration_minutes: Option<u16>,
    ) -> Result<(), BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let state = if enable {
            DeviceCommunicationState::Enable
        } else {
            DeviceCommunicationState::Disable
        };
        let duration_secs = duration_minutes.map(|m| m.saturating_mul(60));
        with_client!(tc, |c| c
            .device_communication_control(dev.address, duration_secs, state, None)
            .await
            .map_err(|e| BridgeError::Protocol(format!(
                "DeviceCommunicationControl failed: {e}"
            ))))?;
        Ok(())
    }

    /// Synchronize time on a BACnet device to the current system UTC time.
    pub async fn sync_time(&self, device_instance: u32) -> Result<(), BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let (date, time) = now_bacnet_utc();
        with_client!(tc, |c| c
            .time_synchronize(dev.address, date, time, true)
            .await
            .map_err(|e| BridgeError::Protocol(format!("TimeSynchronization failed: {e}"))))?;
        Ok(())
    }

    /// Poll the device for active event/alarm information.
    pub async fn get_event_info(
        &self,
        device_instance: u32,
    ) -> Result<Vec<BacnetEventInfo>, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let result = with_client!(tc, |c| c
            .get_event_information(dev.address, None)
            .await
            .map_err(|e| BridgeError::Protocol(format!("GetEventInformation failed: {e}"))))?;
        Ok(result
            .summaries
            .into_iter()
            .map(|s| BacnetEventInfo {
                object_id: s.object_id,
                event_state: s.event_state_raw,
            })
            .collect())
    }

    // -----------------------------------------------------------------------
    // TrendLog reading
    // -----------------------------------------------------------------------

    /// Read entries from a TrendLog object on a remote device.
    /// Returns (timestamp_ms, value) pairs suitable for history backfill.
    pub async fn read_trend_log(
        &self,
        device_instance: u32,
        trend_log_instance: u32,
        start_index: i32,
        count: i16,
    ) -> Result<Vec<(i64, f64)>, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::TrendLog, trend_log_instance);
        let result: ReadRangeResult = with_client!(tc, |c| c
            .read_range_by_position(
                dev.address,
                object_id,
                PropertyId::LogBuffer,
                None,
                start_index,
                count,
            )
            .await
            .map_err(|e| BridgeError::Protocol(format!("ReadRange failed: {e}"))))?;

        Ok(trend_log_items_to_samples(&result.items))
    }

    /// Get the record count of a TrendLog object.
    pub async fn trend_log_record_count(
        &self,
        device_instance: u32,
        trend_log_instance: u32,
    ) -> Result<u32, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::TrendLog, trend_log_instance);
        let val = with_client!(tc, |c| c
            .read_property(dev.address, object_id, PropertyId::RecordCount)
            .await
            .map_err(|e| BridgeError::Protocol(format!("ReadProperty RecordCount: {e}"))))?;
        match val {
            ClientDataValue::Unsigned(n) => Ok(n),
            _ => Ok(0),
        }
    }

    /// Backfill a TrendLog into the HistoryStore. Reads all records and inserts them.
    pub async fn backfill_trend_log(
        &self,
        device_instance: u32,
        trend_log_instance: u32,
        device_key: &str,
        point_id: &str,
        history_store: &HistoryStore,
    ) -> Result<usize, BridgeError> {
        let record_count = self
            .trend_log_record_count(device_instance, trend_log_instance)
            .await?;
        if record_count == 0 {
            return Ok(0);
        }

        let batch_size: i16 = 100;
        let mut total = 0usize;
        let mut index: i32 = 1; // BACnet ReadRange is 1-based

        while (index as u32) <= record_count {
            let remaining = record_count.saturating_sub(index as u32);
            let count = batch_size.min(remaining as i16 + 1);
            let samples = self
                .read_trend_log(device_instance, trend_log_instance, index, count)
                .await?;

            if samples.is_empty() {
                break;
            }

            let point_key = format!("{device_key}:{point_id}");
            let batch: Vec<(String, i64, f64)> = samples
                .iter()
                .map(|(ts, v)| (point_key.clone(), *ts, *v))
                .collect();
            total += batch.len();
            history_store.backfill(batch).await;

            index += count as i32;
        }

        println!(
            "BACnet: backfilled {total} TrendLog records for {device_key}/{point_id}"
        );
        Ok(total)
    }

    // -----------------------------------------------------------------------
    // Schedule interop
    // -----------------------------------------------------------------------

    /// Read the weekly schedule from a BACnet Schedule object.
    pub async fn read_schedule(
        &self,
        device_instance: u32,
        schedule_instance: u32,
    ) -> Result<Vec<Vec<TimeValue>>, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::Schedule, schedule_instance);
        let val = with_client!(tc, |c| c
            .read_property(dev.address, object_id, PropertyId::WeeklySchedule)
            .await
            .map_err(|e| BridgeError::Protocol(format!("Read WeeklySchedule: {e}"))))?;
        schedule::decode_weekly_schedule(&val).ok_or_else(|| {
            BridgeError::Protocol("Failed to decode WeeklySchedule".into())
        })
    }

    /// Write a weekly schedule to a BACnet Schedule object.
    pub async fn write_schedule(
        &self,
        device_instance: u32,
        schedule_instance: u32,
        week: &[Vec<TimeValue>],
    ) -> Result<(), BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::Schedule, schedule_instance);
        let encoded = schedule::encode_weekly_schedule(week);
        with_client!(tc, |c| c
            .write_many(
                dev.address,
                &[(object_id, PropertyId::WeeklySchedule, encoded, None)],
            )
            .await
            .map_err(|e| BridgeError::Protocol(format!("Write WeeklySchedule: {e}"))))?;
        Ok(())
    }

    /// Read the default value from a BACnet Schedule object.
    pub async fn read_schedule_default(
        &self,
        device_instance: u32,
        schedule_instance: u32,
    ) -> Result<ClientDataValue, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::Schedule, schedule_instance);
        with_client!(tc, |c| c
            .read_property(dev.address, object_id, PropertyId::ScheduleDefault)
            .await
            .map_err(|e| BridgeError::Protocol(format!("Read ScheduleDefault: {e}"))))
    }

    /// Read the date list from a BACnet Calendar object.
    pub async fn read_calendar(
        &self,
        device_instance: u32,
        calendar_instance: u32,
    ) -> Result<Vec<CalendarEntry>, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::Calendar, calendar_instance);
        let val = with_client!(tc, |c| c
            .read_property(dev.address, object_id, PropertyId::DateList)
            .await
            .map_err(|e| BridgeError::Protocol(format!("Read DateList: {e}"))))?;
        schedule::decode_date_list(&val).ok_or_else(|| {
            BridgeError::Protocol("Failed to decode DateList".into())
        })
    }

    /// Read the exception schedule from a BACnet Schedule object.
    pub async fn read_exception_schedule(
        &self,
        device_instance: u32,
        schedule_instance: u32,
    ) -> Result<ClientDataValue, BridgeError> {
        let tc = self.require_transport()?;
        let dev = self.find_device(device_instance)?;
        let object_id = ObjectId::new(ObjectType::Schedule, schedule_instance);
        with_client!(tc, |c| c
            .read_property(dev.address, object_id, PropertyId::ExceptionSchedule)
            .await
            .map_err(|e| BridgeError::Protocol(format!("Read ExceptionSchedule: {e}"))))
    }

    fn find_device(&self, device_instance: u32) -> Result<&BacnetDevice, BridgeError> {
        self.devices
            .iter()
            .find(|d| d.device_id.instance() == device_instance)
            .ok_or_else(|| BridgeError::PointNotFound {
                device_id: format!("bacnet-{device_instance}"),
                point_id: String::new(),
            })
    }
}

/// Summary of an event/alarm on a remote BACnet device.
#[derive(Debug, Clone)]
pub struct BacnetEventInfo {
    pub object_id: ObjectId,
    pub event_state: u32,
}

// ---------------------------------------------------------------------------
// PointSource implementation
// ---------------------------------------------------------------------------

impl super::traits::PointSource for BacnetBridge {
    async fn start(&mut self, store: PointStore) -> Result<(), BridgeError> {
        self.store = Some(store.clone());

        // 1. Create BACnet client (Normal, Foreign, or Secure Connect)
        match &self.bacnet_config.mode {
            BacnetMode::Normal => {
                let client = BacnetClient::new()
                    .await
                    .map_err(|e| BridgeError::ConnectionFailed(format!("BACnet/IP init: {e}")))?;
                let tc = TransportClient::Ip(Arc::new(client));
                self.start_with_transport(tc, store.clone()).await?;
            }
            BacnetMode::Foreign { bbmd_addr, ttl } => {
                println!("BACnet: registering as foreign device with BBMD {bbmd_addr} (TTL={ttl}s)");
                let client = BacnetClient::new_foreign(*bbmd_addr, *ttl)
                    .await
                    .map_err(|e| BridgeError::ConnectionFailed(format!("BACnet/IP foreign init: {e}")))?;
                let tc = TransportClient::Ip(Arc::new(client));
                self.start_with_transport(tc, store.clone()).await?;
            }
            BacnetMode::SecureConnect { hub_endpoint } => {
                println!("BACnet: connecting to SC hub {hub_endpoint}...");
                let client = BacnetClient::new_sc(hub_endpoint.clone())
                    .await
                    .map_err(|e| BridgeError::ConnectionFailed(format!("BACnet/SC init: {e}")))?;
                let tc = TransportClient::Sc(Arc::new(client));
                self.start_with_transport(tc, store.clone()).await?;
            }
        }

        let total_points: usize = self.devices.iter().map(|d| d.objects.len()).sum();
        println!(
            "BACnet: monitoring {} device(s), {} point(s)",
            self.devices.len(),
            total_points,
        );

        Ok(())

    }

    async fn stop(&mut self) -> Result<(), BridgeError> {
        for handle in [
            self.cov_handle.take(),
            self.poll_handle.take(),
            self.time_sync_handle.take(),
            self.event_poll_handle.take(),
            self.trend_log_handle.take(),
        ] {
            if let Some(h) = handle {
                h.abort();
            }
        }
        self.transport = None;
        Ok(())
    }

    async fn write_point(
        &self,
        device_id: &str,
        point_id: &str,
        value: PointValue,
        priority: Option<u8>,
    ) -> Result<(), BridgeError> {
        let tc = self.require_transport()?;

        // Find the device and object
        let (dev, obj) = self
            .devices
            .iter()
            .flat_map(|d| d.objects.iter().map(move |o| (d, o)))
            .find(|(d, o)| {
                let dev_key = format!("bacnet-{}", d.device_id.instance());
                dev_key == device_id && object_point_id(o) == point_id
            })
            .ok_or_else(|| BridgeError::PointNotFound {
                device_id: device_id.to_string(),
                point_id: point_id.to_string(),
            })?;

        if !obj.writable {
            return Err(BridgeError::WriteRejected(format!(
                "Object {} is not writable",
                obj.object_id.instance()
            )));
        }

        let bac_value = point_value_to_client(&value, obj.object_id.object_type());
        with_client!(tc, |c| c
            .write_many(
                dev.address,
                &[(obj.object_id, PropertyId::PresentValue, bac_value, priority)],
            )
            .await
            .map_err(|e| BridgeError::Protocol(format!("WriteProperty failed: {e}"))))?;

        // Update PointStore immediately so value is reflected without waiting for next poll/COV
        if let Some(store) = &self.store {
            store.set(
                PointKey {
                    device_instance_id: device_id.to_string(),
                    point_id: point_id.to_string(),
                },
                value,
            );
        }

        Ok(())
    }
}

impl BacnetBridge {
    /// Internal helper: discover devices and start background loops.
    async fn start_with_transport(
        &mut self,
        tc: TransportClient,
        store: PointStore,
    ) -> Result<(), BridgeError> {
        let discovery_timeout = self.discovery_timeout;
        // 2. Discover devices via Who-Is broadcast
        println!(
            "BACnet: sending Who-Is broadcast (waiting {}s)...",
            discovery_timeout.as_secs()
        );
        let discovered = match with_client!(&tc, |c| c.who_is(None, discovery_timeout).await) {
            Ok(devs) => devs,
            Err(e) => {
                println!("BACnet: discovery failed ({e}), no devices found.");
                return Ok(());
            }
        };

        if discovered.is_empty() {
            println!("BACnet: no devices discovered on the network.");
            return Ok(());
        }

        println!("BACnet: discovered {} device(s)", discovered.len());

        // 3. Walk each discovered device to enumerate objects
        let mut all_devices = Vec::new();
        for dev in &discovered {
            let device_id = match dev.device_id {
                Some(id) => id,
                None => continue,
            };

            println!(
                "BACnet: walking device {} (instance {})...",
                device_id.object_type(),
                device_id.instance()
            );

            match with_client!(&tc, |c| walk_device(c, dev.address, device_id).await) {
                Ok(walk_result) => {
                    let mut objects = Vec::new();
                    let mut trend_logs = Vec::new();

                    for o in walk_result.objects {
                        if o.object_id.object_type() == ObjectType::TrendLog {
                            trend_logs.push(TrendLogRef {
                                object_id: o.object_id,
                                object_name: o.object_name,
                            });
                        } else if is_point_object(o.object_id.object_type()) {
                            let classification =
                                rustbac_client::point::classify_point(o.object_id.object_type());
                            objects.push(BacnetObject {
                                object_id: o.object_id,
                                object_name: o.object_name,
                                description: o.description,
                                units: o.units,
                                present_value: o.present_value,
                                writable: classification.writable,
                            });
                        }
                    }

                    println!(
                        "  device {} — {} point(s), {} trend log(s)",
                        device_id.instance(),
                        objects.len(),
                        trend_logs.len(),
                    );

                    all_devices.push(BacnetDevice {
                        device_id,
                        address: dev.address,
                        vendor: walk_result.device_info.vendor_name,
                        model: walk_result.device_info.model_name,
                        firmware_revision: walk_result.device_info.firmware_revision,
                        objects,
                        trend_logs,
                    });
                }
                Err(e) => {
                    println!(
                        "  device {} — walk failed: {e}",
                        device_id.instance()
                    );
                }
            }
        }

        // 4. Populate PointStore with discovered objects
        for dev in &all_devices {
            let dev_instance = dev.device_id.instance();
            let device_key = format!("bacnet-{dev_instance}");

            for obj in &dev.objects {
                let point_id = object_point_id(obj);
                let key = PointKey {
                    device_instance_id: device_key.clone(),
                    point_id: point_id.clone(),
                };

                if let Some(pv) = &obj.present_value {
                    store.set(key, client_to_point_value(pv, obj.object_id.object_type()));
                }

                self.point_map
                    .insert((dev_instance, obj.object_id.instance()), obj.object_id);
            }
        }

        self.devices = all_devices;
        self.transport = Some(tc.clone());

        // 5. Start COV subscriptions with polling fallback
        let cov_tc = tc.clone();
        let cov_store = store.clone();
        let cov_devices = self.devices.clone();
        let poll_interval = self.poll_interval;
        let cov_lifetime = self.cov_lifetime;
        let cov_event_bus = self.event_bus.clone();

        let cov_handle = tokio::spawn(async move {
            run_cov_with_poll_fallback(
                cov_tc,
                cov_store,
                &cov_devices,
                poll_interval,
                cov_lifetime,
                cov_event_bus,
            )
            .await;
        });
        self.cov_handle = Some(cov_handle);

        // 6. Start periodic time synchronization (every 4 hours)
        let ts_tc = tc.clone();
        let ts_devices = self.devices.clone();
        let ts_handle = tokio::spawn(async move {
            run_time_sync_loop(ts_tc, &ts_devices).await;
        });
        self.time_sync_handle = Some(ts_handle);

        // 7. Start event notification polling (for intrinsic reporting)
        let ev_tc = tc.clone();
        let ev_devices = self.devices.clone();
        let ev_event_bus = self.event_bus.clone();
        let ev_store = store.clone();
        let ev_handle = tokio::spawn(async move {
            run_event_poll_loop(ev_tc, ev_store, &ev_devices, ev_event_bus).await;
        });
        self.event_poll_handle = Some(ev_handle);

        // 8. Start periodic TrendLog sync (if any devices have TrendLog objects)
        let has_trend_logs = self.devices.iter().any(|d| !d.trend_logs.is_empty());
        if has_trend_logs {
            if let Some(history_store) = &self.history_store {
                let tl_tc = tc;
                let tl_devices = self.devices.clone();
                let tl_history = history_store.clone();
                let tl_handle = tokio::spawn(async move {
                    run_trend_log_sync_loop(tl_tc, &tl_devices, tl_history).await;
                });
                self.trend_log_handle = Some(tl_handle);
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// COV + polling loop
// ---------------------------------------------------------------------------

async fn run_cov_with_poll_fallback(
    tc: TransportClient,
    store: PointStore,
    devices: &[BacnetDevice],
    poll_interval: Duration,
    cov_lifetime: u32,
    event_bus: Option<EventBus>,
) {
    // CovManagerBuilder is generic over DataLink, so dispatch on transport type.
    // If COV fails to start, fall back to plain polling.
    let cov_ok = match &tc {
        TransportClient::Ip(client) => {
            run_cov_inner(Arc::clone(client), store.clone(), devices, poll_interval, cov_lifetime, event_bus.clone()).await
        }
        TransportClient::Sc(client) => {
            run_cov_inner(Arc::clone(client), store.clone(), devices, poll_interval, cov_lifetime, event_bus.clone()).await
        }
    };
    if !cov_ok {
        poll_loop(tc, store, devices, poll_interval, event_bus).await;
    }
}

/// Returns true if COV ran successfully, false if it failed to start (caller should fall back).
async fn run_cov_inner<D: DataLink + 'static>(
    client: Arc<BacnetClient<D>>,
    store: PointStore,
    devices: &[BacnetDevice],
    poll_interval: Duration,
    cov_lifetime: u32,
    _event_bus: Option<EventBus>,
) -> bool {
    let mut builder = CovManagerBuilder::new(Arc::clone(&client))
        .poll_interval(poll_interval)
        .silence_threshold(Duration::from_secs((cov_lifetime as u64) / 2))
        .renewal_fraction(0.75);

    let mut process_id: u32 = 1;
    let mut sub_map: HashMap<u32, (String, String)> = HashMap::new(); // process_id → (device_key, point_id)

    for dev in devices {
        let dev_key = format!("bacnet-{}", dev.device_id.instance());
        for obj in &dev.objects {
            let point_id = object_point_id(obj);
            sub_map.insert(process_id, (dev_key.clone(), point_id));

            builder = builder.subscribe(CovSubscriptionSpec {
                address: dev.address,
                object_id: obj.object_id,
                property_id: None, // subscribe to all properties
                lifetime_seconds: cov_lifetime,
                cov_increment: None,
                confirmed: false,
                subscriber_process_id: process_id,
            });
            process_id += 1;
        }
    }

    let mut manager = match builder.build() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("BACnet: COV manager failed to start: {e}, falling back to polling");
            return false;
        }
    };

    // Process COV updates and write to PointStore
    while let Some(update) = manager.recv().await {
        // Find the device/point this update belongs to
        // Look through devices to find the matching one
        for dev in devices {
            let dk = format!("bacnet-{}", dev.device_id.instance());
            let point_id_str = dev
                .objects
                .iter()
                .find(|o| o.object_id == update.object_id)
                .map(object_point_id);

            if let Some(pid) = point_id_str {
                for prop in &update.values {
                    let key = PointKey {
                        device_instance_id: dk.clone(),
                        point_id: pid.clone(),
                    };
                    match prop.property_id {
                        PropertyId::PresentValue => {
                            store.set(
                                key,
                                client_to_point_value(
                                    &prop.value,
                                    update.object_id.object_type(),
                                ),
                            );
                        }
                        PropertyId::StatusFlags => {
                            apply_bacnet_status_flags(&store, &key, &prop.value);
                        }
                        _ => {}
                    }
                }
                break;
            }
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Backoff state for reconnection logic
// ---------------------------------------------------------------------------

const BACKOFF_BASE_SECS: u64 = 2;
const BACKOFF_MAX_SECS: u64 = 300; // 5 minutes
const DEVICE_DOWN_THRESHOLD: u32 = 5;

struct DeviceBackoff {
    failures: u32,
    next_retry: Instant,
    was_down: bool,
}

impl DeviceBackoff {
    fn new() -> Self {
        Self {
            failures: 0,
            next_retry: Instant::now(),
            was_down: false,
        }
    }

    fn record_success(&mut self) {
        self.failures = 0;
        self.next_retry = Instant::now();
    }

    fn record_failure(&mut self) {
        self.failures = self.failures.saturating_add(1);
        let delay_secs = std::cmp::min(
            BACKOFF_BASE_SECS.saturating_pow(self.failures),
            BACKOFF_MAX_SECS,
        );
        self.next_retry = Instant::now() + Duration::from_secs(delay_secs);
    }

    fn should_skip(&self) -> bool {
        Instant::now() < self.next_retry
    }

    fn is_down(&self) -> bool {
        self.failures >= DEVICE_DOWN_THRESHOLD
    }
}

/// Simple periodic polling fallback when COV is unavailable.
async fn poll_loop(
    tc: TransportClient,
    store: PointStore,
    devices: &[BacnetDevice],
    interval: Duration,
    event_bus: Option<EventBus>,
) {
    let mut backoffs: HashMap<u32, DeviceBackoff> = devices
        .iter()
        .map(|d| (d.device_id.instance(), DeviceBackoff::new()))
        .collect();

    loop {
        for dev in devices {
            let instance = dev.device_id.instance();
            let dev_key = format!("bacnet-{instance}");

            let backoff = backoffs.entry(instance).or_insert_with(DeviceBackoff::new);
            if backoff.should_skip() {
                continue;
            }

            // Build batch read requests for PresentValue + StatusFlags
            let requests: Vec<(ObjectId, PropertyId)> = dev
                .objects
                .iter()
                .flat_map(|o| {
                    vec![
                        (o.object_id, PropertyId::PresentValue),
                        (o.object_id, PropertyId::StatusFlags),
                    ]
                })
                .collect();

            if requests.is_empty() {
                continue;
            }

            match with_client!(&tc, |c| c.read_many(dev.address, &requests).await) {
                Ok(results) => {
                    let was_down = backoff.was_down;
                    backoff.record_success();

                    // Clear DOWN on all points for this device on success
                    for obj in &dev.objects {
                        let key = PointKey {
                            device_instance_id: dev_key.clone(),
                            point_id: object_point_id(obj),
                        };
                        store.clear_status(&key, PointStatusFlags::DOWN);
                    }

                    // Process results
                    for ((obj_id, prop_id), value) in &results {
                        if let Some(obj) = dev.objects.iter().find(|o| o.object_id == *obj_id) {
                            let point_id = object_point_id(obj);
                            let key = PointKey {
                                device_instance_id: dev_key.clone(),
                                point_id,
                            };
                            match prop_id {
                                PropertyId::PresentValue => {
                                    store.clear_status(&key, PointStatusFlags::FAULT);
                                    store.set(
                                        key,
                                        client_to_point_value(value, obj_id.object_type()),
                                    );
                                }
                                PropertyId::StatusFlags => {
                                    apply_bacnet_status_flags(&store, &key, value);
                                }
                                _ => {}
                            }
                        }
                    }

                    // Publish recovery event if device was previously down
                    if was_down {
                        backoff.was_down = false;
                        if let Some(ref bus) = event_bus {
                            let _ = bus.publish(Event::DeviceDiscovered {
                                bridge_type: "bacnet".into(),
                                device_key: dev_key.clone(),
                            });
                        }
                        println!("BACnet: device {instance} recovered");
                    }
                }
                Err(e) => {
                    backoff.record_failure();
                    eprintln!(
                        "BACnet: poll failed for device {instance}: {e} (failure #{})",
                        backoff.failures
                    );

                    // Set DOWN on all points for this device
                    for obj in &dev.objects {
                        let key = PointKey {
                            device_instance_id: dev_key.clone(),
                            point_id: object_point_id(obj),
                        };
                        store.set_status(&key, PointStatusFlags::DOWN);
                    }

                    // Publish DeviceDown after threshold
                    if backoff.is_down() && !backoff.was_down {
                        backoff.was_down = true;
                        if let Some(ref bus) = event_bus {
                            let _ = bus.publish(Event::DeviceDown {
                                bridge_type: "bacnet".into(),
                                device_key: dev_key.clone(),
                            });
                        }
                        println!(
                            "BACnet: device {instance} marked DOWN after {} consecutive failures",
                            backoff.failures
                        );
                    }
                }
            }
        }

        tokio::time::sleep(interval).await;
    }
}

// ---------------------------------------------------------------------------
// Periodic time synchronization
// ---------------------------------------------------------------------------

/// How often to send UTC time synchronization to all devices.
const TIME_SYNC_INTERVAL_SECS: u64 = 4 * 3600; // every 4 hours

async fn run_time_sync_loop(
    tc: TransportClient,
    devices: &[BacnetDevice],
) {
    // Initial sync shortly after startup
    tokio::time::sleep(Duration::from_secs(30)).await;
    loop {
        let (date, time) = now_bacnet_utc();
        for dev in devices {
            if let Err(e) = with_client!(&tc, |c| c
                .time_synchronize(dev.address, date, time, true)
                .await)
            {
                eprintln!(
                    "BACnet: time sync failed for device {}: {e}",
                    dev.device_id.instance()
                );
            }
        }
        tokio::time::sleep(Duration::from_secs(TIME_SYNC_INTERVAL_SECS)).await;
    }
}

/// Convert current system UTC time to BACnet Date + Time.
fn now_bacnet_utc() -> (rustbac_core::types::Date, rustbac_core::types::Time) {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Simple UTC date/time calculation
    let days_since_epoch = (secs / 86400) as i64;
    let time_of_day = secs % 86400;

    // Days from 1970-01-01
    // Algorithm: convert days since epoch to year/month/day
    let (year, month, day, weekday) = days_to_ymd(days_since_epoch);

    let date = rustbac_core::types::Date {
        year_since_1900: ((year - 1900).clamp(0, 255)) as u8,
        month: month as u8,
        day: day as u8,
        weekday: weekday as u8,
    };
    let time = rustbac_core::types::Time {
        hour: (time_of_day / 3600) as u8,
        minute: ((time_of_day % 3600) / 60) as u8,
        second: (time_of_day % 60) as u8,
        hundredths: 0,
    };
    (date, time)
}

/// Convert days since Unix epoch to (year, month, day, weekday).
/// Weekday: 1=Monday..7=Sunday (BACnet convention).
fn days_to_ymd(days: i64) -> (i32, i32, i32, i32) {
    // 1970-01-01 was a Thursday (weekday=4)
    let weekday = ((days % 7 + 4 - 1) % 7 + 1) as i32; // 1=Mon..7=Sun

    // Civil calendar conversion (Euclidean affine algorithm)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if m <= 2 { y + 1 } else { y };

    (year, m as i32, d as i32, weekday)
}

// ---------------------------------------------------------------------------
// Event notification polling (intrinsic reporting)
// ---------------------------------------------------------------------------

/// How often to poll devices for event/alarm notifications.
const EVENT_POLL_INTERVAL_SECS: u64 = 60;

async fn run_event_poll_loop(
    tc: TransportClient,
    store: PointStore,
    devices: &[BacnetDevice],
    event_bus: Option<EventBus>,
) {
    // Wait for initial startup to settle
    tokio::time::sleep(Duration::from_secs(15)).await;

    loop {
        for dev in devices {
            let dev_key = format!("bacnet-{}", dev.device_id.instance());

            // Try to receive any unsolicited event notifications (non-blocking check)
            match with_client!(&tc, |c| c
                .recv_event_notification(Duration::from_millis(100))
                .await)
            {
                Ok(Some(notification)) => {
                    handle_event_notification(&store, &event_bus, &dev_key, &notification);
                }
                Ok(None) => {} // no pending notification
                Err(_) => {}   // timeout or error, continue
            }

            // Also poll GetEventInformation for this device
            match with_client!(&tc, |c| c.get_event_information(dev.address, None).await) {
                Ok(result) => {
                    for summary in &result.summaries {
                        // event_state_raw: 0=normal, 1=fault, 2=offnormal, 3=high-limit, 4=low-limit
                        if summary.event_state_raw != 0 {
                            // Find matching point
                            let point_id = dev
                                .objects
                                .iter()
                                .find(|o| o.object_id == summary.object_id)
                                .map(object_point_id);

                            if let Some(pid) = point_id {
                                // Set ALARM flag on the point
                                let key = PointKey {
                                    device_instance_id: dev_key.clone(),
                                    point_id: pid.clone(),
                                };
                                store.set_status(&key, PointStatusFlags::ALARM);

                                if let Some(ref bus) = event_bus {
                                    let node_id = format!("{dev_key}/{pid}");
                                    let _ = bus.publish(Event::AlarmRaised {
                                        alarm_id: summary.object_id.instance() as i64,
                                        node_id,
                                    });
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    // Not all devices support GetEventInformation — this is expected
                    // Only log at debug level to avoid spam
                    let _ = e; // suppress unused warning
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(EVENT_POLL_INTERVAL_SECS)).await;
    }
}

/// Process an unsolicited BACnet EventNotification.
fn handle_event_notification(
    store: &PointStore,
    event_bus: &Option<EventBus>,
    dev_key: &str,
    notification: &EventNotification,
) {
    let instance = notification.event_object_id.instance();
    let event_type = notification.event_type;

    // Map to_state to alarm action
    let is_alarm = notification
        .to_state
        .map(|s| s != rustbac_client::EventState::Normal)
        .unwrap_or(notification.to_state_raw != 0);

    let point_id = format!(
        "{}-{}",
        notification.event_object_id.object_type(),
        instance
    );

    let key = PointKey {
        device_instance_id: dev_key.to_string(),
        point_id: point_id.clone(),
    };

    if is_alarm {
        store.set_status(&key, PointStatusFlags::ALARM);
        if let Some(ref bus) = event_bus {
            let _ = bus.publish(Event::AlarmRaised {
                alarm_id: instance as i64,
                node_id: format!("{dev_key}/{point_id}"),
            });
        }
    } else {
        store.clear_status(&key, PointStatusFlags::ALARM);
        if let Some(ref bus) = event_bus {
            let _ = bus.publish(Event::AlarmCleared {
                alarm_id: instance as i64,
                node_id: format!("{dev_key}/{point_id}"),
            });
        }
    }

    println!(
        "BACnet: event notification — device {dev_key}, object {instance}, \
         type={event_type}, to_state={}, message={:?}",
        notification.to_state_raw,
        notification.message_text
    );
}

// ---------------------------------------------------------------------------
// Periodic TrendLog synchronization
// ---------------------------------------------------------------------------

/// How often to poll TrendLog objects for new records.
const TREND_LOG_SYNC_INTERVAL_SECS: u64 = 300; // 5 minutes

/// Periodically reads new TrendLog records from all devices and inserts into HistoryStore.
/// Tracks the last-read record count per TrendLog to only fetch incremental records.
async fn run_trend_log_sync_loop(
    tc: TransportClient,
    devices: &[BacnetDevice],
    history_store: HistoryStore,
) {
    // Wait for startup to settle
    tokio::time::sleep(Duration::from_secs(60)).await;

    // Track last-known record count per (device_instance, trendlog_instance)
    let mut last_counts: HashMap<(u32, u32), u32> = HashMap::new();

    loop {
        for dev in devices {
            let dev_instance = dev.device_id.instance();
            let dev_key = format!("bacnet-{dev_instance}");

            for tl in &dev.trend_logs {
                let tl_instance = tl.object_id.instance();
                let tl_key = (dev_instance, tl_instance);

                // Read current record count
                let current_count = match with_client!(&tc, |c| c
                    .read_property(
                        dev.address,
                        tl.object_id,
                        PropertyId::RecordCount,
                    )
                    .await)
                {
                    Ok(ClientDataValue::Unsigned(n)) => n,
                    _ => continue,
                };

                let last_count = last_counts.get(&tl_key).copied().unwrap_or(0);

                if current_count <= last_count {
                    // No new records
                    last_counts.insert(tl_key, current_count);
                    continue;
                }

                // Read only the new records
                let new_start = (last_count + 1) as i32;
                let fallback_name = format!("TrendLog-{tl_instance}");
                let point_id = tl
                    .object_name
                    .as_deref()
                    .unwrap_or(&fallback_name);
                let point_key = format!("{dev_key}:{point_id}");

                // Read in batches of 100
                let batch_size: i16 = 100;
                let mut index = new_start;
                let mut total = 0usize;

                while index <= current_count as i32 {
                    let remaining = current_count as i32 - index + 1;
                    let count = batch_size.min(remaining as i16);

                    let items = match with_client!(&tc, |c| c
                        .read_range_by_position(
                            dev.address,
                            tl.object_id,
                            PropertyId::LogBuffer,
                            None,
                            index,
                            count,
                        )
                        .await)
                    {
                        Ok(result) => result.items,
                        Err(e) => {
                            eprintln!(
                                "BACnet: TrendLog sync failed for {dev_key}/TrendLog-{tl_instance}: {e}"
                            );
                            break;
                        }
                    };

                    if items.is_empty() {
                        break;
                    }

                    let samples = trend_log_items_to_samples(&items);
                    let batch: Vec<(String, i64, f64)> = samples
                        .iter()
                        .map(|(ts, v)| (point_key.clone(), *ts, *v))
                        .collect();
                    total += batch.len();
                    history_store.backfill(batch).await;

                    index += count as i32;
                }

                if total > 0 {
                    println!(
                        "BACnet: synced {total} new TrendLog records for {dev_key}/{point_id}"
                    );
                }

                last_counts.insert(tl_key, current_count);
            }
        }

        tokio::time::sleep(Duration::from_secs(TREND_LOG_SYNC_INTERVAL_SECS)).await;
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Map BACnet StatusFlags (BitString) to PointStatusFlags on the store.
///
/// BACnet StatusFlags bit ordering (ASHRAE 135):
///   bit 0 = IN_ALARM
///   bit 1 = FAULT
///   bit 2 = OVERRIDDEN
///   bit 3 = OUT_OF_SERVICE
fn apply_bacnet_status_flags(store: &PointStore, key: &PointKey, value: &ClientDataValue) {
    let (unused_bits, data) = match value {
        ClientDataValue::BitString { unused_bits, data } => (*unused_bits, data.as_slice()),
        _ => return,
    };

    // BACnet StatusFlags is a 4-bit BitString.
    // Bit ordering: MSB-first within each byte.
    // Bit 0 (MSB of byte 0) = IN_ALARM
    // Bit 1 = FAULT
    // Bit 2 = OVERRIDDEN
    // Bit 3 = OUT_OF_SERVICE
    let total_bits = data.len() * 8 - unused_bits as usize;

    let mappings: &[(usize, u8)] = &[
        (0, PointStatusFlags::ALARM),      // IN_ALARM
        (1, PointStatusFlags::FAULT),      // FAULT
        (2, PointStatusFlags::OVERRIDDEN), // OVERRIDDEN
        (3, PointStatusFlags::DISABLED),   // OUT_OF_SERVICE
    ];

    for &(bit_index, flag) in mappings {
        if bit_index < total_bits {
            let byte_idx = bit_index / 8;
            let bit_pos = 7 - (bit_index % 8); // MSB-first
            let is_set = byte_idx < data.len() && (data[byte_idx] & (1 << bit_pos)) != 0;
            if is_set {
                store.set_status(key, flag);
            } else {
                store.clear_status(key, flag);
            }
        }
    }
}

/// Extract (timestamp_ms, f64) pairs from TrendLog ReadRange items.
/// TrendLog entries are typically Constructed values with date/time + value.
fn trend_log_items_to_samples(items: &[ClientDataValue]) -> Vec<(i64, f64)> {
    let mut samples = Vec::new();
    for item in items {
        if let ClientDataValue::Constructed { values, .. } = item {
            // BACnet LogRecord: { timestamp, logDatum }
            // Try to extract a numeric value from the last element
            let value = values.iter().rev().find_map(|p| match p {
                ClientDataValue::Real(f) => Some(*f as f64),
                ClientDataValue::Double(f) => Some(*f),
                ClientDataValue::Unsigned(u) => Some(*u as f64),
                ClientDataValue::Signed(i) => Some(*i as f64),
                ClientDataValue::Enumerated(e) => Some(*e as f64),
                ClientDataValue::Boolean(b) => Some(if *b { 1.0 } else { 0.0 }),
                _ => None,
            });
            // Try to extract a timestamp from a Date+Time pair at the start
            let ts_ms = extract_log_timestamp(values);
            if let (Some(ts), Some(val)) = (ts_ms, value) {
                samples.push((ts, val));
            }
        }
    }
    samples
}

/// Try to extract a Unix timestamp from BACnet Date+Time values at the start of a LogRecord.
fn extract_log_timestamp(parts: &[ClientDataValue]) -> Option<i64> {
    // Look for a Date followed by a Time in the constructed value
    let mut date_opt = None;
    let mut time_opt = None;
    for part in parts {
        if let ClientDataValue::Constructed { values: inner, .. } = part {
            // Nested date-time constructed value
            return extract_log_timestamp(inner);
        }
        // Date is typically encoded as OctetString(4 bytes) or as a tagged value
        if let ClientDataValue::OctetString(bytes) = part {
            if bytes.len() == 4 && date_opt.is_none() {
                // year_since_1900, month, day, weekday
                let year = 1900 + bytes[0] as i64;
                let month = bytes[1] as i64;
                let day = bytes[2] as i64;
                // Simple conversion — days since epoch
                let days = civil_to_days(year as i32, month as i32, day as i32);
                date_opt = Some(days * 86400 * 1000);
            } else if bytes.len() == 4 && date_opt.is_some() {
                // hour, minute, second, hundredths
                let ms = (bytes[0] as i64) * 3_600_000
                    + (bytes[1] as i64) * 60_000
                    + (bytes[2] as i64) * 1000
                    + (bytes[3] as i64) * 10;
                time_opt = Some(ms);
            }
        }
    }
    match (date_opt, time_opt) {
        (Some(d), Some(t)) => Some(d + t),
        (Some(d), None) => Some(d),
        _ => {
            // Fallback: use current time if we can't parse the timestamp
            use std::time::{SystemTime, UNIX_EPOCH};
            Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            )
        }
    }
}

/// Convert a civil date to days since Unix epoch (inverse of days_to_ymd).
fn civil_to_days(year: i32, month: i32, day: i32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let m = if month <= 2 { month + 9 } else { month - 3 } as u32;
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * m + 2) / 5 + day as u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era as i64) * 146097 + (doe as i64) - 719468
}

/// Returns true for object types that represent monitorable points.
fn is_point_object(ot: ObjectType) -> bool {
    matches!(
        ot,
        ObjectType::AnalogInput
            | ObjectType::AnalogOutput
            | ObjectType::AnalogValue
            | ObjectType::BinaryInput
            | ObjectType::BinaryOutput
            | ObjectType::BinaryValue
            | ObjectType::MultiStateInput
            | ObjectType::MultiStateOutput
            | ObjectType::MultiStateValue
            | ObjectType::Accumulator
            | ObjectType::PulseConverter
    )
}

/// Build a stable point ID string from a BACnet object.
/// Prefers ObjectName if available, otherwise uses "type-instance" format.
fn object_point_id(obj: &BacnetObject) -> String {
    match &obj.object_name {
        Some(name) if !name.is_empty() => name.clone(),
        _ => format!(
            "{}-{}",
            obj.object_id.object_type(),
            obj.object_id.instance()
        ),
    }
}

/// Convert our PointValue to a BACnet ClientDataValue appropriate for the object type.
fn point_value_to_client(pv: &PointValue, ot: ObjectType) -> ClientDataValue {
    let classification = rustbac_client::point::classify_point(ot);
    match (pv, classification.kind) {
        (PointValue::Float(f), rustbac_client::PointKind::Analog) => {
            ClientDataValue::Real(*f as f32)
        }
        (PointValue::Integer(i), rustbac_client::PointKind::Analog) => {
            ClientDataValue::Real(*i as f32)
        }
        (PointValue::Bool(b), rustbac_client::PointKind::Binary) => {
            ClientDataValue::Enumerated(if *b { 1 } else { 0 })
        }
        (PointValue::Integer(i), rustbac_client::PointKind::MultiState) => {
            ClientDataValue::Unsigned(*i as u32)
        }
        // Fallbacks
        (PointValue::Float(f), _) => ClientDataValue::Real(*f as f32),
        (PointValue::Integer(i), _) => ClientDataValue::Unsigned(*i as u32),
        (PointValue::Bool(b), _) => ClientDataValue::Enumerated(if *b { 1 } else { 0 }),
    }
}

/// Convert a BACnet ClientDataValue to our PointValue, using the object type
/// to preserve semantic types (e.g. binary objects → Bool, not Integer).
fn client_to_point_value(cv: &ClientDataValue, ot: ObjectType) -> PointValue {
    let classification = rustbac_client::point::classify_point(ot);
    match classification.kind {
        rustbac_client::PointKind::Binary => {
            // BACnet binary uses Enumerated(0=inactive, 1=active)
            let active = match cv {
                ClientDataValue::Enumerated(e) => *e != 0,
                ClientDataValue::Boolean(b) => *b,
                ClientDataValue::Unsigned(u) => *u != 0,
                ClientDataValue::Real(f) => *f != 0.0,
                _ => false,
            };
            PointValue::Bool(active)
        }
        rustbac_client::PointKind::MultiState => {
            let state = match cv {
                ClientDataValue::Unsigned(u) => *u as i64,
                ClientDataValue::Enumerated(e) => *e as i64,
                ClientDataValue::Signed(i) => *i as i64,
                ClientDataValue::Real(f) => *f as i64,
                _ => 0,
            };
            PointValue::Integer(state)
        }
        _ => {
            // Analog and everything else → Float
            match cv {
                ClientDataValue::Real(f) => PointValue::Float(*f as f64),
                ClientDataValue::Double(f) => PointValue::Float(*f),
                ClientDataValue::Unsigned(u) => PointValue::Float(*u as f64),
                ClientDataValue::Signed(i) => PointValue::Float(*i as f64),
                ClientDataValue::Boolean(b) => PointValue::Float(if *b { 1.0 } else { 0.0 }),
                ClientDataValue::Enumerated(e) => PointValue::Float(*e as f64),
                _ => PointValue::Float(0.0),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::scenario::{BacnetNetworkConfig, ScenarioSettings};

    // -- Backoff progression --------------------------------------------------

    #[test]
    fn backoff_initial_state() {
        let b = DeviceBackoff::new();
        assert!(!b.should_skip());
        assert!(!b.is_down());
        assert_eq!(b.failures, 0);
    }

    #[test]
    fn backoff_progression() {
        let mut b = DeviceBackoff::new();

        // First failure: delay = 2^1 = 2s
        b.record_failure();
        assert_eq!(b.failures, 1);
        assert!(b.should_skip()); // within the 2s window
        assert!(!b.is_down());

        // Accumulate to threshold
        for _ in 1..DEVICE_DOWN_THRESHOLD {
            b.record_failure();
        }
        assert_eq!(b.failures, DEVICE_DOWN_THRESHOLD);
        assert!(b.is_down());
    }

    #[test]
    fn backoff_success_resets() {
        let mut b = DeviceBackoff::new();
        for _ in 0..DEVICE_DOWN_THRESHOLD {
            b.record_failure();
        }
        assert!(b.is_down());

        b.record_success();
        assert_eq!(b.failures, 0);
        assert!(!b.is_down());
        assert!(!b.should_skip());
    }

    #[test]
    fn backoff_delay_capped() {
        let mut b = DeviceBackoff::new();
        // Many failures — delay should cap at BACKOFF_MAX_SECS
        for _ in 0..50 {
            b.record_failure();
        }
        // next_retry should be at most BACKOFF_MAX_SECS from now
        let max_future = Instant::now() + Duration::from_secs(BACKOFF_MAX_SECS + 1);
        assert!(b.next_retry < max_future);
    }

    // -- StatusFlags mapping --------------------------------------------------

    #[test]
    fn status_flags_all_clear() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "bacnet-1".into(),
            point_id: "temp".into(),
        };
        store.set(key.clone(), PointValue::Float(72.0));

        // StatusFlags: 4 bits, all clear → 0x00, unused_bits=4
        let value = ClientDataValue::BitString {
            unused_bits: 4,
            data: vec![0x00],
        };
        apply_bacnet_status_flags(&store, &key, &value);

        let ts = store.get(&key).unwrap();
        assert!(!ts.status.has(PointStatusFlags::ALARM));
        assert!(!ts.status.has(PointStatusFlags::FAULT));
        assert!(!ts.status.has(PointStatusFlags::OVERRIDDEN));
        assert!(!ts.status.has(PointStatusFlags::DISABLED));
    }

    #[test]
    fn status_flags_in_alarm() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "bacnet-1".into(),
            point_id: "temp".into(),
        };
        store.set(key.clone(), PointValue::Float(72.0));

        // IN_ALARM = bit 0 (MSB) → byte 0x80, unused_bits=4
        let value = ClientDataValue::BitString {
            unused_bits: 4,
            data: vec![0x80],
        };
        apply_bacnet_status_flags(&store, &key, &value);

        let ts = store.get(&key).unwrap();
        assert!(ts.status.has(PointStatusFlags::ALARM));
        assert!(!ts.status.has(PointStatusFlags::FAULT));
        assert!(!ts.status.has(PointStatusFlags::OVERRIDDEN));
        assert!(!ts.status.has(PointStatusFlags::DISABLED));
    }

    #[test]
    fn status_flags_all_set() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "bacnet-1".into(),
            point_id: "temp".into(),
        };
        store.set(key.clone(), PointValue::Float(72.0));

        // All 4 bits set: 0xF0 (bits 7,6,5,4 → IN_ALARM, FAULT, OVERRIDDEN, OUT_OF_SERVICE)
        let value = ClientDataValue::BitString {
            unused_bits: 4,
            data: vec![0xF0],
        };
        apply_bacnet_status_flags(&store, &key, &value);

        let ts = store.get(&key).unwrap();
        assert!(ts.status.has(PointStatusFlags::ALARM));
        assert!(ts.status.has(PointStatusFlags::FAULT));
        assert!(ts.status.has(PointStatusFlags::OVERRIDDEN));
        assert!(ts.status.has(PointStatusFlags::DISABLED));
    }

    #[test]
    fn status_flags_fault_only() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "bacnet-1".into(),
            point_id: "temp".into(),
        };
        store.set(key.clone(), PointValue::Float(72.0));

        // FAULT = bit 1 → 0x40, unused_bits=4
        let value = ClientDataValue::BitString {
            unused_bits: 4,
            data: vec![0x40],
        };
        apply_bacnet_status_flags(&store, &key, &value);

        let ts = store.get(&key).unwrap();
        assert!(!ts.status.has(PointStatusFlags::ALARM));
        assert!(ts.status.has(PointStatusFlags::FAULT));
        assert!(!ts.status.has(PointStatusFlags::OVERRIDDEN));
        assert!(!ts.status.has(PointStatusFlags::DISABLED));
    }

    #[test]
    fn status_flags_non_bitstring_ignored() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "bacnet-1".into(),
            point_id: "temp".into(),
        };
        store.set(key.clone(), PointValue::Float(72.0));

        // Non-BitString value should be silently ignored
        let value = ClientDataValue::Unsigned(42);
        apply_bacnet_status_flags(&store, &key, &value);

        let ts = store.get(&key).unwrap();
        assert!(ts.status.is_normal());
    }

    // -- Date conversion ------------------------------------------------------

    #[test]
    fn days_to_ymd_epoch() {
        // 1970-01-01 is day 0, Thursday (weekday=4)
        let (y, m, d, wd) = days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
        assert_eq!(wd, 4); // Thursday
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2024-01-01 = day 19723 (from epoch), Monday
        let days = 19723;
        let (y, m, d, wd) = days_to_ymd(days);
        assert_eq!((y, m, d), (2024, 1, 1));
        assert_eq!(wd, 1); // Monday
    }

    #[test]
    fn days_to_ymd_leap_day() {
        // 2024-02-29 = 19723 + 59 = 19782
        let days = 19782;
        let (y, m, d, _wd) = days_to_ymd(days);
        assert_eq!((y, m, d), (2024, 2, 29));
    }

    #[test]
    fn now_bacnet_utc_valid_ranges() {
        let (date, time) = now_bacnet_utc();
        // Year should be recent (2020+)
        assert!(date.year_since_1900 >= 120); // 2020
        assert!((1..=12).contains(&date.month));
        assert!((1..=31).contains(&date.day));
        assert!((1..=7).contains(&date.weekday));
        assert!(time.hour < 24);
        assert!(time.minute < 60);
        assert!(time.second < 60);
    }

    // -- BacnetConfig from scenario -------------------------------------------

    #[test]
    fn config_from_scenario_none() {
        let config = bacnet_config_from_scenario(&None);
        assert!(matches!(config.mode, BacnetMode::Normal));
    }

    #[test]
    fn config_from_scenario_no_bacnet() {
        let settings = Some(ScenarioSettings {
            tick_rate_ms: Some(100),
            realtime: None,
            bacnet: None,
        });
        let config = bacnet_config_from_scenario(&settings);
        assert!(matches!(config.mode, BacnetMode::Normal));
    }

    #[test]
    fn config_from_scenario_normal() {
        let settings = Some(ScenarioSettings {
            tick_rate_ms: None,
            realtime: None,
            bacnet: Some(BacnetNetworkConfig {
                mode: Some("normal".into()),
                bbmd_addr: None,
                ttl: None,
                hub_endpoint: None,
            }),
        });
        let config = bacnet_config_from_scenario(&settings);
        assert!(matches!(config.mode, BacnetMode::Normal));
    }

    #[test]
    fn config_from_scenario_foreign() {
        let settings = Some(ScenarioSettings {
            tick_rate_ms: None,
            realtime: None,
            bacnet: Some(BacnetNetworkConfig {
                mode: Some("foreign".into()),
                bbmd_addr: Some("192.168.1.1:47808".into()),
                ttl: Some(120),
                hub_endpoint: None,
            }),
        });
        let config = bacnet_config_from_scenario(&settings);
        match config.mode {
            BacnetMode::Foreign { bbmd_addr, ttl } => {
                assert_eq!(bbmd_addr.to_string(), "192.168.1.1:47808");
                assert_eq!(ttl, 120);
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn config_from_scenario_sc() {
        let settings = Some(ScenarioSettings {
            tick_rate_ms: None,
            realtime: None,
            bacnet: Some(BacnetNetworkConfig {
                mode: Some("sc".into()),
                bbmd_addr: None,
                ttl: None,
                hub_endpoint: Some("wss://hub.example.com:1234/bacnet".into()),
            }),
        });
        let config = bacnet_config_from_scenario(&settings);
        match config.mode {
            BacnetMode::SecureConnect { hub_endpoint } => {
                assert_eq!(hub_endpoint, "wss://hub.example.com:1234/bacnet");
            }
            other => panic!("expected SecureConnect, got {other:?}"),
        }
    }

    #[test]
    fn config_from_scenario_foreign_defaults() {
        // Foreign mode without explicit addr/ttl should use defaults
        let settings = Some(ScenarioSettings {
            tick_rate_ms: None,
            realtime: None,
            bacnet: Some(BacnetNetworkConfig {
                mode: Some("foreign".into()),
                bbmd_addr: None,
                ttl: None,
                hub_endpoint: None,
            }),
        });
        let config = bacnet_config_from_scenario(&settings);
        match config.mode {
            BacnetMode::Foreign { ttl, .. } => {
                assert_eq!(ttl, 60);
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    // -- civil_to_days / days_to_ymd roundtrip ---------------------------------

    #[test]
    fn civil_to_days_epoch() {
        assert_eq!(civil_to_days(1970, 1, 1), 0);
    }

    #[test]
    fn civil_to_days_known_date() {
        // 2024-01-01 should be day 19723
        assert_eq!(civil_to_days(2024, 1, 1), 19723);
    }

    #[test]
    fn civil_days_roundtrip() {
        for days in [0i64, 1, 365, 10000, 19723, 19782, 20000] {
            let (y, m, d, _wd) = days_to_ymd(days);
            let back = civil_to_days(y, m, d);
            assert_eq!(back, days, "roundtrip failed for days={days} -> ({y},{m},{d})");
        }
    }

    // -- TrendLog sample extraction -------------------------------------------

    #[test]
    fn trend_log_items_empty() {
        let items: Vec<ClientDataValue> = vec![];
        assert!(trend_log_items_to_samples(&items).is_empty());
    }

    #[test]
    fn trend_log_items_non_constructed_skipped() {
        let items = vec![ClientDataValue::Real(42.0), ClientDataValue::Unsigned(7)];
        assert!(trend_log_items_to_samples(&items).is_empty());
    }

    #[test]
    fn trend_log_items_constructed_with_value() {
        // Constructed with an OctetString date, OctetString time, and a Real value
        let date_bytes = vec![
            124, // 1900+124 = 2024
            1,   // January
            1,   // day 1
            1,   // Monday
        ];
        let time_bytes = vec![
            12, // hour
            30, // minute
            0,  // second
            0,  // hundredths
        ];
        let items = vec![ClientDataValue::Constructed {
            tag_num: 0,
            values: vec![
                ClientDataValue::OctetString(date_bytes),
                ClientDataValue::OctetString(time_bytes),
                ClientDataValue::Real(72.5),
            ],
        }];
        let samples = trend_log_items_to_samples(&items);
        assert_eq!(samples.len(), 1);
        assert!((samples[0].1 - 72.5).abs() < f64::EPSILON);
        // Timestamp should be 2024-01-01 12:30:00 UTC in ms
        let expected_ts = 19723 * 86400 * 1000 + 12 * 3600000 + 30 * 60000;
        assert_eq!(samples[0].0, expected_ts);
    }
}
