//! Telnet protocol implementation (RFC 854 / RFC 855)
//!
//! Supports:
//!   - IAC command parsing & negotiation
//!   - ECHO (opt 1), SGA (opt 3), TERMINAL-TYPE (opt 24), NAWS (opt 31)
//!   - Proactive client option announcement on connect
//!   - Window resize via NAWS subnegotiation

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};
use std::thread;

use tauri::{AppHandle, Emitter};

// ─── Telnet protocol constants ────────────────────────────────────────────────

const IAC: u8 = 255; // Interpret As Command
const SE: u8 = 240;  // End of subnegotiation
const SB: u8 = 250;  // Begin subnegotiation
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;

// Telnet options
const OPT_ECHO: u8 = 1;          // RFC 857
const OPT_SGA: u8 = 3;           // RFC 858 – Suppress Go Ahead (full-duplex)
const OPT_TERMINAL_TYPE: u8 = 24; // RFC 1091
const OPT_NAWS: u8 = 31;          // RFC 1073 – Negotiate About Window Size

// Subnegotiation sub-commands
const TERM_IS: u8 = 0;   // TERMINAL-TYPE IS ...
const TERM_SEND: u8 = 1; // TERMINAL-TYPE SEND

// ─── Events ───────────────────────────────────────────────────────────────────

#[derive(Clone, serde::Serialize)]
struct PtyOutputEvent {
    id: String,
    data: String,
}

#[derive(Clone, serde::Serialize)]
struct PtyExitEvent {
    id: String,
    message: String,
}

// ─── Session state ────────────────────────────────────────────────────────────

pub struct TelnetSession {
    /// Write half of the TCP stream (protected by Mutex for cross-thread access)
    pub writer: Arc<Mutex<TcpStream>>,
    /// Current terminal size – updated by resize_telnet_pty
    pub size: Arc<Mutex<(u16, u16)>>,
}

#[derive(Clone)]
pub struct TelnetState {
    pub sessions: Arc<Mutex<HashMap<String, TelnetSession>>>,
}

