use std::time::Duration;

use rustmod_client::{ModbusClient, UnitId};
use rustmod_datalink::ModbusTcpTransport;
use tokio::task::JoinHandle;

use crate::config::loader::LoadedDevice;
use crate::config::profile::{
    ByteOrder, ModbusDataType, ModbusPointMapping, ModbusRegisterType, PointAccess, PointValue,
};
use crate::store::point_store::{PointKey, PointStatusFlags, PointStore};

use super::traits::BridgeError;

// ---------------------------------------------------------------------------
// Config types built from loaded scenario
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct ModbusDeviceConfig {
    instance_id: String,
    host: String,
    port: u16,
    unit_id: u8,
    byte_order: ByteOrder,
    word_order: ByteOrder,
    points: Vec<ModbusPointConfig>,
}

#[derive(Debug, Clone)]
struct ModbusPointConfig {
    point_id: String,
    access: PointAccess,
    mapping: ModbusPointMapping,
}

// ---------------------------------------------------------------------------
// ModbusBridge — client-side Modbus TCP integration
// ---------------------------------------------------------------------------

pub struct ModbusBridge {
    poll_interval: Duration,
    devices: Vec<ModbusDeviceConfig>,
    poll_handles: Vec<JoinHandle<()>>,
    store: Option<PointStore>,
}

impl Default for ModbusBridge {
    fn default() -> Self {
        Self::new()
    }
}

impl ModbusBridge {
    pub fn new() -> Self {
        ModbusBridge {
            poll_interval: Duration::from_secs(10),
            devices: Vec::new(),
            poll_handles: Vec::new(),
            store: None,
        }
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Build device configs from loaded scenario devices.
    /// Only includes devices that have Modbus mappings AND a configured host.
    pub fn from_loaded_devices(mut self, devices: &[LoadedDevice]) -> Self {
        self.devices = devices
            .iter()
            .filter_map(|dev| {
                let defaults = dev.profile.defaults.as_ref()?.protocols.as_ref()?.modbus.as_ref()?;
                let host = defaults.host.as_ref()?;

                let points: Vec<ModbusPointConfig> = dev
                    .profile
                    .points
                    .iter()
                    .filter_map(|pt| {
                        let mapping = pt.protocols.as_ref()?.modbus.clone()?;
                        Some(ModbusPointConfig {
                            point_id: pt.id.clone(),
                            access: pt.access.clone(),
                            mapping,
                        })
                    })
                    .collect();

                if points.is_empty() {
                    return None;
                }

                Some(ModbusDeviceConfig {
                    instance_id: dev.instance_id.clone(),
                    host: host.clone(),
                    port: defaults.port.unwrap_or(502),
                    unit_id: defaults.unit_id.unwrap_or(1),
                    byte_order: defaults.byte_order.clone().unwrap_or(ByteOrder::BigEndian),
                    word_order: defaults.word_order.clone().unwrap_or(ByteOrder::BigEndian),
                    points,
                })
            })
            .collect();

        self
    }

    pub fn device_count(&self) -> usize {
        self.devices.len()
    }

    pub fn point_count(&self) -> usize {
        self.devices.iter().map(|d| d.points.len()).sum()
    }
}

// ---------------------------------------------------------------------------
// PointSource implementation
// ---------------------------------------------------------------------------

impl super::traits::PointSource for ModbusBridge {
    async fn start(&mut self, store: PointStore) -> Result<(), BridgeError> {
        self.store = Some(store.clone());

        if self.devices.is_empty() {
            println!("Modbus: no devices configured with host addresses.");
            return Ok(());
        }

        println!(
            "Modbus: connecting to {} device(s)...",
            self.devices.len()
        );

        for dev_config in &self.devices {
            let addr = format!("{}:{}", dev_config.host, dev_config.port);
            let transport = match ModbusTcpTransport::connect(&addr).await {
                Ok(t) => t,
                Err(e) => {
                    println!(
                        "  {} — connection failed to {}: {e}",
                        dev_config.instance_id, addr
                    );
                    continue;
                }
            };

            let client = ModbusClient::new(transport);
            let unit_id = UnitId::new(dev_config.unit_id);

            // Initial read of all points
            let point_count = dev_config.points.len();
            let mut success_count = 0;
            for pt in &dev_config.points {
                match read_point(&client, unit_id, &pt.mapping, &dev_config.byte_order, &dev_config.word_order).await {
                    Ok(value) => {
                        let key = PointKey {
                            device_instance_id: dev_config.instance_id.clone(),
                            point_id: pt.point_id.clone(),
                        };
                        store.set(key, value);
                        success_count += 1;
                    }
                    Err(e) => {
                        println!(
                            "  {} point {} — read failed: {e}",
                            dev_config.instance_id, pt.point_id
                        );
                    }
                }
            }

            println!(
                "  {} — connected to {}, {}/{} points read",
                dev_config.instance_id, addr, success_count, point_count
            );

            // Spawn poll loop for this device
            let poll_store = store.clone();
            let poll_config = dev_config.clone();
            let poll_interval = self.poll_interval;

            let handle = tokio::spawn(async move {
                poll_device(client, unit_id, poll_config, poll_store, poll_interval).await;
            });
            self.poll_handles.push(handle);
        }

        let total_points: usize = self.devices.iter().map(|d| d.points.len()).sum();
        println!(
            "Modbus: monitoring {} device(s), {} point(s)",
            self.poll_handles.len(),
            total_points,
        );

        Ok(())
    }

