use serde::Serialize;
use serialport::{available_ports, SerialPortType};
use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
use tauri::{AppHandle, Emitter, State};
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

use crate::connection_test::{
    tcp_connect, ConnectionTestReport, ProbeFailure, TestCode, TCP_CONNECT_TIMEOUT,
};
use crate::serial_link::{
    connect_network, local_port_builder, open_local, SerialLink, SerialSink, SinkSlot,
};
use crate::serial_params::{PurgeTarget, SerialParams, SerialStatus, SerialTransport};

/// How long to wait for the peer to answer `WILL COM-PORT-OPTION` before
/// declaring the session degraded and carrying on as a plain byte pipe.
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(2);
const NEGOTIATION_POLL: Duration = Duration::from_millis(25);

/// How long the connection test waits for a local port to open. Bluetooth
/// serial ports in particular can sit in `open()` for a long time.
const LOCAL_OPEN_TIMEOUT: Duration = Duration::from_secs(5);

/// Smallest gap between two `serial-status` events.
///
/// Some device server firmware pushes `NOTIFY-MODEMSTATE` dozens of times a
/// second. Coalescing here keeps that off the IPC channel and out of Vue's
/// reactivity graph; the frontend only ever needs the latest value.
const STATUS_THROTTLE: Duration = Duration::from_millis(50);

/// How often to ask a local UART for its modem lines. They are only readable by
/// polling, and CTS/DSR/CD do not move fast enough to justify doing it per read.
const MODEM_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// BREAK hold time when the caller does not specify one. A quarter second is the
/// conventional "long break" that bootloaders and ROM monitors look for.
const DEFAULT_BREAK_MILLIS: u32 = 250;

/// Backoff schedule for reconnecting a dropped network session, in seconds.
/// The last value repeats: a device server can be down for a long while, and a
/// console that comes back on its own is the entire point.
const RECONNECT_BACKOFF_SECS: [u64; 6] = [1, 2, 4, 8, 15, 30];

struct SerialSession {
    /// Indirected so a reconnect can swap the socket underneath every writer.
    sink: SinkSlot,
    stop_flag: Arc<AtomicBool>,
}

/// Everything needed to rebuild a dropped network session.
#[derive(Clone)]
struct NetworkTarget {
    host: String,
    port: u16,
    transport: SerialTransport,
    params: SerialParams,
    adopt_server_params: bool,
}

#[derive(Clone, Default)]
pub struct SerialState {
    sessions: Arc<Mutex<HashMap<String, SerialSession>>>,
    hub: Arc<crate::terminal_event_hub::TerminalEventHub>,
}

impl SerialState {
    pub fn new(hub: Arc<crate::terminal_event_hub::TerminalEventHub>) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            hub,
        }
    }

    pub async fn contains(&self, id: &str) -> bool {
        self.sessions.lock().await.contains_key(id)
    }

    /// Look up a session's write half.
    ///
    /// The `Arc` is cloned out and the map lock released before any IO, so a
    /// slow or blocked port cannot stall every other serial session.
    async fn sink(&self, id: &str) -> Result<Arc<dyn SerialSink>, String> {
        let guard = self.sessions.lock().await;
        Ok(guard
            .get(id)
            .ok_or_else(|| "Serial session not found".to_string())?
            .sink
            .get())
    }

    pub async fn write_bytes(&self, id: &str, data: &[u8]) -> Result<(), String> {
        self.sink(id).await?.write_data(data)
    }
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SerialReconnectingEvent {
    id: String,
    attempt: u32,
    delay_ms: u64,
    message: String,
}

/// Write a line straight into the session's terminal.
///
/// Reconnect progress belongs in the scrollback next to the output it
/// interrupts, and it must not go out as `pty-exit` — the frontend treats that
/// as the session ending for good.
fn emit_notice(app: &AppHandle, id: &str, text: &str) {
    let _ = app.emit(
        &crate::util::session_event("pty-output", id),
        crate::PtyOutputEvent {
            id: id.to_string(),
            data: format!("\r\n\x1b[33m{text}\x1b[0m\r\n"),
        },
    );
}

