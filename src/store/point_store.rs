use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crate::config::profile::{DeviceProfile, PointValue};
use crate::event::bus::{Event, EventBus};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PointKey {
    pub device_instance_id: String,
    pub point_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PointStatusFlags(pub u8);

impl PointStatusFlags {
    pub const ALARM: u8      = 0b0000_0001;
    pub const STALE: u8      = 0b0000_0010;
    pub const FAULT: u8      = 0b0000_0100;
    pub const OVERRIDDEN: u8 = 0b0000_1000;
    pub const DOWN: u8       = 0b0001_0000;
    pub const DISABLED: u8   = 0b0010_0000;

    pub fn has(self, flag: u8) -> bool { self.0 & flag != 0 }
    pub fn set(&mut self, flag: u8) { self.0 |= flag; }
    pub fn clear(&mut self, flag: u8) { self.0 &= !flag; }
    pub fn is_normal(self) -> bool { self.0 == 0 }

    /// Returns the highest-priority active flag name for display
    pub fn worst_status(self) -> Option<&'static str> {
        if self.has(Self::DOWN) { Some("down") }
        else if self.has(Self::FAULT) { Some("fault") }
        else if self.has(Self::ALARM) { Some("alarm") }
        else if self.has(Self::OVERRIDDEN) { Some("overridden") }
        else if self.has(Self::STALE) { Some("stale") }
        else if self.has(Self::DISABLED) { Some("disabled") }
        else { None }
    }

    /// Returns all active flag names
    pub fn active_flags(self) -> Vec<&'static str> {
        let mut flags = Vec::new();
        if self.has(Self::DOWN) { flags.push("down"); }
        if self.has(Self::FAULT) { flags.push("fault"); }
        if self.has(Self::ALARM) { flags.push("alarm"); }
        if self.has(Self::OVERRIDDEN) { flags.push("overridden"); }
        if self.has(Self::STALE) { flags.push("stale"); }
        if self.has(Self::DISABLED) { flags.push("disabled"); }
        flags
    }
}

#[derive(Debug, Clone)]
pub struct TimestampedValue {
    pub value: PointValue,
    pub timestamp: Instant,
    pub status: PointStatusFlags,
}

#[derive(Clone)]
pub struct PointStore {
    data: Arc<RwLock<HashMap<PointKey, TimestampedValue>>>,
    version_tx: tokio::sync::watch::Sender<u64>,
    version_rx: tokio::sync::watch::Receiver<u64>,
    history_tx: tokio::sync::broadcast::Sender<(PointKey, PointValue)>,
    event_bus: Option<EventBus>,
}

impl Default for PointStore {
    fn default() -> Self {
        Self::new()
    }
}

impl PointStore {
    pub fn new() -> Self {
        let (version_tx, version_rx) = tokio::sync::watch::channel(0u64);
        let (history_tx, _) = tokio::sync::broadcast::channel(1024);
        PointStore {
            data: Arc::new(RwLock::new(HashMap::new())),
            version_tx,
            version_rx,
            history_tx,
            event_bus: None,
        }
    }

    pub fn with_event_bus(mut self, bus: EventBus) -> Self {
        self.event_bus = Some(bus);
        self
    }

    pub fn get(&self, key: &PointKey) -> Option<TimestampedValue> {
        self.data.read().unwrap().get(key).cloned()
    }

    pub fn set(&self, key: PointKey, value: PointValue) {
        let _ = self.history_tx.send((key.clone(), value.clone()));
        let mut data = self.data.write().unwrap();
        let existing_status = data.get(&key).map(|tv| tv.status).unwrap_or_default();
        data.insert(key.clone(), TimestampedValue {
            value: value.clone(),
            timestamp: Instant::now(),
            status: existing_status,
        });
        drop(data);
        let current = *self.version_rx.borrow();
        let _ = self.version_tx.send(current + 1);

        if let Some(ref bus) = self.event_bus {
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            bus.publish(Event::ValueChanged {
                node_id: format!("{}/{}", key.device_instance_id, key.point_id),
                value,
                timestamp_ms: now_ms,
            });
        }
    }