    async fn stop(&mut self) -> Result<(), BridgeError> {
        for h in self.poll_handles.drain(..) {
            h.abort();
        }
        Ok(())
    }

    async fn write_point(
        &self,
        device_id: &str,
        point_id: &str,
        value: PointValue,
        _priority: Option<u8>,
    ) -> Result<(), BridgeError> {
        // We don't keep client handles around for writes in this version.
        // A production implementation would maintain persistent connections.
        let dev = self
            .devices
            .iter()
            .find(|d| d.instance_id == device_id)
            .ok_or_else(|| BridgeError::PointNotFound {
                device_id: device_id.to_string(),
                point_id: point_id.to_string(),
            })?;

        let pt = dev
            .points
            .iter()
            .find(|p| p.point_id == point_id)
            .ok_or_else(|| BridgeError::PointNotFound {
                device_id: device_id.to_string(),
                point_id: point_id.to_string(),
            })?;

        if matches!(pt.access, PointAccess::Input) {
            return Err(BridgeError::WriteRejected(format!(
                "Point {} is read-only (input)",
                point_id
            )));
        }

        let addr = format!("{}:{}", dev.host, dev.port);
        let transport = ModbusTcpTransport::connect(&addr)
            .await
            .map_err(|e| BridgeError::ConnectionFailed(format!("Modbus connect: {e}")))?;
        let client = ModbusClient::new(transport);
        let unit_id = UnitId::new(dev.unit_id);

        write_point(&client, unit_id, &pt.mapping, &value, &dev.byte_order, &dev.word_order)
            .await
            .map_err(|e| BridgeError::Protocol(format!("Modbus write failed: {e}")))?;

        // Update PointStore immediately so value is reflected without waiting for next poll
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

// ---------------------------------------------------------------------------
// Poll loop
// ---------------------------------------------------------------------------

async fn poll_device(
    client: ModbusClient<ModbusTcpTransport>,
    unit_id: UnitId,
    config: ModbusDeviceConfig,
    store: PointStore,
    interval: Duration,
) {
    let mut device_down = false;
    loop {
        tokio::time::sleep(interval).await;

        let mut any_error = false;
        for pt in &config.points {
            let key = PointKey {
                device_instance_id: config.instance_id.clone(),
                point_id: pt.point_id.clone(),
            };
            match read_point(&client, unit_id, &pt.mapping, &config.byte_order, &config.word_order).await {
                Ok(value) => {
                    store.set(key.clone(), value);
                    store.clear_status(&key, PointStatusFlags::FAULT);
                }
                Err(e) => {
                    eprintln!(
                        "Modbus: poll failed for {}.{}: {e}",
                        config.instance_id, pt.point_id
                    );
                    store.set_status(&key, PointStatusFlags::FAULT);
                    any_error = true;
                }
            }
        }

        // Set/clear DOWN at device level
        if any_error && !device_down {
            device_down = true;
            for pt in &config.points {
                let key = PointKey {
                    device_instance_id: config.instance_id.clone(),
                    point_id: pt.point_id.clone(),
                };
                store.set_status(&key, PointStatusFlags::DOWN);
            }
        } else if !any_error && device_down {
            device_down = false;
            for pt in &config.points {
                let key = PointKey {
                    device_instance_id: config.instance_id.clone(),
                    point_id: pt.point_id.clone(),
                };
                store.clear_status(&key, PointStatusFlags::DOWN);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Register read/write helpers
// ---------------------------------------------------------------------------

async fn read_point(
    client: &ModbusClient<ModbusTcpTransport>,
    unit_id: UnitId,
    mapping: &ModbusPointMapping,
    byte_order: &ByteOrder,
    word_order: &ByteOrder,
) -> Result<PointValue, String> {
    let data_type = mapping.data_type.clone().unwrap_or(default_data_type(&mapping.register_type));
    let scale = mapping.scale.unwrap_or(1.0);

    match mapping.register_type {
        ModbusRegisterType::Coil => {
            let values = client
                .read_coils(unit_id, mapping.address, 1)
                .await
                .map_err(|e| e.to_string())?;
            Ok(PointValue::Bool(values[0]))
        }
        ModbusRegisterType::DiscreteInput => {
            let values = client
                .read_discrete_inputs(unit_id, mapping.address, 1)
                .await
                .map_err(|e| e.to_string())?;
            Ok(PointValue::Bool(values[0]))
        }
        ModbusRegisterType::Holding => {
            let reg_count = register_count(&data_type, mapping.register_count);
            let regs = client
                .read_holding_registers(unit_id, mapping.address, reg_count)
                .await
                .map_err(|e| e.to_string())?;
            Ok(decode_registers(&regs, &data_type, scale, byte_order, word_order))
        }
        ModbusRegisterType::Input => {
            let reg_count = register_count(&data_type, mapping.register_count);
            let regs = client
                .read_input_registers(unit_id, mapping.address, reg_count)
                .await
                .map_err(|e| e.to_string())?;
            Ok(decode_registers(&regs, &data_type, scale, byte_order, word_order))
        }
    }
}

async fn write_point(
    client: &ModbusClient<ModbusTcpTransport>,
    unit_id: UnitId,
    mapping: &ModbusPointMapping,
    value: &PointValue,
    byte_order: &ByteOrder,
    word_order: &ByteOrder,
) -> Result<(), String> {
    let data_type = mapping.data_type.clone().unwrap_or(default_data_type(&mapping.register_type));
    let scale = mapping.scale.unwrap_or(1.0);

    match mapping.register_type {
        ModbusRegisterType::Coil => {
            let bool_val = match value {
                PointValue::Bool(b) => *b,
                PointValue::Integer(i) => *i != 0,
                PointValue::Float(f) => *f != 0.0,
            };
            client
                .write_single_coil(unit_id, mapping.address, bool_val)
                .await
                .map_err(|e| e.to_string())
        }
        ModbusRegisterType::Holding => {
            let regs = encode_registers(value, &data_type, scale, byte_order, word_order);
            if regs.len() == 1 {
                client
                    .write_single_register(unit_id, mapping.address, regs[0])
                    .await
                    .map_err(|e| e.to_string())
            } else {
                client
                    .write_multiple_registers(unit_id, mapping.address, &regs)
                    .await
                    .map_err(|e| e.to_string())
            }
        }
        ModbusRegisterType::DiscreteInput | ModbusRegisterType::Input => {
            Err("Cannot write to input registers".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Data conversion helpers
// ---------------------------------------------------------------------------

fn default_data_type(reg_type: &ModbusRegisterType) -> ModbusDataType {
    match reg_type {
        ModbusRegisterType::Coil | ModbusRegisterType::DiscreteInput => ModbusDataType::Bool,
        ModbusRegisterType::Holding | ModbusRegisterType::Input => ModbusDataType::Uint16,
    }
}

fn register_count(data_type: &ModbusDataType, explicit: Option<u16>) -> u16 {
    explicit.unwrap_or(match data_type {
        ModbusDataType::Bool | ModbusDataType::Uint16 | ModbusDataType::Int16 => 1,
        ModbusDataType::Uint32 | ModbusDataType::Int32 | ModbusDataType::Float32 => 2,
        ModbusDataType::Float64 => 4,
    })
}

fn decode_registers(
    regs: &[u16],
    data_type: &ModbusDataType,
    scale: f64,
    byte_order: &ByteOrder,
    word_order: &ByteOrder,
) -> PointValue {
    // Apply byte-swap within each register if device uses little-endian byte order
    let regs: Vec<u16> = if matches!(byte_order, ByteOrder::LittleEndian) {
        regs.iter().map(|r| r.swap_bytes()).collect()
    } else {
        regs.to_vec()
    };
    let regs = &regs;
    match data_type {
        ModbusDataType::Bool => PointValue::Bool(regs.first().copied().unwrap_or(0) != 0),
        ModbusDataType::Uint16 => {
            let raw = regs.first().copied().unwrap_or(0) as f64;
            PointValue::Float(raw / scale)
        }
        ModbusDataType::Int16 => {
            let raw = regs.first().copied().unwrap_or(0) as i16 as f64;
            PointValue::Float(raw / scale)
        }
        ModbusDataType::Uint32 => {
            let raw = assemble_u32(regs, word_order) as f64;
            PointValue::Float(raw / scale)
        }
        ModbusDataType::Int32 => {
            let raw = assemble_u32(regs, word_order) as i32 as f64;
            PointValue::Float(raw / scale)
        }
        ModbusDataType::Float32 => {
            let bits = assemble_u32(regs, word_order);
            let raw = f32::from_bits(bits) as f64;
            PointValue::Float(raw / scale)
        }
        ModbusDataType::Float64 => {
            let bits = assemble_u64(regs, word_order);
            let raw = f64::from_bits(bits);
            PointValue::Float(raw / scale)
        }
    }
}

fn encode_registers(
    value: &PointValue,
    data_type: &ModbusDataType,
    scale: f64,
    byte_order: &ByteOrder,
    word_order: &ByteOrder,
) -> Vec<u16> {
    let raw = value.as_f64() * scale;

    let mut regs = match data_type {
        ModbusDataType::Bool => vec![if raw != 0.0 { 1 } else { 0 }],
        ModbusDataType::Uint16 => vec![raw as u16],
        ModbusDataType::Int16 => vec![(raw as i16) as u16],
        ModbusDataType::Uint32 => split_u32(raw as u32, word_order),
        ModbusDataType::Int32 => split_u32((raw as i32) as u32, word_order),
        ModbusDataType::Float32 => split_u32((raw as f32).to_bits(), word_order),
        ModbusDataType::Float64 => split_u64(raw.to_bits(), word_order),
    };

    // Apply byte-swap within each register if device uses little-endian byte order
    if matches!(byte_order, ByteOrder::LittleEndian) {
        for r in &mut regs {
            *r = r.swap_bytes();
        }
    }

    regs
}

/// Assemble two u16 registers into a u32, respecting word order.
fn assemble_u32(regs: &[u16], word_order: &ByteOrder) -> u32 {
    if regs.len() < 2 {
        return regs.first().copied().unwrap_or(0) as u32;
    }
    match word_order {
        ByteOrder::BigEndian => (regs[0] as u32) << 16 | regs[1] as u32,
        ByteOrder::LittleEndian => (regs[1] as u32) << 16 | regs[0] as u32,
    }
}

/// Assemble four u16 registers into a u64, respecting word order.
fn assemble_u64(regs: &[u16], word_order: &ByteOrder) -> u64 {
    if regs.len() < 4 {
        return assemble_u32(regs, word_order) as u64;
    }
    match word_order {
        ByteOrder::BigEndian => {
            (regs[0] as u64) << 48 | (regs[1] as u64) << 32 | (regs[2] as u64) << 16 | regs[3] as u64
        }
        ByteOrder::LittleEndian => {
            (regs[3] as u64) << 48 | (regs[2] as u64) << 32 | (regs[1] as u64) << 16 | regs[0] as u64
        }
    }
}

/// Split a u32 into two u16 registers, respecting word order.
fn split_u32(val: u32, word_order: &ByteOrder) -> Vec<u16> {
    let hi = (val >> 16) as u16;
    let lo = val as u16;
    match word_order {
        ByteOrder::BigEndian => vec![hi, lo],
        ByteOrder::LittleEndian => vec![lo, hi],
    }
}

/// Split a u64 into four u16 registers, respecting word order.
fn split_u64(val: u64, word_order: &ByteOrder) -> Vec<u16> {
    let w0 = (val >> 48) as u16;
    let w1 = (val >> 32) as u16;
    let w2 = (val >> 16) as u16;
    let w3 = val as u16;
    match word_order {
        ByteOrder::BigEndian => vec![w0, w1, w2, w3],
        ByteOrder::LittleEndian => vec![w3, w2, w1, w0],
    }
}