fn emit_serial_status(app: &AppHandle, status: &SerialStatus) {
    let _ = app.emit(
        &crate::util::session_event("serial-status", &status.id),
        status,
    );
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

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_serial_session(
    app: AppHandle,
    state: State<'_, SerialState>,
    zmodem: State<'_, crate::zmodem::ZmodemState>,
    id: String,
    port_name: String,
    baud_rate: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: String,
    flow_control: String,
    transport: Option<String>,
    host: Option<String>,
    net_port: Option<u16>,
    adopt_server_params: Option<bool>,
    auto_reconnect: Option<bool>,
) -> Result<(), String> {
    let transport = SerialTransport::parse(transport.as_deref())?;
    let params = SerialParams::from_wire(baud_rate, data_bits, stop_bits, &parity, &flow_control)?;

    let mut target: Option<NetworkTarget> = None;
    let (link, sink, net_sink) = if transport.is_network() {
        let host = host
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "A network serial session needs a host".to_string())?;
        let net_port = net_port.filter(|port| *port > 0).ok_or_else(|| {
            "A network serial session needs a port (2217 is the RFC 2217 default)".to_string()
        })?;

        let adopt = adopt_server_params.unwrap_or(false);
        let (link, net_sink) = connect_network(host, net_port, transport, params, adopt)?;
        if auto_reconnect.unwrap_or(true) {
            target = Some(NetworkTarget {
                host: host.to_string(),
                port: net_port,
                transport,
                params,
                adopt_server_params: adopt,
            });
        }
        let sink: Arc<dyn SerialSink> = net_sink.clone();
        (link, sink, Some(net_sink))
    } else {
        let (link, sink) = open_local(&port_name, params)?;
        (link, sink, None)
    };

    let stop_flag = Arc::new(AtomicBool::new(false));
    let slot = SinkSlot::new(sink.clone());

    {
        let mut guard = state.sessions.lock().await;
        guard.insert(
            id.clone(),
            SerialSession {
                sink: slot.clone(),
                stop_flag: stop_flag.clone(),
            },
        );
    }

    let zmodem_state = zmodem.inner().clone();
    zmodem_state.reset_session(&id);
    spawn_reader(
        app.clone(),
        id.clone(),
        link,
        slot,
        stop_flag,
        state.hub.clone(),
        zmodem_state,
        target,
    );

    // The reader has to be running before the handshake can complete, so the
    // verdict is awaited here rather than inside `connect_network`.
    if let Some(net_sink) = &net_sink {
        let deadline = tokio::time::Instant::now() + NEGOTIATION_TIMEOUT;
        while !net_sink.negotiation_settled() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(NEGOTIATION_POLL).await;
        }
        // Silence is a verdict too. Without this the status below would say
        // "still waiting" forever against a peer that ignores the option.
        net_sink.give_up_negotiation();
    }

    // Emitted after the handshake settles so the frontend never sees a session
    // flip from "connected" to "degraded" a beat later.
    let _ = app.emit(
        &crate::util::session_event("serial-connected", &id),
        SerialConnectedEvent { id: id.clone() },
    );
    emit_serial_status(&app, &sink.status(&id));

    Ok(())
}

/// Sleep, but wake early when the session is closed.
///
/// Returns false when the caller should stop. Without this a session closed
/// during a 30-second backoff would leave its thread alive until the sleep
/// happened to end.
fn sleep_unless_stopped(duration: Duration, stop_flag: &AtomicBool) -> bool {
    let deadline = std::time::Instant::now() + duration;
    while !stop_flag.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(100)));
    }
    false
}

/// Delay before the nth reconnect attempt. The last step repeats forever: a
/// device server can be down a long while, and a console that comes back on its
/// own is the entire point of the feature.
fn reconnect_backoff(attempt: u32) -> Duration {
    let index = attempt.max(1) as usize - 1;
    Duration::from_secs(RECONNECT_BACKOFF_SECS[index.min(RECONNECT_BACKOFF_SECS.len() - 1)])
}

/// What a reconnect attempt is doing, for whoever is watching.
enum ReconnectProgress<'a> {
    Waiting { attempt: u32, delay: Duration },
    Failed { error: &'a str },
    Recovered,
}

