//! The seam every serial session sits on.
//!
//! A serial session is a byte stream plus a set of line parameters. Whether
//! those bytes come from a UART on this machine or from a device server across
//! the network changes only *how* the bytes are moved, not what the session
//! does with them — so the read loop in [`crate::serial`] talks to these two
//! traits and stays identical for both.
//!
//! Reads and writes are split because they live on different threads: the
//! reader loop owns a [`SerialLink`] exclusively, while the [`SerialSink`] is
//! shared between that loop (protocol replies, ZMODEM responses) and every
//! Tauri command that writes user input.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use serialport::SerialPort;

use crate::rfc2217::ComPortState;
use crate::serial_params::{
    LineErrors, ModemLines, PurgeTarget, SerialParams, SerialParamsConfirmed, SerialSignals,
    SerialStatus, SerialTransport,
};
use crate::util::{telnet_escape_outbound, TelnetIacFilter};

/// How long to wait for the TCP handshake before giving up.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle time before the kernel starts probing a quiet connection.
///
/// Without keepalive, a device server that reboots — or a NAT that quietly
/// drops the flow — leaves the socket looking alive indefinitely and the reader
/// blocked on a connection that will never speak again.
const KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
/// Gap between probes once they start.
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
/// Read timeout. Bounds how long a stopped session takes to notice `stop_flag`.
const READ_TIMEOUT: Duration = Duration::from_millis(100);

/// The live write half of a session, swappable underneath its holders.
///
/// A network session that drops and reconnects gets an entirely new socket and
/// sink. Everything that writes — Tauri commands, the shared-session bridge,
/// the reader's own protocol replies — goes through this slot, so a reconnect
/// never leaves anyone holding a handle to the dead socket.
#[derive(Clone)]
pub struct SinkSlot(Arc<StdMutex<Arc<dyn SerialSink>>>);

impl SinkSlot {
    pub fn new(sink: Arc<dyn SerialSink>) -> Self {
        Self(Arc::new(StdMutex::new(sink)))
    }

    pub fn get(&self) -> Arc<dyn SerialSink> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn replace(&self, sink: Arc<dyn SerialSink>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = sink;
    }
}

/// The read half of a serial session.
pub trait SerialLink: Send {
    /// Read the next chunk of *terminal* bytes, with any protocol framing
    /// already stripped and answered.
    ///
    /// `Ok(0)` means "nothing for the terminal this tick" — a read timeout, or a
    /// chunk that was entirely protocol negotiation. It never means end of
    /// stream; a closed peer is reported as an error so the read loop exits.
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize>;
}

/// The write half of a serial session, plus its status.
pub trait SerialSink: Send + Sync {
    /// Write user data — keystrokes, pastes, ZMODEM payload. Network transports
    /// escape it for the wire.
    fn write_data(&self, data: &[u8]) -> Result<(), String>;

    /// Write bytes that are already framed protocol output (negotiation
    /// replies, subnegotiations).
    ///
    /// Kept separate from [`SerialSink::write_data`] on purpose: a negotiation
    /// reply contains `IAC` bytes, and sending it through the escaper would
    /// double them into `IAC IAC` and stall the handshake for good.
    fn write_raw(&self, data: &[u8]) -> Result<(), String>;

    /// Current view of the session for the frontend.
    fn status(&self, id: &str) -> SerialStatus;

    /// Status snapshot, but only when something changed since the last call.
    fn status_if_changed(&self, id: &str) -> Option<SerialStatus>;

    /// Retune the line without dropping the session.
    ///
    /// Guessing a baud rate is the single most common serial chore, and having
    /// to close and reopen the port for each guess loses whatever the device
    /// printed in between.
    fn set_params(&self, params: SerialParams) -> Result<(), String>;

    /// Assert BREAK for `millis`, then release it. Blocks for that long, so
    /// callers must not run this on an async runtime thread.
    ///
    /// BREAK is how you interrupt a device that is not listening to characters:
    /// U-Boot, Cisco ROMMON, a Solaris OK prompt.
    fn send_break(&self, millis: u32) -> Result<(), String>;

    /// Drive DTR and/or RTS. `None` leaves that line alone.
    fn set_signals(&self, dtr: Option<bool>, rts: Option<bool>) -> Result<(), String>;

    /// Discard buffered bytes in one or both directions.
    fn purge(&self, target: PurgeTarget) -> Result<(), String>;

    /// Declare the COM-PORT-OPTION handshake over when nobody ever answered.
    /// A no-op for transports with no handshake to begin with.
    fn give_up_negotiation(&self) {}

    /// Refresh modem lines on transports that have to ask for them.
    ///
    /// A local UART only reports CTS/DSR/CD/RI when polled; RFC 2217 pushes
    /// them, so its implementation is a no-op.
    fn poll_modem_lines(&self) {}
}

/// How long a control operation may block before we assume the peer is wedged.
const MAX_BREAK_MILLIS: u32 = 5_000;

// ── Local serial port ────────────────────────────────────────────────────────