impl Default for TelnetState {
    fn default() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// ─── Protocol parser state machine ───────────────────────────────────────────

/// Internal parser state.  
/// The state machine advances one byte at a time and is driven by `parse_telnet`.
enum ParseState {
    /// Normal data bytes
    Normal,
    /// Received IAC – waiting for the next command byte
    Iac,
    /// Received IAC + WILL/WONT/DO/DONT – waiting for the option byte
    IacCmd(u8),
    /// Received IAC + SB – waiting for the option byte that identifies the subneg
    Sb,
    /// Inside a subnegotiation: collecting payload bytes
    SbData { opt: u8, data: Vec<u8> },
    /// Inside a subnegotiation, received IAC – waiting for SE or escaped IAC
    SbIac { opt: u8, data: Vec<u8> },
}

/// Parse a raw Telnet byte stream.
///
/// Returns `(clean_data, responses_to_send)`:
/// - `clean_data`        – stripped payload bytes to display in the terminal
/// - `responses_to_send` – bytes that must be written back to the server immediately
fn parse_telnet(
    input: &[u8],
    state: &mut ParseState,
    cols: u16,
    rows: u16,
) -> (Vec<u8>, Vec<u8>) {
    let mut output: Vec<u8> = Vec::new();
    let mut response: Vec<u8> = Vec::new();

    for &byte in input {
        match state {
            // ── Normal data ───────────────────────────────────────────────
            ParseState::Normal => {
                if byte == IAC {
                    *state = ParseState::Iac;
                } else {
                    output.push(byte);
                }
            }

            // ── Just saw IAC ──────────────────────────────────────────────
            ParseState::Iac => {
                match byte {
                    IAC => {
                        // Escaped IAC – literal 0xFF in the data stream
                        output.push(0xFF);
                        *state = ParseState::Normal;
                    }
                    WILL | WONT | DO | DONT => {
                        *state = ParseState::IacCmd(byte);
                    }
                    SB => {
                        *state = ParseState::Sb;
                    }
                    SE => {
                        // Stray SE outside subneg – ignore
                        *state = ParseState::Normal;
                    }
                    // NOP, DM, BRK, IP, AO, AYT, EC, EL, GA – single-byte commands, discard
                    _ => {
                        *state = ParseState::Normal;
                    }
                }
            }

            // ── IAC WILL/WONT/DO/DONT <option> ───────────────────────────
            ParseState::IacCmd(cmd) => {
                let cmd = *cmd;
                let opt = byte;
                *state = ParseState::Normal;

                match cmd {
                    // Server announces: WILL <opt> → we send DO or DONT
                    WILL => match opt {
                        OPT_ECHO | OPT_SGA => {
                            // Accept: echo suppression & full-duplex mode
                            response.extend_from_slice(&[IAC, DO, opt]);
                        }
                        _ => {
                            response.extend_from_slice(&[IAC, DONT, opt]);
                        }
                    },
                    // Server refuses: WONT <opt> → acknowledge with DONT
                    WONT => {
                        response.extend_from_slice(&[IAC, DONT, opt]);
                    }
                    // Server requests: DO <opt> → we send WILL or WONT
                    DO => match opt {
                        OPT_TERMINAL_TYPE => {
                            response.extend_from_slice(&[IAC, WILL, opt]);
                        }
                        OPT_NAWS => {
                            response.extend_from_slice(&[IAC, WILL, opt]);
                            // Immediately report current window size
                            push_naws(&mut response, cols, rows);
                        }
                        _ => {
                            response.extend_from_slice(&[IAC, WONT, opt]);
                        }
                    },
                    // Server withdraws: DONT <opt> → acknowledge with WONT
                    DONT => {
                        response.extend_from_slice(&[IAC, WONT, opt]);
                    }
                    _ => {}
                }
            }

            // ── IAC SB – waiting for option byte ─────────────────────────
            ParseState::Sb => {
                *state = ParseState::SbData {
                    opt: byte,
                    data: Vec::new(),
                };
            }

            // ── Collecting subnegotiation payload ─────────────────────────
            ParseState::SbData { opt, data } => {
                if byte == IAC {
                    let opt_val = *opt;
                    let d = std::mem::take(data);
                    *state = ParseState::SbIac { opt: opt_val, data: d };
                } else {
                    data.push(byte);
                }
            }

            // ── IAC received inside subneg payload ───────────────────────
            ParseState::SbIac { opt, data } => {
                match byte {
                    SE => {
                        // Subnegotiation complete
                        let opt_val = *opt;
                        let d = std::mem::take(data);
                        *state = ParseState::Normal;

                        handle_subneg(&mut response, opt_val, &d);
                    }
                    IAC => {
                        // Escaped IAC inside subneg payload
                        data.push(0xFF);
                        let opt_val = *opt;
                        let d = std::mem::take(data);
                        *state = ParseState::SbData { opt: opt_val, data: d };
                    }
                    _ => {
                        // Malformed – reset
                        *state = ParseState::Normal;
                    }
                }
            }
        }
    }

    (output, response)
}

/// Handle a completed subnegotiation and append any required response to `buf`.
fn handle_subneg(buf: &mut Vec<u8>, opt: u8, data: &[u8]) {
    match opt {
        OPT_TERMINAL_TYPE => {
            // Server asks: TERMINAL-TYPE SEND
            if data.first() == Some(&TERM_SEND) {
                // Respond: TERMINAL-TYPE IS xterm-256color
                buf.extend_from_slice(&[IAC, SB, OPT_TERMINAL_TYPE, TERM_IS]);
                buf.extend_from_slice(b"xterm-256color");
                buf.extend_from_slice(&[IAC, SE]);
            }
        }
        _ => {
            // Other subneg options – silently ignored
        }
    }
}

/// Build an IAC SB NAWS ... IAC SE packet with IAC-escaped size bytes.
fn push_naws(buf: &mut Vec<u8>, cols: u16, rows: u16) {
    fn push_escaped(buf: &mut Vec<u8>, b: u8) {
        if b == IAC {
            buf.push(IAC); // escape
        }
        buf.push(b);
    }

    buf.extend_from_slice(&[IAC, SB, OPT_NAWS]);
    push_escaped(buf, (cols >> 8) as u8);
    push_escaped(buf, (cols & 0xFF) as u8);
    push_escaped(buf, (rows >> 8) as u8);
    push_escaped(buf, (rows & 0xFF) as u8);
    buf.extend_from_slice(&[IAC, SE]);
}

/// Escape any IAC (0xFF) bytes in `data` so they survive the Telnet data channel.
fn escape_iac(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    for &b in data {
        if b == IAC {
            out.push(IAC); // double it
        }
        out.push(b);
    }
    out
}

// ─── Tauri commands ───────────────────────────────────────────────────────────

/// Start a Telnet session and begin emitting `pty-output` / `pty-exit` events.
#[tauri::command]
pub fn start_telnet_pty(
    app: AppHandle,
    state: tauri::State<'_, super::AppState>,
    host: String,
    port: u16,
    cols: u16,
    rows: u16,
    id: String,
) -> Result<String, String> {
    let addr = format!("{}:{}", host, port);
    let stream = TcpStream::connect(&addr).map_err(|e| e.to_string())?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;

    // Clone stream for the write half
    let write_stream = stream.try_clone().map_err(|e| e.to_string())?;
    let writer = Arc::new(Mutex::new(write_stream));
    let size = Arc::new(Mutex::new((cols, rows)));

    // ── Store session ─────────────────────────────────────────────────────
    {
        let mut guard = state.telnet_state.sessions.lock().map_err(|e| e.to_string())?;
        guard.insert(
            id.clone(),
            TelnetSession {
                writer: writer.clone(),
                size: size.clone(),
            },
        );
    }

    // ── Proactive option announcement ─────────────────────────────────────
    // Announce client capabilities so the server knows what we support.
    let announce: &[u8] = &[
        IAC, WILL, OPT_TERMINAL_TYPE, // We can do terminal-type negotiation
        IAC, WILL, OPT_NAWS,          // We can report window size
        IAC, DO,   OPT_SGA,           // Request full-duplex (suppress go-ahead)
        IAC, DO,   OPT_ECHO,          // Request server-side echo
    ];
    {
        let mut w = writer.lock().map_err(|e| e.to_string())?;
        w.write_all(announce).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())?;
    }

