use crate::{PtyExitEvent, PtyOutputEvent};
use serde::Serialize;
use serialport::{available_ports, DataBits, FlowControl, Parity, SerialPort, StopBits, SerialPortType};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;

struct SerialSession {
    writer: Box<dyn SerialPort>,
    stop_flag: Arc<AtomicBool>,
}

#[derive(Clone, Default)]
pub struct SerialState {
    sessions: Arc<Mutex<HashMap<String, SerialSession>>>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SerialPortInfo {
    pub port_name: String,
    pub port_type: String,
    pub manufacturer: Option<String>,
    pub serial_number: Option<String>,
    pub vid: Option<u16>,
    pub pid: Option<u16>,
}

#[derive(Clone, Serialize)]
struct SerialConnectedEvent {
    id: String,
}

fn parse_data_bits(bits: u8) -> Result<DataBits, String> {
    match bits {
        5 => Ok(DataBits::Five),
        6 => Ok(DataBits::Six),
        7 => Ok(DataBits::Seven),
        8 => Ok(DataBits::Eight),
        _ => Err(format!("Unsupported data bits: {}", bits)),
    }
}

fn parse_stop_bits(bits: u8) -> Result<StopBits, String> {
    match bits {
        1 => Ok(StopBits::One),
        2 => Ok(StopBits::Two),
        _ => Err(format!("Unsupported stop bits: {}", bits)),
    }
}

fn parse_parity(parity: &str) -> Result<Parity, String> {
    match parity {
        "none" => Ok(Parity::None),
        "odd" => Ok(Parity::Odd),
        "even" => Ok(Parity::Even),
        _ => Err(format!("Unsupported parity: {}", parity)),
    }
}

fn parse_flow_control(flow_control: &str) -> Result<FlowControl, String> {
    match flow_control {
        "none" => Ok(FlowControl::None),
        "hardware" => Ok(FlowControl::Hardware),
        "software" => Ok(FlowControl::Software),
        _ => Err(format!("Unsupported flow control: {}", flow_control)),
    }
}

#[tauri::command]
pub fn list_serial_ports() -> Result<Vec<SerialPortInfo>, String> {
    let ports = available_ports().map_err(|e| e.to_string())?;
    let mapped = ports
        .into_iter()
        .map(|port| {
            let (port_type, manufacturer, serial_number, vid, pid) = match port.port_type {
                SerialPortType::UsbPort(info) => (
                    "usb".to_string(),
                    info.manufacturer,
                    info.serial_number,
                    Some(info.vid),
                    Some(info.pid),
                ),
                SerialPortType::BluetoothPort => ("bluetooth".to_string(), None, None, None, None),
                SerialPortType::PciPort => ("pci".to_string(), None, None, None, None),
                SerialPortType::Unknown => ("unknown".to_string(), None, None, None, None),
            };

            SerialPortInfo {
                port_name: port.port_name,
                port_type,
                manufacturer,
                serial_number,
                vid,
                pid,
            }
        })
        .collect();
    Ok(mapped)
}

#[tauri::command]
pub async fn start_serial_session(
    app: AppHandle,
    state: State<'_, SerialState>,
    id: String,
    port_name: String,
    baud_rate: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: String,
    flow_control: String,
) -> Result<(), String> {
    let port = serialport::new(&port_name, baud_rate)
        .timeout(Duration::from_millis(100))
        .data_bits(parse_data_bits(data_bits)?)
        .stop_bits(parse_stop_bits(stop_bits)?)
        .parity(parse_parity(&parity)?)
        .flow_control(parse_flow_control(&flow_control)?)
        .open()
        .map_err(|e| e.to_string())?;

    let reader = port.try_clone().map_err(|e| e.to_string())?;
    let stop_flag = Arc::new(AtomicBool::new(false));

    {
        let mut guard = state.sessions.lock().await;
        guard.insert(
            id.clone(),
            SerialSession {
                writer: port,
                stop_flag: stop_flag.clone(),
            },
        );
    }

    let _ = app.emit(
        "serial-connected",
        SerialConnectedEvent {
            id: id.clone(),
        },
    );

    let app_handle = app.clone();
    let session_id = id.clone();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut buffer = [0_u8; 4096];

        while !stop_flag.load(Ordering::Relaxed) {
            match reader.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    let output = String::from_utf8_lossy(&buffer[..size]).to_string();
                    let _ = app_handle.emit(
                        "pty-output",
                        PtyOutputEvent {
                            id: session_id.clone(),
                            data: output,
                        },
                    );
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => {
                    if !stop_flag.load(Ordering::Relaxed) {
                        let _ = app_handle.emit(
                            "pty-exit",
                            PtyExitEvent {
                                id: session_id.clone(),
                                message: format!("Serial read error: {}", error),
                            },
                        );
                    }
                    break;
                }
            }
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn write_serial_input(
    state: State<'_, SerialState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let mut guard = state.sessions.lock().await;
    let Some(session) = guard.get_mut(&id) else {
        return Err("Serial session not found".to_string());
    };

    session
        .writer
        .write_all(data.as_bytes())
        .map_err(|e| e.to_string())?;
    session.writer.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn close_serial_session(
    state: State<'_, SerialState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.sessions.lock().await;
    if let Some(session) = guard.remove(&id) {
        session.stop_flag.store(true, Ordering::Relaxed);
    }
    Ok(())
}