struct LocalSerialLink {
    reader: Box<dyn SerialPort>,
}

impl SerialLink for LocalSerialLink {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

struct LocalState {
    params: SerialParams,
    modem: ModemLines,
    signals: SerialSignals,
    /// Cleared after the first failure. A port with no modem lines — a USB CDC
    /// gadget, a pty — errors on every read, and retrying forever is noise.
    modem_pollable: bool,
    dirty: bool,
}

struct LocalSerialSink {
    port: StdMutex<Box<dyn SerialPort>>,
    state: StdMutex<LocalState>,
}

impl LocalSerialSink {
    fn write_all(&self, data: &[u8]) -> Result<(), String> {
        let mut port = self.port.lock().map_err(|error| error.to_string())?;
        port.write_all(data).map_err(|error| error.to_string())?;
        port.flush().map_err(|error| error.to_string())
    }
}

impl SerialSink for LocalSerialSink {
    fn write_data(&self, data: &[u8]) -> Result<(), String> {
        self.write_all(data)
    }

    fn write_raw(&self, data: &[u8]) -> Result<(), String> {
        // A local UART has no framing layer, so the two paths coincide.
        self.write_all(data)
    }

    fn status(&self, id: &str) -> SerialStatus {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        local_status(id, &state)
    }

    fn status_if_changed(&self, id: &str) -> Option<SerialStatus> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !std::mem::take(&mut state.dirty) {
            return None;
        }
        Some(local_status(id, &state))
    }

    fn set_params(&self, params: SerialParams) -> Result<(), String> {
        {
            let mut port = self.port.lock().map_err(|error| error.to_string())?;
            port.set_baud_rate(params.baud_rate)
                .map_err(|error| format!("Cannot set baud rate: {error}"))?;
            port.set_data_bits(params.data_bits_enum()?)
                .map_err(|error| format!("Cannot set data bits: {error}"))?;
            port.set_stop_bits(params.stop_bits_enum()?)
                .map_err(|error| format!("Cannot set stop bits: {error}"))?;
            port.set_parity(params.parity_enum())
                .map_err(|error| format!("Cannot set parity: {error}"))?;
            port.set_flow_control(params.flow_control_enum())
                .map_err(|error| format!("Cannot set flow control: {error}"))?;
        }
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        state.params = params;
        state.dirty = true;
        Ok(())
    }

    fn send_break(&self, millis: u32) -> Result<(), String> {
        {
            let port = self.port.lock().map_err(|error| error.to_string())?;
            port.set_break()
                .map_err(|error| format!("Cannot assert BREAK: {error}"))?;
        }
        // The lock is released across the hold so a concurrent write is not
        // stalled for the whole duration.
        std::thread::sleep(Duration::from_millis(millis.min(MAX_BREAK_MILLIS) as u64));
        let port = self.port.lock().map_err(|error| error.to_string())?;
        port.clear_break()
            .map_err(|error| format!("Cannot release BREAK: {error}"))
    }

    fn set_signals(&self, dtr: Option<bool>, rts: Option<bool>) -> Result<(), String> {
        {
            let mut port = self.port.lock().map_err(|error| error.to_string())?;
            if let Some(dtr) = dtr {
                port.write_data_terminal_ready(dtr)
                    .map_err(|error| format!("Cannot set DTR: {error}"))?;
            }
            if let Some(rts) = rts {
                port.write_request_to_send(rts)
                    .map_err(|error| format!("Cannot set RTS: {error}"))?;
            }
        }
        let mut state = self.state.lock().map_err(|error| error.to_string())?;
        if let Some(dtr) = dtr {
            state.signals.dtr = dtr;
        }
        if let Some(rts) = rts {
            state.signals.rts = rts;
        }
        state.dirty = true;
        Ok(())
    }

    fn purge(&self, target: PurgeTarget) -> Result<(), String> {
        let port = self.port.lock().map_err(|error| error.to_string())?;
        port.clear(match target {
            PurgeTarget::Input => serialport::ClearBuffer::Input,
            PurgeTarget::Output => serialport::ClearBuffer::Output,
            PurgeTarget::Both => serialport::ClearBuffer::All,
        })
        .map_err(|error| format!("Cannot purge buffers: {error}"))
    }

    fn poll_modem_lines(&self) {
        if !self.state.lock().is_ok_and(|state| state.modem_pollable) {
            return;
        }

        let read = {
            let Ok(mut port) = self.port.lock() else {
                return;
            };
            // One failure means this port has no modem lines at all, so all four
            // are read together and the result is taken as a whole.
            (|| -> Result<ModemLines, serialport::Error> {
                Ok(ModemLines {
                    cts: port.read_clear_to_send()?,
                    dsr: port.read_data_set_ready()?,
                    cd: port.read_carrier_detect()?,
                    ri: port.read_ring_indicator()?,
                })
            })()
        };

        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match read {
            Ok(modem) => {
                if modem != state.modem {
                    state.modem = modem;
                    state.dirty = true;
                }
            }
            Err(_) => state.modem_pollable = false,
        }
    }
}