/// Rebuild a dropped network session, retrying with backoff until it comes back
/// or the session is closed.
///
/// Reporting is a callback rather than an `AppHandle` so the retry logic — the
/// part with the off-by-one risk and the stop-flag race — can be exercised
/// without a Tauri runtime.
fn reconnect(
    target: &NetworkTarget,
    stop_flag: &AtomicBool,
    mut on_progress: impl FnMut(ReconnectProgress),
) -> Option<(Box<dyn SerialLink>, Arc<dyn SerialSink>)> {
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let delay = reconnect_backoff(attempt);
        on_progress(ReconnectProgress::Waiting { attempt, delay });

        if !sleep_unless_stopped(delay, stop_flag) {
            return None;
        }

        match connect_network(
            &target.host,
            target.port,
            target.transport,
            target.params,
            target.adopt_server_params,
        ) {
            Ok((link, net_sink)) => {
                on_progress(ReconnectProgress::Recovered);
                return Some((link, net_sink));
            }
            // Keep the reason visible: "connection refused" and "no route to
            // host" call for very different next steps.
            Err(error) => on_progress(ReconnectProgress::Failed { error: &error }),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_reader(
    app: AppHandle,
    session_id: String,
    mut link: Box<dyn SerialLink>,
    slot: SinkSlot,
    stop_flag: Arc<AtomicBool>,
    event_hub: Arc<crate::terminal_event_hub::TerminalEventHub>,
    zmodem_state: crate::zmodem::ZmodemState,
    target: Option<NetworkTarget>,
) {
    std::thread::spawn(move || {
        let mut sink = slot.get();
        let mut buffer = [0_u8; 4096];
        let mut decoder = crate::util::Utf8StreamDecoder::new();
        let mut pending_status: Option<SerialStatus> = None;
        let mut last_status_emit = std::time::Instant::now() - STATUS_THROTTLE;
        let mut last_modem_poll = std::time::Instant::now();
        // Set only after a reconnect: the initial handshake is timed by
        // `start_serial_session`, which has to wait for it anyway.
        let mut negotiation_deadline: Option<std::time::Instant> = None;

        while !stop_flag.load(Ordering::Relaxed) {
            match link.read(&mut buffer) {
                Ok(size) if size > 0 => {
                    event_hub.publish(
                        &session_id,
                        &crate::terminal_event_hub::TerminalEvent::Output(buffer[..size].to_vec()),
                    );
                    let (_, response) = crate::util::pump_stream_chunk(
                        &app,
                        &session_id,
                        &mut decoder,
                        &buffer[..size],
                        &zmodem_state,
                    );
                    if !response.is_empty() {
                        let _ = sink.write_data(&response);
                    }
                }
                // Not end of stream: a read timeout, or a chunk that held only
                // protocol negotiation.
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => {
                    if stop_flag.load(Ordering::Relaxed) {
                        break;
                    }

                    if let Some(target) = target.as_ref() {
                        let notify_app = app.clone();
                        let notify_id = session_id.clone();
                        let notify_host = target.host.clone();
                        let notify_port = target.port;
                        let outcome = reconnect(target, &stop_flag, move |progress| {
                            match progress {
                                ReconnectProgress::Waiting { attempt, delay } => {
                                    let message = format!(
                                        "[Link lost] reconnecting to {notify_host}:{notify_port} \
                                         in {}s (attempt {attempt})",
                                        delay.as_secs(),
                                    );
                                    emit_notice(&notify_app, &notify_id, &message);
                                    let _ = notify_app.emit(
                                        &crate::util::session_event(
                                            "serial-reconnecting",
                                            &notify_id,
                                        ),
                                        SerialReconnectingEvent {
                                            id: notify_id.clone(),
                                            attempt,
                                            delay_ms: delay.as_millis() as u64,
                                            message,
                                        },
                                    );
                                }
                                ReconnectProgress::Failed { error } => emit_notice(
                                    &notify_app,
                                    &notify_id,
                                    &format!("[Reconnect failed] {error}"),
                                ),
                                ReconnectProgress::Recovered => {
                                    emit_notice(&notify_app, &notify_id, "[Reconnected]")
                                }
                            }
                        });
                        let Some((new_link, new_sink)) = outcome else {
                            break;
                        };
                        link = new_link;
                        // Swap before anything else: a command that fires during
                        // the handover must reach the new socket, not the dead one.
                        slot.replace(new_sink.clone());
                        sink = new_sink;
                        // A half-read multi-byte character and a mid-flight
                        // transfer both belong to the connection that just died.
                        decoder = crate::util::Utf8StreamDecoder::new();
                        zmodem_state.reset_session(&session_id);
                        pending_status = None;
                        // The new socket has to redo the handshake. Give it the
                        // same grace the first connection got, so the status
                        // below reads "still settling" rather than "refused".
                        negotiation_deadline = Some(std::time::Instant::now() + NEGOTIATION_TIMEOUT);
                        let _ = app.emit(
                            &crate::util::session_event("serial-connected", &session_id),
                            SerialConnectedEvent { id: session_id.clone() },
                        );
                        emit_serial_status(&app, &sink.status(&session_id));
                        continue;
                    }

                    crate::util::emit_pty_exit(
                        &app,
                        &session_id,
                        format!("Serial read error: {}", error),
                    );
                    event_hub.publish(
                        &session_id,
                        &crate::terminal_event_hub::TerminalEvent::Exit(error.to_string()),
                    );
                    break;
                }
            }

            if let Some(deadline) = negotiation_deadline {
                if std::time::Instant::now() >= deadline {
                    negotiation_deadline = None;
                    sink.give_up_negotiation();
                }
            }

            // A local UART only reports its modem lines when asked.
            if last_modem_poll.elapsed() >= MODEM_POLL_INTERVAL {
                last_modem_poll = std::time::Instant::now();
                sink.poll_modem_lines();
            }

            // Negotiation replies and notifications arrive interleaved with
            // data; report only when they changed something, and no faster than
            // the throttle allows. The newest snapshot always wins.
            if let Some(status) = sink.status_if_changed(&session_id) {
                pending_status = Some(status);
            }
            if pending_status.is_some() && last_status_emit.elapsed() >= STATUS_THROTTLE {
                if let Some(status) = pending_status.take() {
                    emit_serial_status(&app, &status);
                    last_status_emit = std::time::Instant::now();
                }
            }
        }
    });
}

#[tauri::command]
pub async fn write_serial_input(
    state: State<'_, SerialState>,
    id: String,
    data: String,
) -> Result<(), String> {
    state.write_bytes(&id, data.as_bytes()).await
}

#[tauri::command]
pub async fn write_serial_bytes(
    state: State<'_, SerialState>,
    id: String,
    data: Vec<u8>,
) -> Result<(), String> {
    state.write_bytes(&id, &data).await
}

/// Retune a live session.
///
/// Guessing a baud rate is the most common serial chore there is, and closing
/// the port for each guess throws away whatever the device printed in between.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn set_serial_params(
    app: AppHandle,
    state: State<'_, SerialState>,
    id: String,
    baud_rate: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: String,
    flow_control: String,
) -> Result<(), String> {
    let params = SerialParams::from_wire(baud_rate, data_bits, stop_bits, &parity, &flow_control)?;
    let sink = state.sink(&id).await?;
    sink.set_params(params)?;
    // Report straight away rather than waiting for the reader loop's next tick:
    // over RFC 2217 the server's confirmation lands later, and the UI should
    // show "asked" in the meantime.
    emit_serial_status(&app, &sink.status(&id));
    Ok(())
}

/// Hold BREAK on the line, then release it.
///
/// This is how you interrupt a device that is not reading characters at all —
/// U-Boot, Cisco ROMMON, a Solaris OK prompt.
#[tauri::command]
pub async fn send_serial_break(
    state: State<'_, SerialState>,
    id: String,
    duration_ms: Option<u32>,
) -> Result<(), String> {
    let sink = state.sink(&id).await?;
    let millis = duration_ms.unwrap_or(DEFAULT_BREAK_MILLIS);
    // The hold is a real sleep; keep it off the async runtime's threads.
    tokio::task::spawn_blocking(move || sink.send_break(millis))
        .await
        .map_err(|error| format!("BREAK task failed: {error}"))?
}

/// Drive DTR and/or RTS. Omitted lines are left alone.
///
/// Toggling DTR is how an ESP32 or Arduino is reset and put into its
/// bootloader, so this is a first-class control, not a diagnostic.
#[tauri::command]
pub async fn set_serial_signals(
    app: AppHandle,
    state: State<'_, SerialState>,
    id: String,
    dtr: Option<bool>,
    rts: Option<bool>,
) -> Result<(), String> {
    let sink = state.sink(&id).await?;
    sink.set_signals(dtr, rts)?;
    emit_serial_status(&app, &sink.status(&id));
    Ok(())
}

/// Discard buffered bytes, in one or both directions.
#[tauri::command]
pub async fn purge_serial_buffers(
    state: State<'_, SerialState>,
    id: String,
    target: Option<String>,
) -> Result<(), String> {
    let target = match target.as_deref() {
        Some(value) => PurgeTarget::parse(value)?,
        None => PurgeTarget::Both,
    };
    state.sink(&id).await?.purge(target)
}

#[tauri::command]
pub async fn get_serial_status(
    state: State<'_, SerialState>,
    id: String,
) -> Result<SerialStatus, String> {
    Ok(state.sink(&id).await?.status(&id))
}

/// Try the serial settings from the New Session form without opening a session.
///
/// - **Local:** open the port with these settings and close it again. That
///   proves the port exists, is free, and the driver takes the settings — not
///   that a device is attached or that the baud rate matches it.
/// - **Raw TCP:** connect and hang up without sending a byte.
/// - **RFC 2217:** connect, run the handshake in adopt mode (queries only, see
///   [`crate::serial_link::probe_rfc2217`]) and report what the server said.
///
/// Nothing is registered in `SerialState` and no event is emitted. Expected
/// failures come back as `Ok(report)`; `Err` means malformed parameters.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn serial_test_connection(
    port_name: String,
    baud_rate: u32,
    data_bits: u8,
    stop_bits: u8,
    parity: String,
    flow_control: String,
    transport: Option<String>,
    host: Option<String>,
    net_port: Option<u16>,
) -> Result<ConnectionTestReport, String> {
    let started = std::time::Instant::now();
    let transport = SerialTransport::parse(transport.as_deref())?;
    let params = SerialParams::from_wire(baud_rate, data_bits, stop_bits, &parity, &flow_control)?;

    if !transport.is_network() {
        return Ok(test_local_port(port_name, params, started).await);
    }

    // Same validation and trimming as `start_serial_session`.
    let host = host
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "A network serial session needs a host".to_string())?;
    let net_port = net_port.filter(|port| *port > 0).ok_or_else(|| {
        "A network serial session needs a port (2217 is the RFC 2217 default)".to_string()
    })?;
    let endpoint = format!("{host}:{net_port}");
    let stream = match tcp_connect((host, net_port), &endpoint, TCP_CONNECT_TIMEOUT).await {
        Ok(stream) => stream,
        Err(failure) => return Ok(ConnectionTestReport::from_failure(failure, started)),
    };

    if transport == SerialTransport::Rfc2217 {
        return Ok(test_rfc2217(stream, params, started).await);
    }

    // Raw TCP: nothing to negotiate, and any byte sent would reach the device.
    let mut stream = stream;
    let _ = stream.shutdown().await;
    Ok(ConnectionTestReport::new(TestCode::Ok, started))
}

