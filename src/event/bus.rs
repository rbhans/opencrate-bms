use std::sync::Arc;

use tokio::sync::broadcast;

use crate::config::profile::PointValue;

/// Platform-wide event for cross-system communication.
#[derive(Debug, Clone)]
pub enum Event {
    ValueChanged {
        node_id: String,
        value: PointValue,
        timestamp_ms: i64,
    },
    StatusChanged {
        node_id: String,
        flags: u8,
    },
    AlarmRaised {
        alarm_id: i64,
        node_id: String,
    },
    AlarmCleared {
        alarm_id: i64,
        node_id: String,
    },
    AlarmAcknowledged {
        alarm_id: i64,
    },
    ScheduleWritten {
        assignment_id: i64,
        node_id: String,
        value: PointValue,
    },
    EntityCreated {
        entity_id: String,
    },
    EntityUpdated {
        entity_id: String,
    },
    EntityDeleted {
        entity_id: String,
    },
    DeviceDiscovered {
        bridge_type: String,
        device_key: String,
    },
    DeviceDown {
        bridge_type: String,
        device_key: String,
    },
    DeviceAccepted {
        device_key: String,
        protocol: String,
        point_count: usize,
    },
    DiscoveryScanComplete {
        protocol: String,
        device_count: usize,
    },
}

const BUS_CAPACITY: usize = 4096;

/// Broadcast-based event bus. Arc<Event> avoids cloning large payloads.
#[derive(Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Arc<Event>>,
}

impl EventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(BUS_CAPACITY);
        EventBus { tx }
    }

    pub fn publish(&self, event: Event) {
        // Ignore send errors (no subscribers is OK)
        let _ = self.tx.send(Arc::new(event));
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>> {
        self.tx.subscribe()
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn publish_subscribe_roundtrip() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(Event::ValueChanged {
            node_id: "ahu-1/dat".into(),
            value: PointValue::Float(72.5),
            timestamp_ms: 1000,
        });

        let event = rx.recv().await.unwrap();
        match event.as_ref() {
            Event::ValueChanged { node_id, .. } => {
                assert_eq!(node_id, "ahu-1/dat");
            }
            _ => panic!("wrong event type"),
        }
    }

    #[tokio::test]
    async fn multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        bus.publish(Event::AlarmRaised {
            alarm_id: 42,
            node_id: "vav-1/zat".into(),
        });

        assert!(rx1.recv().await.is_ok());
        assert!(rx2.recv().await.is_ok());
    }

    #[test]
    fn no_subscribers_is_ok() {
        let bus = EventBus::new();
        // Should not panic
        bus.publish(Event::DeviceDiscovered {
            bridge_type: "bacnet".into(),
            device_key: "bacnet-1000".into(),
        });
    }
}