fn local_status(id: &str, state: &LocalState) -> SerialStatus {
    let mut status = SerialStatus::local(id, state.params);
    status.modem = state.modem;
    status.signals = state.signals;
    status
}

/// Open a port on this machine.
pub fn open_local(
    port_name: &str,
    params: SerialParams,
) -> Result<(Box<dyn SerialLink>, Arc<dyn SerialSink>), String> {
    let port = serialport::new(port_name, params.baud_rate)
        .timeout(READ_TIMEOUT)
        .data_bits(params.data_bits_enum()?)
        .stop_bits(params.stop_bits_enum()?)
        .parity(params.parity_enum())
        .flow_control(params.flow_control_enum())
        .open()
        .map_err(|error| error.to_string())?;

    let reader = port.try_clone().map_err(|error| error.to_string())?;
    let sink = Arc::new(LocalSerialSink {
        port: StdMutex::new(port),
        state: StdMutex::new(LocalState {
            params,
            modem: ModemLines::default(),
            signals: SerialSignals::default(),
            modem_pollable: true,
            dirty: false,
        }),
    });

    Ok((Box::new(LocalSerialLink { reader }), sink))
}

// ── Network serial port (RFC 2217 / raw TCP) ─────────────────────────────────

/// Shared write half of a network serial session.
pub struct NetSerialSink {
    writer: StdMutex<TcpStream>,
    /// `None` in raw-TCP mode, where the socket is a bare byte pipe with no
    /// Telnet layer to parse or escape.
    codec: Option<StdMutex<TelnetIacFilter>>,
    transport: SerialTransport,
    requested: SerialParams,
}

impl NetSerialSink {
    fn write_all(&self, data: &[u8]) -> Result<(), String> {
        let mut writer = self.writer.lock().map_err(|error| error.to_string())?;
        writer.write_all(data).map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    }

    /// Feed one raw socket chunk through the Telnet layer, answering any
    /// negotiation it contains, and return the bytes bound for the terminal.
    fn feed(&self, chunk: &[u8]) -> Result<Vec<u8>, String> {
        let Some(codec) = &self.codec else {
            return Ok(chunk.to_vec());
        };

        // Take the codec lock only long enough to parse; writing the reply
        // needs the writer lock, and holding both at once invites a deadlock.
        let filtered = {
            let mut codec = codec.lock().map_err(|error| error.to_string())?;
            codec.push(chunk)
        };

        if !filtered.response.is_empty() {
            self.write_raw(&filtered.response)?;
        }
        Ok(filtered.data)
    }

    fn require_com_port(&self) -> Result<&StdMutex<TelnetIacFilter>, String> {
        self.codec.as_ref().ok_or_else(|| {
            "Raw TCP has no control channel: line parameters, BREAK and the modem lines are              whatever the device server is configured with."
                .to_string()
        })
    }

    /// Send an already-framed control subnegotiation, refusing early when the
    /// peer never agreed to listen.
    fn control(&self, frames: Vec<u8>) -> Result<(), String> {
        {
            let codec = self.require_com_port()?;
            let codec = codec.lock().map_err(|error| error.to_string())?;
            let negotiated = codec
                .com_port()
                .is_some_and(|com_port| com_port.negotiated());
            if !negotiated {
                return Err(
                    "The server never accepted COM-PORT-OPTION, so it cannot act on control                      requests."
                        .to_string(),
                );
            }
        }
        self.write_raw(&frames)
    }

    /// Whether the COM-PORT-OPTION handshake has reached a verdict — either the
    /// peer accepted it, or it refused and the session degrades to a byte pipe.
    pub fn negotiation_settled(&self) -> bool {
        let Some(codec) = &self.codec else {
            return true;
        };
        let Ok(codec) = codec.lock() else {
            return true;
        };
        codec.com_port().map(|com_port| com_port.settled()).unwrap_or(true)
    }

    fn snapshot(&self, id: &str, only_if_changed: bool) -> Option<SerialStatus> {
        let Some(codec) = &self.codec else {
            if only_if_changed {
                return None;
            }
            // Raw TCP carries 8-bit data untouched, but nothing about the far
            // end's line settings is knowable from here.
            return Some(SerialStatus {
                id: id.to_string(),
                transport: self.transport,
                rfc2217_negotiated: false,
                // Nothing to negotiate, so there is nothing to wait for.
                negotiation_settled: true,
                binary_negotiated: true,
                requested: self.requested,
                effective: self.requested,
                confirmed: SerialParamsConfirmed::default(),
                modem: ModemLines::default(),
                signals: SerialSignals::default(),
                line_errors: LineErrors::default(),
                // A bare byte pipe has no control channel at all.
                controllable: false,
                signature: None,
            });
        };

        let mut codec = codec.lock().ok()?;
        let binary_negotiated = codec.binary_both_ways();
        let com_port = codec.com_port_mut()?;
        if only_if_changed && !com_port.take_dirty() {
            return None;
        }

        Some(SerialStatus {
            id: id.to_string(),
            transport: self.transport,
            rfc2217_negotiated: com_port.negotiated(),
            negotiation_settled: com_port.settled(),
            binary_negotiated,
            requested: com_port.requested(),
            effective: com_port.effective(),
            confirmed: com_port.confirmed(),
            modem: com_port.modem(),
            signals: com_port.signals(),
            line_errors: com_port.line_errors(),
            controllable: com_port.negotiated(),
            signature: com_port.signature().map(str::to_string),
        })
    }
}