async fn test_local_port(
    port_name: String,
    params: SerialParams,
    started: std::time::Instant,
) -> ConnectionTestReport {
    let open = tokio::task::spawn_blocking(move || probe_local_port(&port_name, params));
    match tokio::time::timeout(LOCAL_OPEN_TIMEOUT, open).await {
        Err(_) => ConnectionTestReport::new(TestCode::SerialOpenTimeout, started).with_detail(format!(
            "Opening the port did not finish within {}s",
            LOCAL_OPEN_TIMEOUT.as_secs(),
        )),
        Ok(Err(error)) => ConnectionTestReport::new(TestCode::SerialOpenFailed, started)
            .with_detail(format!("Port test task failed: {error}")),
        Ok(Ok(Err(failure))) => ConnectionTestReport::from_failure(failure, started),
        Ok(Ok(Ok(()))) => ConnectionTestReport::new(TestCode::Ok, started),
    }
}

/// Open `port_name` and close it again, on the calling (blocking) thread.
///
/// Only `open()`: no clone, no read or write, no DTR/RTS change beyond what
/// opening a port does on its own. The port is dropped here rather than handed
/// back, so an open that completes after the caller gave up still releases it.
fn probe_local_port(port_name: &str, params: SerialParams) -> Result<(), ProbeFailure> {
    let builder = local_port_builder(port_name, params)
        .map_err(|detail| ProbeFailure::new(TestCode::SerialParamsRejected, detail))?;
    match builder.open() {
        Ok(port) => {
            drop(port);
            Ok(())
        }
        Err(error) => {
            let listed = error.kind() == serialport::ErrorKind::NoDevice && port_is_listed(port_name);
            Err(ProbeFailure::new(
                classify_serial_open_error(error.kind(), listed),
                error.to_string(),
            ))
        }
    }
}