    pub fn get_all_for_device(&self, device_instance_id: &str) -> Vec<(PointKey, TimestampedValue)> {
        self.data
            .read()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.device_instance_id == device_instance_id)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }

    pub fn initialize_from_profile(&self, device_instance_id: &str, profile: &DeviceProfile) {
        for point in &profile.points {
            if let Some(initial) = &point.initial_value {
                let key = PointKey {
                    device_instance_id: device_instance_id.to_string(),
                    point_id: point.id.clone(),
                };
                let ts = TimestampedValue {
                    value: initial.clone(),
                    timestamp: Instant::now(),
                    status: PointStatusFlags::default(),
                };
                self.data.write().unwrap().insert(key, ts);
            }
        }
        let current = *self.version_rx.borrow();
        let _ = self.version_tx.send(current + 1);
    }

    pub fn point_count(&self) -> usize {
        self.data.read().unwrap().len()
    }

    pub fn device_ids(&self) -> Vec<String> {
        let data = self.data.read().unwrap();
        let mut ids: Vec<String> = data
            .keys()
            .map(|k| k.device_instance_id.clone())
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        ids.sort();
        ids
    }

    /// Set a status flag on a point (additive — does not clear other flags)
    pub fn set_status(&self, key: &PointKey, flag: u8) {
        let mut data = self.data.write().unwrap();
        let new_flags = if let Some(tv) = data.get_mut(key) {
            tv.status.set(flag);
            Some(tv.status.0)
        } else {
            None
        };
        drop(data);
        let current = *self.version_rx.borrow();
        let _ = self.version_tx.send(current + 1);

        if let (Some(ref bus), Some(flags)) = (&self.event_bus, new_flags) {
            bus.publish(Event::StatusChanged {
                node_id: format!("{}/{}", key.device_instance_id, key.point_id),
                flags,
            });
        }
    }

    /// Clear a status flag on a point
    pub fn clear_status(&self, key: &PointKey, flag: u8) {
        let mut data = self.data.write().unwrap();
        let new_flags = if let Some(tv) = data.get_mut(key) {
            tv.status.clear(flag);
            Some(tv.status.0)
        } else {
            None
        };
        drop(data);
        let current = *self.version_rx.borrow();
        let _ = self.version_tx.send(current + 1);

        if let (Some(ref bus), Some(flags)) = (&self.event_bus, new_flags) {
            bus.publish(Event::StatusChanged {
                node_id: format!("{}/{}", key.device_instance_id, key.point_id),
                flags,
            });
        }
    }

    /// Get all point keys (for status sync iteration)
    pub fn all_keys(&self) -> Vec<PointKey> {
        self.data.read().unwrap().keys().cloned().collect()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.version_rx.clone()
    }

    pub fn subscribe_history(&self) -> tokio::sync::broadcast::Receiver<(PointKey, PointValue)> {
        self.history_tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_set_roundtrip() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "ahu-1".to_string(),
            point_id: "dat".to_string(),
        };

        store.set(key.clone(), PointValue::Float(55.0));

        let result = store.get(&key).unwrap();
        assert!(matches!(result.value, PointValue::Float(f) if (f - 55.0).abs() < f64::EPSILON));
    }

    #[test]
    fn get_all_for_device() {
        let store = PointStore::new();
        store.set(
            PointKey {
                device_instance_id: "ahu-1".to_string(),
                point_id: "dat".to_string(),
            },
            PointValue::Float(55.0),
        );
        store.set(
            PointKey {
                device_instance_id: "ahu-1".to_string(),
                point_id: "oat".to_string(),
            },
            PointValue::Float(85.0),
        );
        store.set(
            PointKey {
                device_instance_id: "vav-1".to_string(),
                point_id: "zat".to_string(),
            },
            PointValue::Float(72.0),
        );

        let ahu_points = store.get_all_for_device("ahu-1");
        assert_eq!(ahu_points.len(), 2);

        let vav_points = store.get_all_for_device("vav-1");
        assert_eq!(vav_points.len(), 1);
    }

    #[test]
    fn initialize_from_profile() {
        let json = std::fs::read_to_string("profiles/ahu-single-duct.json").unwrap();
        let profile: DeviceProfile = serde_json::from_str(&json).unwrap();

        let store = PointStore::new();
        store.initialize_from_profile("ahu-1", &profile);

        assert_eq!(store.point_count(), 35);

        let dat = store.get(&PointKey {
            device_instance_id: "ahu-1".to_string(),
            point_id: "dat".to_string(),
        });
        assert!(dat.is_some());
    }

    #[test]
    fn status_flags_basics() {
        let mut flags = PointStatusFlags::default();
        assert!(flags.is_normal());
        assert_eq!(flags.worst_status(), None);

        flags.set(PointStatusFlags::ALARM);
        assert!(!flags.is_normal());
        assert!(flags.has(PointStatusFlags::ALARM));
        assert_eq!(flags.worst_status(), Some("alarm"));

        flags.set(PointStatusFlags::DOWN);
        assert_eq!(flags.worst_status(), Some("down"));
        assert_eq!(flags.active_flags(), vec!["down", "alarm"]);

        flags.clear(PointStatusFlags::DOWN);
        assert!(!flags.has(PointStatusFlags::DOWN));
        assert_eq!(flags.worst_status(), Some("alarm"));
    }

    #[test]
    fn set_preserves_status_flags() {
        let store = PointStore::new();
        let key = PointKey {
            device_instance_id: "ahu-1".to_string(),
            point_id: "dat".to_string(),
        };

        store.set(key.clone(), PointValue::Float(55.0));
        store.set_status(&key, PointStatusFlags::ALARM);

        // Update value — status should be preserved
        store.set(key.clone(), PointValue::Float(60.0));
        let result = store.get(&key).unwrap();
        assert!(result.status.has(PointStatusFlags::ALARM));
        assert!(matches!(result.value, PointValue::Float(f) if (f - 60.0).abs() < f64::EPSILON));
    }

    #[test]
    fn all_keys() {
        let store = PointStore::new();
        store.set(
            PointKey { device_instance_id: "a".into(), point_id: "1".into() },
            PointValue::Float(1.0),
        );
        store.set(
            PointKey { device_instance_id: "b".into(), point_id: "2".into() },
            PointValue::Float(2.0),
        );
        assert_eq!(store.all_keys().len(), 2);
    }
}