impl SerialSink for NetSerialSink {
    fn write_data(&self, data: &[u8]) -> Result<(), String> {
        let payload = match &self.codec {
            Some(codec) => {
                let binary = {
                    let codec = codec.lock().map_err(|error| error.to_string())?;
                    codec.binary_out()
                };
                telnet_escape_outbound(data, binary)
            }
            None => data.to_vec(),
        };
        self.write_all(&payload)
    }

    fn write_raw(&self, data: &[u8]) -> Result<(), String> {
        self.write_all(data)
    }

    fn status(&self, id: &str) -> SerialStatus {
        self.snapshot(id, false)
            .unwrap_or_else(|| SerialStatus::local(id, self.requested))
    }

    fn status_if_changed(&self, id: &str) -> Option<SerialStatus> {
        self.snapshot(id, true)
    }

    fn set_params(&self, params: SerialParams) -> Result<(), String> {
        let frames = {
            let codec = self.require_com_port()?;
            let mut codec = codec.lock().map_err(|error| error.to_string())?;
            let com_port = codec
                .com_port_mut()
                .ok_or_else(|| "This session has no COM-PORT-OPTION channel".to_string())?;
            if !com_port.negotiated() {
                return Err(
                    "The server never accepted COM-PORT-OPTION, so it has no way to apply                      line parameters. Configure the port on the device server instead."
                        .to_string(),
                );
            }
            com_port.retune(params)
        };
        if frames.is_empty() {
            return Ok(());
        }
        self.write_raw(&frames)
    }

    fn send_break(&self, millis: u32) -> Result<(), String> {
        self.control(ComPortState::break_frame(true))?;
        std::thread::sleep(Duration::from_millis(millis.min(MAX_BREAK_MILLIS) as u64));
        self.control(ComPortState::break_frame(false))
    }

    fn set_signals(&self, dtr: Option<bool>, rts: Option<bool>) -> Result<(), String> {
        let mut frames = Vec::new();
        if let Some(dtr) = dtr {
            frames.extend_from_slice(&ComPortState::dtr_frame(dtr));
        }
        if let Some(rts) = rts {
            frames.extend_from_slice(&ComPortState::rts_frame(rts));
        }
        if frames.is_empty() {
            return Ok(());
        }
        self.control(frames)
    }

    fn purge(&self, target: PurgeTarget) -> Result<(), String> {
        self.control(ComPortState::purge_frame(target))
    }

    fn give_up_negotiation(&self) {
        let Some(codec) = &self.codec else {
            return;
        };
        if let Ok(mut codec) = codec.lock() {
            if let Some(com_port) = codec.com_port_mut() {
                com_port.give_up();
            }
        }
    }
}

struct NetSerialLink {
    reader: TcpStream,
    sink: Arc<NetSerialSink>,
    scratch: Vec<u8>,
    /// Terminal bytes decoded but not yet handed to the caller's buffer.
    pending: std::collections::VecDeque<u8>,
}

impl NetSerialLink {
    fn drain_pending(&mut self, buf: &mut [u8]) -> usize {
        let count = self.pending.len().min(buf.len());
        for slot in buf.iter_mut().take(count) {
            *slot = self.pending.pop_front().unwrap_or(0);
        }
        count
    }
}

impl SerialLink for NetSerialLink {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.pending.is_empty() {
            return Ok(self.drain_pending(buf));
        }

        let read = match self.reader.read(&mut self.scratch) {
            // On a socket, zero bytes means the peer hung up. The read loop's
            // own `Ok(0)` means "nothing this tick", so this has to surface as
            // an error or the loop would spin forever on a dead connection.
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed by peer",
                ))
            }
            Ok(size) => size,
            // A read timeout is `WouldBlock` on Unix and `TimedOut` on Windows.
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(0)
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => return Ok(0),
            Err(error) => return Err(error),
        };

        let data = self
            .sink
            .feed(&self.scratch[..read])
            .map_err(|error| std::io::Error::other(error))?;

        // A chunk that was pure negotiation leaves nothing for the terminal.
        if data.is_empty() {
            return Ok(0);
        }

        self.pending.extend(data);
        Ok(self.drain_pending(buf))
    }
}