/// Whether the OS still enumerates `port_name`.
fn port_is_listed(port_name: &str) -> bool {
    available_ports()
        .map(|ports| ports.iter().any(|port| same_port_name(&port.port_name, port_name)))
        .unwrap_or(false)
}

/// Whether two port names refer to the same port. Windows opens `com3` as
/// `COM3`, so a hand-typed name must not turn a busy port into a missing one;
/// device paths elsewhere are case-sensitive.
fn same_port_name(listed: &str, requested: &str) -> bool {
    if cfg!(windows) {
        listed.eq_ignore_ascii_case(requested)
    } else {
        listed == requested
    }
}

/// Map an open failure onto what the user should do about it.
///
/// On Windows serialport folds "access denied" (someone else has the port) and
/// "file not found" (no such port) into the same `NoDevice`, and the OS text
/// that would tell them apart is localised. Whether the port is still listed
/// is what separates the two: a busy port is enumerated, a missing one is not.
pub(crate) fn classify_serial_open_error(kind: serialport::ErrorKind, listed: bool) -> TestCode {
    use serialport::ErrorKind;
    match kind {
        ErrorKind::NoDevice if listed => TestCode::SerialPortBusy,
        ErrorKind::NoDevice => TestCode::SerialPortNotFound,
        ErrorKind::Io(std::io::ErrorKind::NotFound) => TestCode::SerialPortNotFound,
        // Unix: not in the dialout/uucp group.
        ErrorKind::Io(std::io::ErrorKind::PermissionDenied) => TestCode::SerialPermissionDenied,
        ErrorKind::InvalidInput => TestCode::SerialParamsRejected,
        _ => TestCode::SerialOpenFailed,
    }
}

async fn test_rfc2217(
    stream: tokio::net::TcpStream,
    params: SerialParams,
    started: std::time::Instant,
) -> ConnectionTestReport {
    // The probe is blocking code shared with the session transport, so it gets
    // a std socket. `into_std` hands it over non-blocking; undo that.
    let stream = match stream
        .into_std()
        .and_then(|stream| stream.set_nonblocking(false).map(|()| stream))
    {
        Ok(stream) => stream,
        Err(error) => {
            return ConnectionTestReport::new(TestCode::ConnectFailed, started)
                .with_detail(error.to_string())
        }
    };

    let probe = tokio::task::spawn_blocking(move || {
        crate::serial_link::probe_rfc2217(stream, params, NEGOTIATION_TIMEOUT)
    });
    // The probe bounds itself; this is only a safety net over it.
    let outcome = tokio::time::timeout(NEGOTIATION_TIMEOUT + Duration::from_secs(1), probe).await;
    match outcome {
        Err(_) => ConnectionTestReport::new(TestCode::Timeout, started).with_detail(format!(
            "The RFC 2217 handshake did not finish within {}s",
            NEGOTIATION_TIMEOUT.as_secs(),
        )),
        // Same convention as the local-port path: a failed task is a failed
        // test, not a malformed request.
        Ok(Err(error)) => ConnectionTestReport::new(TestCode::ConnectFailed, started)
            .with_detail(format!("RFC 2217 test task failed: {error}")),
        Ok(Ok(Err(failure))) => ConnectionTestReport::from_failure(failure, started),
        Ok(Ok(Ok(probe))) if probe.negotiated => {
            let mut report = ConnectionTestReport::new(TestCode::Ok, started);
            report.server_params = Some(probe.server_params);
            report.signature = probe.signature;
            report
        }
        Ok(Ok(Ok(_))) => ConnectionTestReport::new(TestCode::Rfc2217Refused, started)
            .with_detail("Server did not accept COM-PORT-OPTION"),
    }
}