    // ── Background read thread ────────────────────────────────────────────
    let pty_id = id.clone();
    let writer_clone = writer.clone();
    let size_clone = size.clone();

    thread::spawn(move || {
        let mut reader = stream;
        let mut buffer = [0_u8; 4096];
        let mut parse_state = ParseState::Normal;

        loop {
            let n = match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: pty_id.clone(),
                            message: "Telnet connection closed".to_string(),
                        },
                    );
                    break;
                }
                Ok(n) => n,
                Err(e) => {
                    let _ = app.emit(
                        "pty-exit",
                        PtyExitEvent {
                            id: pty_id.clone(),
                            message: format!("Telnet read error: {}", e),
                        },
                    );
                    break;
                }
            };

            let (cols, rows) = *size_clone.lock().unwrap();
            let (clean, responses) = parse_telnet(&buffer[..n], &mut parse_state, cols, rows);

            // Send negotiation responses back to the server
            if !responses.is_empty() {
                if let Ok(mut w) = writer_clone.lock() {
                    let _ = w.write_all(&responses);
                    let _ = w.flush();
                }
            }

            // Forward clean terminal data to the frontend
            if !clean.is_empty() {
                let output = String::from_utf8_lossy(&clean).to_string();
                let _ = app.emit(
                    "pty-output",
                    PtyOutputEvent {
                        id: pty_id.clone(),
                        data: output,
                    },
                );
            }
        }
    });

    Ok(id)
}

/// Write user input to the Telnet session.  
/// IAC bytes inside `data` are automatically escaped.
#[tauri::command]
pub fn write_telnet_pty_input(
    state: tauri::State<'_, super::AppState>,
    id: String,
    data: String,
) -> Result<(), String> {
    let writer = {
        let guard = state.telnet_state.sessions.lock().map_err(|e| e.to_string())?;
        guard
            .get(&id)
            .map(|s| s.writer.clone())
            .ok_or_else(|| "Telnet session not found".to_string())?
    };

    let escaped = escape_iac(data.as_bytes());
    let mut w = writer.lock().map_err(|e| e.to_string())?;
    w.write_all(&escaped).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Update the terminal window size and send an IAC SB NAWS subnegotiation.
#[tauri::command]
pub fn resize_telnet_pty(
    state: tauri::State<'_, super::AppState>,
    id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let (writer, size_arc) = {
        let guard = state.telnet_state.sessions.lock().map_err(|e| e.to_string())?;
        let s = guard
            .get(&id)
            .ok_or_else(|| "Telnet session not found".to_string())?;
        (s.writer.clone(), s.size.clone())
    };

    // Update stored size
    *size_arc.lock().map_err(|e| e.to_string())? = (cols, rows);

    // Send NAWS
    let mut naws_buf: Vec<u8> = Vec::new();
    push_naws(&mut naws_buf, cols, rows);

    let mut w = writer.lock().map_err(|e| e.to_string())?;
    w.write_all(&naws_buf).map_err(|e| e.to_string())?;
    w.flush().map_err(|e| e.to_string())?;
    Ok(())
}

/// Close and remove a Telnet session.
#[tauri::command]
pub fn close_telnet_pty(
    state: tauri::State<'_, super::AppState>,
    id: String,
) -> Result<(), String> {
    let mut guard = state.telnet_state.sessions.lock().map_err(|e| e.to_string())?;
    if let Some(session) = guard.remove(&id) {
        // Shutting down the TCP stream will cause the read thread to exit cleanly
        if let Ok(w) = session.writer.lock() {
            let _ = w.shutdown(std::net::Shutdown::Both);
        }
    }
    Ok(())
}