/// Connect to a device server.
///
/// In RFC 2217 mode the returned sink has already put the opening offer on the
/// wire (BINARY both ways, SGA, WILL COM-PORT-OPTION); the caller drives the
/// reader loop and then waits for [`NetSerialSink::negotiation_settled`].
pub fn connect_network(
    host: &str,
    port: u16,
    transport: SerialTransport,
    params: SerialParams,
    adopt_server_params: bool,
) -> Result<(Box<dyn SerialLink>, Arc<NetSerialSink>), String> {
    let stream = connect_with_timeout(host, port)?;
    // Console traffic is tiny and latency-sensitive; Nagle would batch
    // keystrokes into visible lag.
    let _ = stream.set_nodelay(true);
    // Best effort: a platform that refuses keepalive still gets a working
    // session, it just takes longer to notice a dead peer.
    let _ = socket2::SockRef::from(&stream).set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(KEEPALIVE_IDLE)
            .with_interval(KEEPALIVE_INTERVAL),
    );

    let reader = stream.try_clone().map_err(|error| error.to_string())?;
    reader
        .set_read_timeout(Some(READ_TIMEOUT))
        .map_err(|error| error.to_string())?;

    let codec = match transport {
        SerialTransport::Rfc2217 => Some(StdMutex::new(TelnetIacFilter::with_com_port(
            ComPortState::new(params, adopt_server_params),
        ))),
        // Raw TCP deliberately has no Telnet layer: a 0xFF is data, not IAC.
        SerialTransport::RawTcp => None,
        SerialTransport::Local => {
            return Err("connect_network called for a local port".to_string())
        }
    };

    let sink = Arc::new(NetSerialSink {
        writer: StdMutex::new(stream),
        codec,
        transport,
        requested: params,
    });

    if let Some(codec) = &sink.codec {
        let opening = {
            let mut codec = codec.lock().map_err(|error| error.to_string())?;
            codec.initial_negotiation()
        };
        if !opening.is_empty() {
            sink.write_raw(&opening)?;
        }
    }

    let link = NetSerialLink {
        reader,
        sink: sink.clone(),
        scratch: vec![0_u8; 4096],
        pending: std::collections::VecDeque::new(),
    };

    Ok((Box::new(link), sink))
}

fn connect_with_timeout(host: &str, port: u16) -> Result<TcpStream, String> {
    let addresses = (host, port).to_socket_addrs().map_err(|_| {
        format!("Cannot resolve \"{host}\". Check the hostname, or use the IP address directly.")
    })?;

    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(stream) => return Ok(stream),
            Err(error) => last_error = Some(error),
        }
    }

    Err(match last_error {
        Some(error) => describe_connect_error(host, port, &error),
        None => format!("\"{host}\" resolved to no addresses."),
    })
}