#[tauri::command]
pub async fn close_serial_session(
    state: State<'_, SerialState>,
    zmodem: State<'_, crate::zmodem::ZmodemState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.sessions.lock().await;
    if let Some(session) = guard.remove(&id) {
        session.stop_flag.store(true, Ordering::Relaxed);
    }
    zmodem.reset_session(&id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::time::Instant;

    fn raw_target(port: u16) -> NetworkTarget {
        NetworkTarget {
            host: "127.0.0.1".to_string(),
            port,
            // Raw TCP needs no handshake, so the test exercises the retry logic
            // rather than the protocol.
            transport: SerialTransport::RawTcp,
            params: SerialParams::default(),
            adopt_server_params: false,
        }
    }

    #[test]
    fn backoff_climbs_then_holds() {
        assert_eq!(reconnect_backoff(1), Duration::from_secs(1));
        assert_eq!(reconnect_backoff(2), Duration::from_secs(2));
        assert_eq!(reconnect_backoff(6), Duration::from_secs(30));
        // The schedule repeats rather than running off the end of the array.
        assert_eq!(reconnect_backoff(7), Duration::from_secs(30));
        assert_eq!(reconnect_backoff(9_999), Duration::from_secs(30));
        // Attempt numbering starts at 1; a 0 must not underflow the index.
        assert_eq!(reconnect_backoff(0), Duration::from_secs(1));
    }

    #[test]
    fn reconnects_once_the_peer_comes_back() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        // Hold the listener so the very first attempt succeeds.
        let accepted = std::thread::spawn(move || listener.accept().map(|_| ()));

        let stop = AtomicBool::new(false);
        let mut attempts = Vec::new();
        let outcome = reconnect(&raw_target(port), &stop, |progress| {
            if let ReconnectProgress::Waiting { attempt, delay } = progress {
                attempts.push((attempt, delay));
            }
        });

        assert!(outcome.is_some(), "expected the session to come back");
        assert_eq!(attempts, vec![(1, Duration::from_secs(1))]);
        assert!(accepted.join().expect("accept thread").is_ok());
    }

    #[test]
    fn retries_until_the_listener_appears() {
        // Reserve a port, then free it so the first attempt is refused.
        let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);

        // Bring the "device server" back while the client is backing off after
        // its first refusal. Waiting for that refusal rather than a fixed delay
        // matters on Windows: a refused loopback connect is retried for ~2s
        // before it fails, so a timer-driven rebind can land inside the first
        // attempt and let it succeed.
        let stop = Arc::new(AtomicBool::new(false));
        let (refused_tx, refused_rx) = std::sync::mpsc::channel::<()>();
        let give_up = stop.clone();
        let relisten = std::thread::spawn(move || {
            // Bounded so a broken `reconnect` fails the test instead of hanging it.
            if refused_rx.recv_timeout(Duration::from_secs(30)).is_err() {
                give_up.store(true, Ordering::Relaxed);
                return Err(std::io::Error::other("the first attempt was never refused"));
            }
            let listener = match TcpListener::bind(("127.0.0.1", port)) {
                Ok(listener) => listener,
                Err(error) => {
                    give_up.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            };
            listener.accept().map(|_| ())
        });

        let mut failures = 0;
        let outcome = reconnect(&raw_target(port), &stop, |progress| {
            if matches!(progress, ReconnectProgress::Failed { .. }) {
                failures += 1;
                let _ = refused_tx.send(());
            }
        });

        assert!(outcome.is_some(), "expected a later attempt to succeed");
        assert!(failures >= 1, "the first attempt should have been refused");
        assert!(relisten.join().expect("relisten thread").is_ok());
    }

    #[test]
    fn closing_the_session_stops_the_retry_immediately() {
        // A session closed during a 30-second backoff must not keep its thread
        // alive until the sleep happens to end.
        let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = probe.local_addr().expect("addr").port();
        drop(probe);

        let stop = AtomicBool::new(true);
        let started = Instant::now();
        let outcome = reconnect(&raw_target(port), &stop, |_| {});

        assert!(outcome.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "took {:?}, so the stop flag was not observed",
            started.elapsed(),
        );
    }

    #[test]
    fn sleep_wakes_early_when_the_session_closes() {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            flag.store(true, Ordering::Relaxed);
        });

        let started = Instant::now();
        assert!(!sleep_unless_stopped(Duration::from_secs(30), &stop));
        assert!(started.elapsed() < Duration::from_secs(2), "slept {:?}", started.elapsed());
    }

    // ── Connection test ─────────────────────────────────────────────────

    use crate::connection_test::TestStatus;
    use crate::rfc2217::{subnegotiation, COM_PORT_OPTION, SET_BAUDRATE, SET_CONTROL, SET_DATASIZE, SET_PARITY, SET_STOPSIZE, SIGNATURE};
    use crate::serial_params::{SerialFlowControl, SerialParity};
    use crate::util::{IAC, SB};
    use std::io::{Read, Write};

    #[cfg(windows)]
    const MISSING_PORT: &str = "COM250";
    #[cfg(unix)]
    const MISSING_PORT: &str = "/dev/auraterm-missing-port";

    const WILL: u8 = 251;
    const DO: u8 = 253;
    const TELNET_BINARY: u8 = 0;

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    /// Call the command the way the frontend does for a network transport.
    async fn test_network(transport: &str, host: &str, port: u16) -> Result<ConnectionTestReport, String> {
        serial_test_connection(
            "ignored".to_string(),
            115200,
            8,
            1,
            "none".to_string(),
            "none".to_string(),
            Some(transport.to_string()),
            Some(host.to_string()),
            Some(port),
        )
        .await
    }

    fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("addr").port()
    }

    /// Accept one client and record every byte it sends until it hangs up.
    fn spawn_recorder(reply: impl Fn(&[u8]) -> Vec<u8> + Send + 'static) -> (u16, std::thread::JoinHandle<Vec<u8>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let handle = std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return Vec::new();
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut received = Vec::new();
            let mut buffer = [0_u8; 1024];
            loop {
                match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return received,
                    Ok(size) => {
                        received.extend_from_slice(&buffer[..size]);
                        let answer = reply(&buffer[..size]);
                        if !answer.is_empty() && stream.write_all(&answer).is_err() {
                            return received;
                        }
                    }
                }
            }
        });
        (port, handle)
    }

    #[test]
    fn open_errors_map_to_what_the_user_can_do_about_them() {
        use serialport::ErrorKind as Kind;
        use std::io::ErrorKind as Io;
        let cases = [
            // Windows folds "in use" and "missing" into NoDevice; the port list decides.
            (Kind::NoDevice, true, TestCode::SerialPortBusy),
            (Kind::NoDevice, false, TestCode::SerialPortNotFound),
            (Kind::Io(Io::NotFound), false, TestCode::SerialPortNotFound),
            (Kind::Io(Io::NotFound), true, TestCode::SerialPortNotFound),
            (Kind::Io(Io::PermissionDenied), false, TestCode::SerialPermissionDenied),
            (Kind::Io(Io::PermissionDenied), true, TestCode::SerialPermissionDenied),
            (Kind::InvalidInput, false, TestCode::SerialParamsRejected),
            (Kind::Io(Io::TimedOut), false, TestCode::SerialOpenFailed),
            (Kind::Io(Io::Other), true, TestCode::SerialOpenFailed),
            (Kind::Unknown, false, TestCode::SerialOpenFailed),
        ];
        for (kind, listed, expected) in cases {
            assert_eq!(classify_serial_open_error(kind, listed), expected, "{kind:?} listed={listed}");
        }
    }

    #[test]
    fn port_names_compare_the_way_the_os_opens_them() {
        assert!(same_port_name("COM3", "COM3"));
        assert!(!same_port_name("COM3", "COM4"));
        // A hand-typed `com3` opens COM3 on Windows, so a busy COM3 must stay
        // "busy" rather than become "not found". Device paths are case-sensitive.
        assert_eq!(same_port_name("COM3", "com3"), cfg!(windows));
        assert_eq!(same_port_name("/dev/ttyUSB0", "/dev/TTYUSB0"), cfg!(windows));
    }

    #[test]
    fn an_enumerated_port_is_listed_however_its_name_is_cased_on_windows() {
        // Exercises `port_is_listed` against the real enumeration. Machines
        // without a serial port (most CI runners) have nothing to check.
        let Ok(ports) = available_ports() else { return };
        for port in ports {
            assert!(port_is_listed(&port.port_name), "{} is enumerated", port.port_name);
            let recased = if port.port_name.to_ascii_lowercase() != port.port_name {
                port.port_name.to_ascii_lowercase()
            } else {
                port.port_name.to_ascii_uppercase()
            };
            if recased != port.port_name {
                assert_eq!(port_is_listed(&recased), cfg!(windows), "{recased} vs {}", port.port_name);
            }
        }
    }

    #[test]
    fn a_hand_typed_missing_port_is_not_found_whatever_its_case() {
        let typed = MISSING_PORT.to_ascii_lowercase();
        assert!(!port_is_listed(&typed));
        let failure = probe_local_port(&typed, SerialParams::default()).expect_err("no such port");
        assert_eq!(failure.code, TestCode::SerialPortNotFound, "{}", failure.detail);
    }

    #[test]
    fn a_missing_local_port_is_reported_as_not_found() {
        assert!(!port_is_listed(MISSING_PORT), "{MISSING_PORT} unexpectedly exists on this machine");
        let failure = probe_local_port(MISSING_PORT, SerialParams::default()).expect_err("no such port");
        assert_eq!(failure.code, TestCode::SerialPortNotFound, "{}", failure.detail);
        assert!(!failure.detail.is_empty());
    }

    #[test]
    fn settings_the_driver_cannot_express_are_rejected_before_opening() {
        let params = SerialParams { data_bits: 9, ..SerialParams::default() };
        let failure = probe_local_port(MISSING_PORT, params).expect_err("9 data bits");
        // Not "not found": the port was never touched.
        assert_eq!(failure.code, TestCode::SerialParamsRejected, "{}", failure.detail);
    }

    #[tokio::test]
    async fn the_local_command_reports_a_missing_port_instead_of_failing() {
        let started = Instant::now();
        let report = serial_test_connection(
            MISSING_PORT.to_string(),
            9600,
            8,
            1,
            "none".to_string(),
            "none".to_string(),
            None,
            None,
            None,
        )
        .await
        .expect("an expected outcome is never Err");
        assert_eq!(report.code, TestCode::SerialPortNotFound);
        assert_eq!(report.status, TestStatus::Failure);
        assert!(report.detail.is_some());
        assert!(started.elapsed() <= LOCAL_OPEN_TIMEOUT, "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn malformed_requests_are_errors() {
        let bad_parity = serial_test_connection(
            MISSING_PORT.to_string(), 9600, 8, 1, "bogus".to_string(), "none".to_string(), None, None, None,
        )
        .await;
        assert!(bad_parity.is_err(), "{bad_parity:?}");

        let bad_bits = serial_test_connection(
            MISSING_PORT.to_string(), 9600, 9, 1, "none".to_string(), "none".to_string(), None, None, None,
        )
        .await;
        assert!(bad_bits.is_err(), "{bad_bits:?}");

        let bad_transport = test_network("carrier-pigeon", "127.0.0.1", 2217).await;
        assert!(bad_transport.is_err(), "{bad_transport:?}");

        let no_host = test_network("rfc2217", "   ", 2217).await;
        assert!(no_host.expect_err("blank host").contains("host"));

        let no_port = test_network("raw-tcp", "127.0.0.1", 0).await;
        assert!(no_port.expect_err("port 0").contains("port"));

        let missing_host = serial_test_connection(
            "x".to_string(), 9600, 8, 1, "none".to_string(), "none".to_string(),
            Some("raw-tcp".to_string()), None, Some(4001),
        )
        .await;
        assert!(missing_host.is_err(), "{missing_host:?}");
    }

    #[tokio::test]
    async fn raw_tcp_connects_and_hangs_up_without_sending_a_byte() {
        let (port, server) = spawn_recorder(|_| Vec::new());
        // Padded like a hand-typed host: trimmed exactly as a session would.
        let report = test_network("raw-tcp", " 127.0.0.1 ", port).await.expect("report");
        assert_eq!(report.code, TestCode::Ok);
        assert_eq!(report.status, TestStatus::Success);
        assert!(report.server_params.is_none());

        let received = server.join().expect("server thread");
        assert!(received.is_empty(), "a raw TCP test must not reach the device: {received:?}");
    }

    #[tokio::test]
    async fn raw_tcp_to_a_closed_port_is_refused() {
        let report = test_network("raw-tcp", "127.0.0.1", closed_port()).await.expect("report");
        assert_eq!(report.code, TestCode::Refused);
    }

    #[tokio::test]
    async fn rfc2217_to_a_closed_port_is_refused() {
        let report = test_network("rfc2217", "127.0.0.1", closed_port()).await.expect("report");
        assert_eq!(report.code, TestCode::Refused);
    }

    #[tokio::test]
    async fn rfc2217_reports_the_servers_settings_without_changing_them() {
        let (port, server) = spawn_recorder(|chunk| {
            let mut reply = Vec::new();
            if contains(chunk, &[IAC, WILL, TELNET_BINARY]) {
                reply.extend_from_slice(&[IAC, DO, TELNET_BINARY]);
            }
            if contains(chunk, &[IAC, WILL, COM_PORT_OPTION]) {
                reply.extend_from_slice(&[IAC, DO, COM_PORT_OPTION]);
            }
            if contains(chunk, &[IAC, SB, COM_PORT_OPTION, SET_BAUDRATE]) {
                reply.extend(subnegotiation(SIGNATURE + 100, b"mock-2217"));
                reply.extend(subnegotiation(SET_BAUDRATE + 100, &9600u32.to_be_bytes()));
                reply.extend(subnegotiation(SET_DATASIZE + 100, &[7]));
                reply.extend(subnegotiation(SET_PARITY + 100, &[3]));
                reply.extend(subnegotiation(SET_STOPSIZE + 100, &[2]));
                reply.extend(subnegotiation(SET_CONTROL + 100, &[3]));
            }
            reply
        });

        let started = Instant::now();
        let report = test_network("rfc2217", "127.0.0.1", port).await.expect("report");
        assert_eq!(report.code, TestCode::Ok, "{:?}", report.detail);
        assert_eq!(report.status, TestStatus::Success);
        assert_eq!(report.signature.as_deref(), Some("mock-2217"));
        let params = report.server_params.expect("server params");
        assert_eq!(params.baud_rate, Some(9600));
        assert_eq!(params.data_bits, Some(7));
        assert_eq!(params.parity, Some(SerialParity::Even));
        assert_eq!(params.stop_bits, Some(2));
        assert_eq!(params.flow_control, Some(SerialFlowControl::Hardware));
        // Everything confirmed, so it does not sit out the whole budget.
        assert!(started.elapsed() < NEGOTIATION_TIMEOUT, "took {:?}", started.elapsed());

        let received = server.join().expect("server thread");
        // Queries only: nothing the form asked for (115200 8N1) is pushed, and
        // the control lines are left alone.
        assert!(contains(&received, &subnegotiation(SET_BAUDRATE, &0u32.to_be_bytes())));
        assert!(!contains(&received, &subnegotiation(SET_BAUDRATE, &115200u32.to_be_bytes())));
        assert!(!contains(&received, &crate::rfc2217::ComPortState::dtr_frame(true)));
        assert!(!contains(&received, &crate::rfc2217::ComPortState::rts_frame(true)));
    }

    #[tokio::test]
    async fn rfc2217_against_a_raw_port_is_a_warning_within_the_budget() {
        // Accepts, never answers: a device server's raw TCP port.
        let (port, _server) = spawn_recorder(|_| Vec::new());
        let started = Instant::now();
        let report = test_network("rfc2217", "127.0.0.1", port).await.expect("report");
        assert_eq!(report.code, TestCode::Rfc2217Refused);
        assert_eq!(report.status, TestStatus::Warning);
        assert_eq!(report.detail.as_deref(), Some("Server did not accept COM-PORT-OPTION"));
        assert!(started.elapsed() < NEGOTIATION_TIMEOUT + Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    #[tokio::test]
    async fn rfc2217_to_a_server_that_hangs_up_is_peer_closed() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let server = std::thread::spawn(move || {
            // Accept and drop straight away, like a single-client server that
            // is already taken.
            let _ = listener.accept();
        });
        let report = test_network("rfc2217", "127.0.0.1", port).await.expect("report");
        server.join().expect("server thread");
        assert_eq!(report.code, TestCode::PeerClosed, "{:?}", report.detail);
        assert_eq!(report.status, TestStatus::Failure);
    }
}