/// Turn a connect failure into something that says what to do next.
///
/// "Connection refused" and "no route to host" call for completely different
/// actions, and the raw OS string says neither.
fn describe_connect_error(host: &str, port: u16, error: &std::io::Error) -> String {
    use std::io::ErrorKind;
    match error.kind() {
        ErrorKind::ConnectionRefused => format!(
            "Nothing is listening on {host}:{port}. Check the device server is running, and that              the port is right — RFC 2217 conventionally uses 2217."
        ),
        ErrorKind::TimedOut => format!(
            "Timed out connecting to {host}:{port} after {}s. A firewall is most likely dropping              the connection rather than refusing it.",
            CONNECT_TIMEOUT.as_secs(),
        ),
        ErrorKind::HostUnreachable | ErrorKind::NetworkUnreachable => {
            format!("No route to {host}. Check the network path to the device server.")
        }
        ErrorKind::PermissionDenied => format!(
            "Permission denied connecting to {host}:{port}. A local firewall or sandbox policy              is blocking the outbound connection."
        ),
        _ => format!("Cannot connect to {host}:{port}: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rfc2217::{subnegotiation, ComPortState, SET_BAUDRATE};
    use crate::serial_params::{SerialFlowControl, SerialParity};
    use crate::util::{IAC, SB};
    use std::net::{SocketAddr, TcpListener};
    use std::time::Instant;

    const WILL: u8 = 251;
    const DO: u8 = 253;
    const DONT: u8 = 254;
    const TELNET_BINARY: u8 = 0;
    const COM_PORT: u8 = crate::rfc2217::COM_PORT_OPTION;

    #[derive(Clone)]
    struct MockBehaviour {
        accept_com_port: bool,
        accept_binary: bool,
        /// Baud rate to report back in the `101 SET-BAUDRATE` reply.
        report_baud: Option<u32>,
        /// Bytes pushed to the client the moment it connects.
        greeting: Vec<u8>,
        /// Hang up right after the greeting.
        hang_up: bool,
        /// Answer nothing at all, like a device server exposing a raw TCP port.
        silent: bool,
    }

    impl Default for MockBehaviour {
        fn default() -> Self {
            Self {
                accept_com_port: true,
                accept_binary: true,
                report_baud: None,
                greeting: Vec::new(),
                hang_up: false,
                silent: false,
            }
        }
    }

    /// A stand-in for a device server, speaking just enough of the protocol to
    /// exercise the client end to end over loopback.
    struct MockServer {
        addr: SocketAddr,
        received: Arc<StdMutex<Vec<u8>>>,
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|window| window == needle)
    }

    fn spawn_mock(behaviour: MockBehaviour) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let received = Arc::new(StdMutex::new(Vec::new()));
        let sink = received.clone();

        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            if !behaviour.greeting.is_empty() {
                let _ = stream.write_all(&behaviour.greeting);
            }
            if behaviour.hang_up {
                return;
            }

            let mut buffer = [0_u8; 1024];
            loop {
                let read = match stream.read(&mut buffer) {
                    Ok(0) | Err(_) => return,
                    Ok(size) => size,
                };
                let chunk = &buffer[..read];
                sink.lock().expect("lock").extend_from_slice(chunk);

                if behaviour.silent {
                    // Echo it back as data, which is what a raw port does with
                    // the bytes of an offer it does not understand.
                    let _ = stream.write_all(chunk);
                    continue;
                }

                let mut reply = Vec::new();
                if behaviour.accept_binary {
                    if contains(chunk, &[IAC, WILL, TELNET_BINARY]) {
                        reply.extend_from_slice(&[IAC, DO, TELNET_BINARY]);
                    }
                    if contains(chunk, &[IAC, DO, TELNET_BINARY]) {
                        reply.extend_from_slice(&[IAC, WILL, TELNET_BINARY]);
                    }
                }
                if contains(chunk, &[IAC, WILL, COM_PORT]) {
                    reply.extend_from_slice(&[
                        IAC,
                        if behaviour.accept_com_port { DO } else { DONT },
                        COM_PORT,
                    ]);
                }
                if let Some(baud) = behaviour.report_baud {
                    if contains(chunk, &[IAC, SB, COM_PORT, SET_BAUDRATE]) {
                        reply.extend_from_slice(&subnegotiation(
                            SET_BAUDRATE + 100,
                            &baud.to_be_bytes(),
                        ));
                    }
                }
                if !reply.is_empty() && stream.write_all(&reply).is_err() {
                    return;
                }
            }
        });

        MockServer { addr, received }
    }

    fn params() -> SerialParams {
        SerialParams {
            baud_rate: 115200,
            data_bits: 8,
            stop_bits: 1,
            parity: SerialParity::None,
            flow_control: SerialFlowControl::None,
        }
    }

    /// Pump the read loop the way `serial.rs` does, until `done` or the deadline.
    fn drive(
        link: &mut Box<dyn SerialLink>,
        done: impl Fn(&[u8]) -> bool,
    ) -> Result<Vec<u8>, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 1024];
        while Instant::now() < deadline {
            match link.read(&mut buffer) {
                Ok(size) => collected.extend_from_slice(&buffer[..size]),
                Err(error) => return Err(error.to_string()),
            }
            if done(&collected) {
                return Ok(collected);
            }
        }
        Err(format!("timed out; collected {collected:?}"))
    }

    #[test]
    fn negotiation_reports_the_baud_rate_the_server_actually_applied() {
        let server = spawn_mock(MockBehaviour {
            report_baud: Some(9600),
            ..MockBehaviour::default()
        });
        let (mut link, sink) = connect_network(
            "127.0.0.1",
            server.addr.port(),
            SerialTransport::Rfc2217,
            params(),
            false,
        )
        .expect("connect");

        let _ = drive(&mut link, |_| sink.negotiation_settled());
        // The 101 reply arrives after the DO, so keep pumping briefly.
        let _ = drive(&mut link, |_| {
            sink.status("t").confirmed.baud_rate
        });

        let status = sink.status("t");
        assert!(status.rfc2217_negotiated);
        assert!(status.binary_negotiated);
        assert_eq!(status.requested.baud_rate, 115200);
        // A server is free to clamp; the UI has to be told the real value.
        assert_eq!(status.effective.baud_rate, 9600);
        assert!(status.confirmed.baud_rate);
    }

    #[test]
    fn a_refused_option_degrades_instead_of_failing() {
        let server = spawn_mock(MockBehaviour {
            accept_com_port: false,
            greeting: b"login: ".to_vec(),
            ..MockBehaviour::default()
        });
        let (mut link, sink) = connect_network(
            "127.0.0.1",
            server.addr.port(),
            SerialTransport::Rfc2217,
            params(),
            false,
        )
        .expect("connect");

        // The greeting races ahead of the refusal, so wait for both.
        let data = drive(&mut link, |seen| {
            sink.negotiation_settled() && seen.ends_with(b"login: ")
        })
        .expect("data");

        // Losing the option must not cost us the byte stream.
        assert_eq!(data, b"login: ");
        assert!(!sink.status("t").rfc2217_negotiated);
    }

    #[test]
    fn a_silent_server_never_settles_so_the_caller_can_time_out() {
        // The most common real-world mistake: pointing an RFC 2217 session at a
        // plain raw-TCP port. Nothing answers, so the handshake has to stay
        // unsettled and let `start_serial_session` fall through to its timeout
        // rather than block forever.
        let server = spawn_mock(MockBehaviour {
            silent: true,
            ..MockBehaviour::default()
        });
        let (mut link, sink) =
            connect_network("127.0.0.1", server.addr.port(), SerialTransport::Rfc2217, params(), false)
                .expect("connect");

        sink.write_data(b"hello").expect("write");
        let data = drive(&mut link, |seen| contains(seen, b"hello")).expect("data");

        // Data still flows both ways; only the parameters never took effect.
        assert!(contains(&data, b"hello"));
        assert!(!sink.negotiation_settled());
        assert!(!sink.status("t").rfc2217_negotiated);
    }

    #[test]
    fn outbound_ff_is_doubled_on_the_wire() {
        let server = spawn_mock(MockBehaviour::default());
        let (mut link, sink) = connect_network(
            "127.0.0.1",
            server.addr.port(),
            SerialTransport::Rfc2217,
            params(),
            false,
        )
        .expect("connect");
        let _ = drive(&mut link, |_| sink.negotiation_settled());

        sink.write_data(&[b'a', 0xFF, b'b']).expect("write");
        let _ = drive(&mut link, |_| {
            contains(&server.received.lock().expect("lock"), &[b'a', IAC, IAC, b'b'])
        });

        assert!(contains(
            &server.received.lock().expect("lock"),
            &[b'a', IAC, IAC, b'b'],
        ));
    }

    #[test]
    fn raw_tcp_leaves_the_byte_stream_completely_alone() {
        let server = spawn_mock(MockBehaviour::default());
        let (mut link, sink) = connect_network(
            "127.0.0.1",
            server.addr.port(),
            SerialTransport::RawTcp,
            params(),
            false,
        )
        .expect("connect");

        // No Telnet layer means no opening offer and no escaping: a 0xFF is data.
        sink.write_data(&[0xFF, b'\r']).expect("write");
        let _ = drive(&mut link, |_| {
            server.received.lock().expect("lock").len() >= 2
        });

        assert_eq!(*server.received.lock().expect("lock"), vec![0xFF, b'\r']);
        assert!(sink.negotiation_settled());
    }

    #[test]
    fn retuning_a_live_session_reaches_the_server() {
        let server = spawn_mock(MockBehaviour::default());
        let (mut link, sink) =
            connect_network("127.0.0.1", server.addr.port(), SerialTransport::Rfc2217, params(), false)
                .expect("connect");
        let _ = drive(&mut link, |_| sink.negotiation_settled());

        let mut next = params();
        next.baud_rate = 9600;
        sink.set_params(next).expect("retune");

        let frame = subnegotiation(SET_BAUDRATE, &9600u32.to_be_bytes());
        let _ = drive(&mut link, |_| {
            contains(&server.received.lock().expect("lock"), &frame)
        });
        assert!(contains(&server.received.lock().expect("lock"), &frame));
        // The session is still open: retuning must not cost the connection.
        assert!(sink.status("t").rfc2217_negotiated);
    }

    #[test]
    fn break_asserts_then_releases() {
        let server = spawn_mock(MockBehaviour::default());
        let (mut link, sink) =
            connect_network("127.0.0.1", server.addr.port(), SerialTransport::Rfc2217, params(), false)
                .expect("connect");
        let _ = drive(&mut link, |_| sink.negotiation_settled());

        let control = sink.clone();
        let breaker = std::thread::spawn(move || control.send_break(10));

        let on = ComPortState::break_frame(true);
        let off = ComPortState::break_frame(false);
        let _ = drive(&mut link, |_| {
            let seen = server.received.lock().expect("lock");
            contains(&seen, &on) && contains(&seen, &off)
        });

        breaker.join().expect("break thread").expect("break");
        let seen = server.received.lock().expect("lock");
        let on_at = seen.windows(on.len()).position(|w| w == on.as_slice());
        let off_at = seen.windows(off.len()).position(|w| w == off.as_slice());
        // Order matters: releasing before asserting would be a no-op on the line.
        assert!(on_at.is_some() && off_at.is_some());
        assert!(on_at < off_at, "BREAK OFF must follow BREAK ON");
    }

    #[test]
    fn control_is_refused_until_the_option_is_agreed() {
        // Sending SET-CONTROL to a peer that never accepted option 44 puts raw
        // bytes into whatever is on the other end of the serial line.
        let server = spawn_mock(MockBehaviour {
            silent: true,
            ..MockBehaviour::default()
        });
        let (_link, sink) =
            connect_network("127.0.0.1", server.addr.port(), SerialTransport::Rfc2217, params(), false)
                .expect("connect");

        let error = sink.set_params(params()).expect_err("must refuse");
        assert!(error.contains("COM-PORT-OPTION"), "got {error}");
        assert!(sink.send_break(1).is_err());
        assert!(sink.set_signals(Some(true), None).is_err());
        assert!(sink.purge(PurgeTarget::Both).is_err());
    }

    #[test]
    fn raw_tcp_says_why_it_has_no_control_surface() {
        let server = spawn_mock(MockBehaviour::default());
        let (_link, sink) =
            connect_network("127.0.0.1", server.addr.port(), SerialTransport::RawTcp, params(), false)
                .expect("connect");

        let error = sink.set_params(params()).expect_err("must refuse");
        assert!(error.contains("Raw TCP"), "got {error}");
        assert!(sink.send_break(1).is_err());
    }

    /// Interop check against a real access server, not our own mock.
    ///
    /// Two implementations of the same misunderstanding agree with each other,
    /// so the mock above cannot catch a protocol error. Point this at pyserial's
    /// `PortManager`, ser2net, or a Moxa/Digi box and run:
    ///
    /// ```text
    /// AURATERM_RFC2217_ENDPOINT=127.0.0.1:2217 \
    ///   cargo test -- --ignored rfc2217_interop
    /// ```
    #[test]
    #[ignore = "needs a live RFC 2217 server; set AURATERM_RFC2217_ENDPOINT"]
    fn rfc2217_interop_against_a_live_server() {
        let endpoint = std::env::var("AURATERM_RFC2217_ENDPOINT")
            .expect("set AURATERM_RFC2217_ENDPOINT=host:port");
        let (host, port) = endpoint.rsplit_once(':').expect("endpoint must be host:port");
        let port: u16 = port.parse().expect("port must be a number");

        let (mut link, sink) =
            connect_network(host, port, SerialTransport::Rfc2217, params(), false)
                .expect("connect");

        let _ = drive(&mut link, |_| sink.status("t").confirmed.baud_rate);
        let status = sink.status("t");
        assert!(status.rfc2217_negotiated, "server did not accept COM-PORT-OPTION");
        assert!(status.binary_negotiated, "server did not accept BINARY both ways");
        assert_eq!(
            status.effective.baud_rate, 115200,
            "server reported a baud rate other than the one requested",
        );
        assert_eq!(status.effective.data_bits, 8);

        // Round-trip a payload containing a literal 0xFF: if escaping is wrong
        // in either direction, this is where it shows.
        let payload = b"ping\xffpong";
        sink.write_data(payload).expect("write");
        let echoed = drive(&mut link, |seen| contains(seen, payload))
            .expect("echo did not come back intact");
        assert!(contains(&echoed, payload), "got {echoed:?}");

        // Retune the live session — the whole point of the control surface is
        // that this costs neither the connection nor the scrollback.
        let mut retuned = params();
        retuned.baud_rate = 9600;
        sink.set_params(retuned).expect("retune");
        let _ = drive(&mut link, |_| {
            sink.status("t").effective.baud_rate == 9600
        });
        assert_eq!(
            sink.status("t").effective.baud_rate,
            9600,
            "server did not confirm the new baud rate",
        );
        assert!(sink.status("t").confirmed.baud_rate);

        // BREAK and the control lines have to be accepted too.
        sink.send_break(10).expect("break");
        sink.set_signals(Some(false), None).expect("drop DTR");
        let _ = drive(&mut link, |_| !sink.status("t").signals.dtr);
        assert!(!sink.status("t").signals.dtr, "server did not acknowledge DTR");

        // Data still flows after all that.
        sink.write_data(b"after").expect("write");
        assert!(drive(&mut link, |seen| contains(seen, b"after")).is_ok());
    }

    #[test]
    fn the_sink_slot_hands_out_the_current_socket() {
        // Everything that writes goes through the slot, so a reconnect must be
        // visible to holders that captured it before the swap.
        let first = spawn_mock(MockBehaviour::default());
        let second = spawn_mock(MockBehaviour::default());
        let (_link_a, sink_a) =
            connect_network("127.0.0.1", first.addr.port(), SerialTransport::RawTcp, params(), false)
                .expect("connect");
        let (_link_b, sink_b) =
            connect_network("127.0.0.1", second.addr.port(), SerialTransport::RawTcp, params(), false)
                .expect("connect");

        let slot = SinkSlot::new(sink_a);
        // A holder captured before the swap, as the Tauri commands are.
        let holder = slot.clone();

        slot.replace(sink_b);
        holder.get().write_data(b"after").expect("write");

        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline
            && !contains(&second.received.lock().expect("lock"), b"after")
        {
            std::thread::sleep(Duration::from_millis(20));
        }

        assert!(contains(&second.received.lock().expect("lock"), b"after"));
        assert!(
            first.received.lock().expect("lock").is_empty(),
            "the replaced socket must not receive anything",
        );
    }

    #[test]
    fn a_peer_hangup_surfaces_as_an_error_not_as_end_of_data() {
        // `Ok(0)` from the link means "nothing this tick". If a closed socket
        // reported that instead of an error, the read loop would spin forever.
        let server = spawn_mock(MockBehaviour {
            greeting: b"bye".to_vec(),
            hang_up: true,
            ..MockBehaviour::default()
        });
        let (mut link, _sink) = connect_network(
            "127.0.0.1",
            server.addr.port(),
            SerialTransport::RawTcp,
            params(),
            false,
        )
        .expect("connect");

        let outcome = drive(&mut link, |_| false);
        assert!(outcome.is_err(), "expected the closed peer to surface as an error");
    }
}
