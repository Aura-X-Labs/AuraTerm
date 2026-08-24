//! Shared low-level utilities used across transport modules and persistence.

use std::fs;
use std::io::Write;
use std::path::Path;
use tauri::{AppHandle, Emitter};

use crate::{PtyExitEvent, PtyOutputEvent};

/// Incremental UTF-8 stream decoder.
///
/// Transport byte streams (local PTY, SSH, serial, telnet) are read in
/// fixed-size chunks, so a single multi-byte UTF-8 character — CJK text, emoji,
/// box-drawing glyphs — can be split across two reads. Decoding each chunk
/// independently with `String::from_utf8_lossy` turns the split bytes into
/// `U+FFFD` replacement characters and visibly corrupts the output.
///
/// `Utf8StreamDecoder` keeps any trailing bytes that form an *incomplete*
/// multi-byte sequence and prepends them to the next chunk, so characters that
/// straddle a read boundary decode correctly. Genuinely invalid bytes are still
/// replaced with `U+FFFD`, matching the previous lossy behaviour.
#[derive(Default)]
pub struct Utf8StreamDecoder {
    /// Trailing bytes of an incomplete multi-byte sequence (at most 3 bytes).
    remainder: Vec<u8>,
}

impl Utf8StreamDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decode a freshly read chunk, returning the portion that forms complete
    /// UTF-8. Incomplete trailing bytes are buffered for the next call.
    pub fn push(&mut self, chunk: &[u8]) -> String {
        if self.remainder.is_empty() && chunk.is_empty() {
            return String::new();
        }

        // Combine any leftover bytes from the previous chunk with the new one.
        let buf: Vec<u8> = if self.remainder.is_empty() {
            chunk.to_vec()
        } else {
            let mut combined = std::mem::take(&mut self.remainder);
            combined.extend_from_slice(chunk);
            combined
        };

        let mut out = String::with_capacity(buf.len());
        let mut start = 0;

        loop {
            match std::str::from_utf8(&buf[start..]) {
                Ok(valid) => {
                    out.push_str(valid);
                    break;
                }
                Err(error) => {
                    let valid_up_to = error.valid_up_to();
                    // SAFETY: `valid_up_to()` guarantees this slice is valid UTF-8.
                    out.push_str(unsafe {
                        std::str::from_utf8_unchecked(&buf[start..start + valid_up_to])
                    });

                    match error.error_len() {
                        // Incomplete sequence at the end: stash for the next chunk.
                        None => {
                            self.remainder.extend_from_slice(&buf[start + valid_up_to..]);
                            break;
                        }
                        // Genuinely invalid bytes: emit a replacement char and skip them.
                        Some(invalid_len) => {
                            out.push('\u{FFFD}');
                            start += valid_up_to + invalid_len;
                        }
                    }
                }
            }
        }

        out
    }
}

/// Atomically write `content` to `path`.
///
/// Writes to a temp file in the same directory, fsyncs it, then renames it over
/// the destination. This prevents a crash or power loss mid-write from leaving a
/// truncated/corrupt file — important for frequently-rewritten state such as
/// `settings.json` (workspace persistence) and `connections.json`.
pub fn write_atomic(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Invalid file path (no parent): {}", path.display()))?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| format!("Invalid file name: {}", path.display()))?;

    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let tmp_path = parent.join(format!(".{file_name}.tmp-{}-{nonce}", std::process::id()));

    let mut tmp_file = fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp_path)
        .map_err(|error| format!("Failed to open temp file: {error}"))?;

    tmp_file
        .write_all(content.as_bytes())
        .map_err(|error| format!("Failed to write temp file: {error}"))?;
    tmp_file
        .sync_all()
        .map_err(|error| format!("Failed to flush temp file: {error}"))?;

    drop(tmp_file);

    fs::rename(&tmp_path, path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to atomically replace file: {error}")
    })?;

    Ok(())
}

/// Build the per-session name for a streaming Tauri event (`<base>:<id>`).
///
/// Per-session events (`pty-output`, `pty-exit`, `ssh-connected`, …) used to be
/// emitted under a single global name, so every mounted `TerminalComponent`
/// received every event and discarded the ones whose `id` did not match. With N
/// open panes/tabs that is an O(N) wake-up for *every* output chunk on the hot
/// path. Suffixing the session id makes each event reach only the owning
/// component's listener, which subscribes to `<base>:<its-own-id>`.
///
/// The session id is the frontend tab id (`tab-0`, …), which only contains
/// alphanumerics and `-`, so the resulting name is always a valid Tauri event
/// name (`:` is permitted). Keep this `:` separator in sync with the frontend
/// listener names in `TerminalComponent.vue`.
pub fn session_event(base: &str, id: &str) -> String {
    format!("{base}:{id}")
}

/// Process one raw terminal stream chunk through the optional Zmodem router,
/// the shared streaming UTF-8 decoder, and the per-session output event.
/// Protocol response bytes are returned to the transport-specific writer.
pub fn pump_stream_chunk(
    app: &AppHandle,
    id: &str,
    decoder: &mut Utf8StreamDecoder,
    chunk: &[u8],
    zmodem: &crate::zmodem::ZmodemState,
) -> (String, Vec<u8>) {
    let processed = zmodem.process_incoming(app, id, chunk);
    let output = decoder.push(&processed.terminal);
    if !output.is_empty() {
        let _ = app.emit(
            &session_event("pty-output", id),
            PtyOutputEvent {
                id: id.to_string(),
                data: output.clone(),
            },
        );
    }
    (output, processed.response)
}

/// Emit a `pty-exit:<id>` event signalling the session ended (clean close,
/// read/write error, or timeout). Shared by the simple transport read loops.
pub fn emit_pty_exit(app: &AppHandle, id: &str, message: impl Into<String>) {
    let _ = app.emit(
        &session_event("pty-exit", id),
        PtyExitEvent {
            id: id.to_string(),
            message: message.into(),
        },
    );
}

// ── Telnet IAC (Interpret As Command) stream filter ───────────────────────────

pub(crate) const IAC: u8 = 255; // Interpret As Command
pub(crate) const SB: u8 = 250; // Subnegotiation begin
pub(crate) const SE: u8 = 240; // Subnegotiation end
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;
const TELNET_BINARY: u8 = 0;
const TELNET_ECHO: u8 = 1;
const TELNET_SGA: u8 = 3;
const TELNET_TERMINAL_TYPE: u8 = 24;
const TELNET_NAWS: u8 = 31;
const TERMINAL_TYPE_IS: u8 = 0;
const TERMINAL_TYPE_SEND: u8 = 1;

#[derive(Clone, Copy, Default, PartialEq)]
enum TelnetParse {
    /// Copying in-band data.
    #[default]
    Data,
    /// Saw an `IAC` byte; the next byte is a command.
    Iac,
    /// Saw `IAC WILL|WONT|DO|DONT`; the next byte is the option being negotiated.
    Option(u8),
    /// Waiting for the option byte after `IAC SB`.
    SubnegOption,
    /// Inside an `IAC SB ... IAC SE` subnegotiation block.
    Subneg,
    /// Inside a subnegotiation and just saw an `IAC` (next byte is `SE` to end, or escaped data).
    SubnegIac,
}

/// Strips Telnet `IAC` command sequences out of a raw TCP byte stream so option
/// bytes never reach the UTF-8 decoder / terminal (which would otherwise show
/// garbage and break multi-byte decoding), and produces the bytes to write back
/// to the server.
///
/// AuraTerm accepts the small option set needed by interactive shells (binary,
/// echo, suppress-go-ahead, terminal type, and NAWS) and politely refuses all
/// other options. The parser is stateful so commands may straddle read chunks.
pub struct TelnetIacFilter {
    state: TelnetParse,
    subneg_option: Option<u8>,
    subneg_data: Vec<u8>,
    local_enabled: [bool; 256],
    remote_enabled: [bool; 256],
    /// Options we already volunteered a `WILL` for, so an incoming `DO` is an
    /// answer and must not be answered again (RFC 854 loop avoidance).
    local_offered: [bool; 256],
    /// Options we already asked for with `DO`, for the same reason.
    remote_requested: [bool; 256],
    cols: u16,
    rows: u16,
    /// Present only in RFC 2217 mode. Its presence switches the offered option
    /// set: a serial device server has neither a window nor a terminal type, so
    /// NAWS and TERMINAL-TYPE are dropped in favour of COM-PORT-OPTION.
    com_port: Option<crate::rfc2217::ComPortState>,
}

impl Default for TelnetIacFilter {
    fn default() -> Self {
        Self {
            state: TelnetParse::Data,
            subneg_option: None,
            subneg_data: Vec::new(),
            local_enabled: [false; 256],
            remote_enabled: [false; 256],
            local_offered: [false; 256],
            remote_requested: [false; 256],
            cols: 80,
            rows: 24,
            com_port: None,
        }
    }
}

/// Result of feeding a chunk through [`TelnetIacFilter::push`].
pub struct TelnetFiltered {
    /// In-band data bytes, with all IAC sequences removed.
    pub data: Vec<u8>,
    /// Bytes to write back to the server (negotiation responses); may be empty.
    pub response: Vec<u8>,
}

impl TelnetIacFilter {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_window_size(cols: u16, rows: u16) -> Self {
        Self { cols, rows, ..Self::default() }
    }

    /// Build a filter in RFC 2217 mode, driving `com_port` through the option
    /// 44 handshake.
    pub fn with_com_port(com_port: crate::rfc2217::ComPortState) -> Self {
        Self { com_port: Some(com_port), ..Self::default() }
    }

    /// Bytes to send immediately after the socket connects.
    ///
    /// Only RFC 2217 sessions open with an unsolicited offer: BINARY in both
    /// directions (a UART stream is 8-bit and must not be mangled by NVT rules),
    /// SGA (no line-at-a-time turn taking), and COM-PORT-OPTION itself. Plain
    /// Telnet keeps its purely reactive behaviour.
    pub fn initial_negotiation(&mut self) -> Vec<u8> {
        if self.com_port.is_none() {
            return Vec::new();
        }

        let mut response = Vec::new();
        for option in [TELNET_BINARY, TELNET_SGA] {
            response.extend_from_slice(&[IAC, WILL, option]);
            self.local_offered[option as usize] = true;
            response.extend_from_slice(&[IAC, DO, option]);
            self.remote_requested[option as usize] = true;
        }
        response.extend_from_slice(&[IAC, WILL, crate::rfc2217::COM_PORT_OPTION]);
        self.local_offered[crate::rfc2217::COM_PORT_OPTION as usize] = true;
        response
    }

    /// Whether outbound data may travel as raw 8-bit bytes. When false the
    /// stream is NVT and CR has to be stuffed with NUL.
    pub fn binary_out(&self) -> bool {
        self.local_enabled[TELNET_BINARY as usize]
    }

    /// Whether BINARY is in effect in both directions.
    pub fn binary_both_ways(&self) -> bool {
        self.local_enabled[TELNET_BINARY as usize] && self.remote_enabled[TELNET_BINARY as usize]
    }

    pub fn com_port(&self) -> Option<&crate::rfc2217::ComPortState> {
        self.com_port.as_ref()
    }

    pub fn com_port_mut(&mut self) -> Option<&mut crate::rfc2217::ComPortState> {
        self.com_port.as_mut()
    }

    pub fn update_window_size(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.cols = cols;
        self.rows = rows;
        if self.local_enabled[TELNET_NAWS as usize] {
            self.naws_response()
        } else {
            Vec::new()
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> TelnetFiltered {
        let mut data = Vec::with_capacity(chunk.len());
        let mut response = Vec::new();

        for &byte in chunk {
            match self.state {
                TelnetParse::Data => {
                    if byte == IAC {
                        self.state = TelnetParse::Iac;
                    } else {
                        data.push(byte);
                    }
                }
                TelnetParse::Iac => match byte {
                    IAC => {
                        // Escaped literal 0xFF data byte.
                        data.push(IAC);
                        self.state = TelnetParse::Data;
                    }
                    WILL | WONT | DO | DONT => {
                        self.state = TelnetParse::Option(byte);
                    }
                    SB => {
                        self.subneg_option = None;
                        self.subneg_data.clear();
                        self.state = TelnetParse::SubnegOption;
                    }
                    // Any other 2-byte command (NOP, GA, DM, …): consume and ignore.
                    _ => {
                        self.state = TelnetParse::Data;
                    }
                },
                TelnetParse::Option(verb) => {
                    match verb {
                        DO if self.supports_local(byte) => {
                            let already_enabled = self.local_enabled[byte as usize];
                            // A DO that answers our own WILL needs no reply.
                            let answers_offer = self.local_offered[byte as usize];
                            self.local_enabled[byte as usize] = true;
                            self.local_offered[byte as usize] = false;
                            if !already_enabled && !answers_offer {
                                response.extend_from_slice(&[IAC, WILL, byte]);
                            }
                            if byte == TELNET_NAWS {
                                response.extend_from_slice(&self.naws_response());
                            }
                            if byte == crate::rfc2217::COM_PORT_OPTION {
                                if let Some(com_port) = self.com_port.as_mut() {
                                    response.extend_from_slice(&com_port.on_do());
                                }
                            }
                        }
                        DO => response.extend_from_slice(&[IAC, WONT, byte]),
                        DONT => {
                            self.local_enabled[byte as usize] = false;
                            self.local_offered[byte as usize] = false;
                            if byte == crate::rfc2217::COM_PORT_OPTION {
                                if let Some(com_port) = self.com_port.as_mut() {
                                    com_port.on_dont();
                                }
                            }
                        }
                        WILL if supports_remote(byte) => {
                            let already_enabled = self.remote_enabled[byte as usize];
                            let answers_request = self.remote_requested[byte as usize];
                            self.remote_enabled[byte as usize] = true;
                            self.remote_requested[byte as usize] = false;
                            if !already_enabled && !answers_request {
                                response.extend_from_slice(&[IAC, DO, byte]);
                            }
                        }
                        WILL => response.extend_from_slice(&[IAC, DONT, byte]),
                        WONT => {
                            self.remote_enabled[byte as usize] = false;
                            self.remote_requested[byte as usize] = false;
                        }
                        _ => {}
                    }
                    self.state = TelnetParse::Data;
                }
                TelnetParse::SubnegOption => {
                    self.subneg_option = Some(byte);
                    self.state = TelnetParse::Subneg;
                }
                TelnetParse::Subneg => {
                    if byte == IAC {
                        self.state = TelnetParse::SubnegIac;
                    } else {
                        self.subneg_data.push(byte);
                    }
                }
                TelnetParse::SubnegIac => {
                    if byte == SE {
                        self.finish_subnegotiation(&mut response);
                        self.state = TelnetParse::Data;
                    } else if byte == IAC {
                        self.subneg_data.push(IAC);
                        self.state = TelnetParse::Subneg;
                    } else {
                        self.state = TelnetParse::Subneg;
                    }
                }
            }
        }

        TelnetFiltered { data, response }
    }

    fn finish_subnegotiation(&mut self, response: &mut Vec<u8>) {
        if self.subneg_option == Some(TELNET_TERMINAL_TYPE)
            && self.local_enabled[TELNET_TERMINAL_TYPE as usize]
            && self.subneg_data.first() == Some(&TERMINAL_TYPE_SEND)
        {
            response.extend_from_slice(&[IAC, SB, TELNET_TERMINAL_TYPE, TERMINAL_TYPE_IS]);
            response.extend_from_slice(b"xterm-256color");
            response.extend_from_slice(&[IAC, SE]);
        }

        if self.subneg_option == Some(crate::rfc2217::COM_PORT_OPTION) {
            // `subneg_data` already has `IAC IAC` collapsed back to a single
            // 0xFF by the parser above, which is what the codec expects.
            if let Some(com_port) = self.com_port.as_mut() {
                com_port.handle_subnegotiation(&self.subneg_data);
            }
        }

        self.subneg_option = None;
        self.subneg_data.clear();
    }

    fn naws_response(&self) -> Vec<u8> {
        let mut response = vec![IAC, SB, TELNET_NAWS];
        for byte in self.cols.to_be_bytes().into_iter().chain(self.rows.to_be_bytes()) {
            response.push(byte);
            if byte == IAC {
                response.push(IAC);
            }
        }
        response.extend_from_slice(&[IAC, SE]);
        response
    }
}

impl TelnetIacFilter {
    fn supports_local(&self, option: u8) -> bool {
        if self.com_port.is_some() {
            // A device server has no window and no terminal type; offering NAWS
            // or TERMINAL-TYPE there only puts noise on the wire and confuses
            // some older firmware.
            return matches!(
                option,
                TELNET_BINARY | TELNET_SGA | crate::rfc2217::COM_PORT_OPTION
            );
        }
        matches!(option, TELNET_BINARY | TELNET_SGA | TELNET_TERMINAL_TYPE | TELNET_NAWS)
    }
}

/// Double every literal `0xFF` so the peer reads it as data instead of `IAC`.
///
/// Telnet's escape rule applies to *everything* leaving the socket that is not
/// itself a command. Skipping it is invisible on ordinary text and then
/// corrupts any 8-bit payload (ZMODEM, a firmware image, a paste of binary
/// data) the moment a `0xFF` shows up.
pub(crate) fn telnet_escape_iac(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    for &byte in data {
        out.push(byte);
        if byte == IAC {
            out.push(IAC);
        }
    }
    out
}

/// Escape user data for a Telnet stream.
///
/// In BINARY mode this is just [`telnet_escape_iac`]. Otherwise the stream is
/// NVT, where a bare CR must be followed by NUL (RFC 854) — without it a peer
/// may read the CR as a line ending nobody asked for. `CR LF` is already a
/// complete NVT line ending and passes through untouched.
pub(crate) fn telnet_escape_outbound(data: &[u8], binary: bool) -> Vec<u8> {
    if binary {
        return telnet_escape_iac(data);
    }

    let mut out = Vec::with_capacity(data.len() + 8);
    let mut index = 0;
    while index < data.len() {
        match data[index] {
            IAC => out.extend_from_slice(&[IAC, IAC]),
            b'\r' => {
                out.push(b'\r');
                if data.get(index + 1) == Some(&b'\n') {
                    out.push(b'\n');
                    index += 1;
                } else {
                    // A CR that ends the chunk gets a NUL even if an LF opens
                    // the next one. Receivers discard NUL, so the worst case is
                    // one ignored byte.
                    out.push(0);
                }
            }
            byte => out.push(byte),
        }
        index += 1;
    }
    out
}

fn supports_remote(option: u8) -> bool {
    matches!(option, TELNET_BINARY | TELNET_ECHO | TELNET_SGA)
}

#[cfg(test)]
mod tests {
    use super::Utf8StreamDecoder;
    use super::session_event;
    use super::{
        telnet_escape_iac, telnet_escape_outbound, TelnetIacFilter, DO, DONT, IAC, SB, SE,
        TELNET_BINARY, TELNET_ECHO, TELNET_NAWS, TELNET_SGA, TELNET_TERMINAL_TYPE,
        TERMINAL_TYPE_SEND, WILL, WONT,
    };
    use crate::rfc2217::{
        subnegotiation, ComPortState, COM_PORT_OPTION, SET_BAUDRATE, SET_DATASIZE,
    };
    use crate::serial_params::{SerialFlowControl, SerialParams, SerialParity};

    #[test]
    fn session_event_appends_id_suffix() {
        assert_eq!(session_event("pty-output", "tab-0"), "pty-output:tab-0");
        assert_eq!(session_event("pty-exit", "tab-12"), "pty-exit:tab-12");
    }

    #[test]
    fn decodes_ascii_in_one_chunk() {
        let mut decoder = Utf8StreamDecoder::new();
        assert_eq!(decoder.push(b"hello"), "hello");
    }

    #[test]
    fn reassembles_multibyte_char_split_across_chunks() {
        // "中" is 0xE4 0xB8 0xAD in UTF-8.
        let bytes = "中".as_bytes();
        let mut decoder = Utf8StreamDecoder::new();
        // First two bytes arrive: incomplete, nothing emitted yet.
        assert_eq!(decoder.push(&bytes[..2]), "");
        // Final byte completes the character.
        assert_eq!(decoder.push(&bytes[2..]), "中");
    }

    #[test]
    fn reassembles_emoji_split_byte_by_byte() {
        // "😀" is a 4-byte sequence (U+1F600).
        let bytes = "😀".as_bytes();
        assert_eq!(bytes.len(), 4);
        let mut decoder = Utf8StreamDecoder::new();
        let mut out = String::new();
        for b in bytes {
            out.push_str(&decoder.push(&[*b]));
        }
        assert_eq!(out, "😀");
    }

    #[test]
    fn handles_mixed_ascii_and_split_multibyte() {
        let text = "ab中文cd";
        let bytes = text.as_bytes();
        let mut decoder = Utf8StreamDecoder::new();
        let mut out = String::new();
        // Split at an awkward boundary inside the first multibyte char.
        let split = 3; // 'a','b', then first byte of '中'
        out.push_str(&decoder.push(&bytes[..split]));
        out.push_str(&decoder.push(&bytes[split..]));
        assert_eq!(out, text);
    }

    #[test]
    fn replaces_genuinely_invalid_bytes() {
        let mut decoder = Utf8StreamDecoder::new();
        // 0xFF is never valid in UTF-8; it should become a replacement char,
        // and surrounding ASCII must survive.
        let out = decoder.push(&[b'a', 0xFF, b'b']);
        assert_eq!(out, "a\u{FFFD}b");
    }

    #[test]
    fn empty_chunk_yields_empty_string() {
        let mut decoder = Utf8StreamDecoder::new();
        assert_eq!(decoder.push(b""), "");
    }

    #[test]
    fn telnet_passes_plain_data_through() {
        let mut filter = TelnetIacFilter::new();
        let out = filter.push(b"hello world");
        assert_eq!(out.data, b"hello world");
        assert!(out.response.is_empty());
    }

    #[test]
    fn telnet_unescapes_doubled_iac_to_literal_ff() {
        let mut filter = TelnetIacFilter::new();
        // IAC IAC is the wire encoding of a literal 0xFF data byte.
        let out = filter.push(&[b'a', IAC, IAC, b'b']);
        assert_eq!(out.data, vec![b'a', 0xFF, b'b']);
        assert!(out.response.is_empty());
    }

    #[test]
    fn telnet_refuses_do_with_wont() {
        let mut filter = TelnetIacFilter::new();
        // Unknown options are refused.
        let out = filter.push(&[IAC, DO, 42]);
        assert!(out.data.is_empty());
        assert_eq!(out.response, vec![IAC, WONT, 42]);
    }

    #[test]
    fn telnet_refuses_will_with_dont() {
        let mut filter = TelnetIacFilter::new();
        let out = filter.push(&[IAC, WILL, 42]);
        assert!(out.data.is_empty());
        assert_eq!(out.response, vec![IAC, DONT, 42]);
    }

    #[test]
    fn telnet_does_not_reply_to_wont_dont() {
        let mut filter = TelnetIacFilter::new();
        let out = filter.push(&[IAC, WONT, 1, IAC, DONT, 3]);
        assert!(out.data.is_empty());
        assert!(out.response.is_empty());
    }

    #[test]
    fn telnet_strips_subnegotiation_block() {
        let mut filter = TelnetIacFilter::new();
        // IAC SB <option> <payload...> IAC SE, wrapped in data.
        let out = filter.push(&[b'x', IAC, SB, 24, 0, b'A', b'B', IAC, SE, b'y']);
        assert_eq!(out.data, b"xy");
        assert!(out.response.is_empty());
    }

    #[test]
    fn telnet_handles_sequence_split_across_pushes() {
        let mut filter = TelnetIacFilter::new();
        // An unsupported option split across reads is still refused.
        let first = filter.push(&[b'a', IAC]);
        assert_eq!(first.data, b"a");
        assert!(first.response.is_empty());
        let second = filter.push(&[DO, 42, b'b']);
        assert_eq!(second.data, b"b");
        assert_eq!(second.response, vec![IAC, WONT, 42]);
    }

    #[test]
    fn telnet_accepts_echo_and_reports_window_size() {
        let mut filter = TelnetIacFilter::with_window_size(120, 40);
        let echo = filter.push(&[IAC, WILL, TELNET_ECHO]);
        assert_eq!(echo.response, vec![IAC, DO, TELNET_ECHO]);

        let naws = filter.push(&[IAC, DO, TELNET_NAWS]);
        assert_eq!(
            naws.response,
            vec![IAC, WILL, TELNET_NAWS, IAC, SB, TELNET_NAWS, 0, 120, 0, 40, IAC, SE],
        );
        assert_eq!(
            filter.update_window_size(132, 50),
            vec![IAC, SB, TELNET_NAWS, 0, 132, 0, 50, IAC, SE],
        );
    }

    #[test]
    fn telnet_answers_terminal_type_subnegotiation() {
        let mut filter = TelnetIacFilter::new();
        assert_eq!(
            filter.push(&[IAC, DO, TELNET_TERMINAL_TYPE]).response,
            vec![IAC, WILL, TELNET_TERMINAL_TYPE],
        );
        let response = filter.push(&[
            IAC,
            SB,
            TELNET_TERMINAL_TYPE,
            TERMINAL_TYPE_SEND,
            IAC,
            SE,
        ]).response;
        let mut expected = vec![IAC, SB, TELNET_TERMINAL_TYPE, 0];
        expected.extend_from_slice(b"xterm-256color");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(response, expected);
    }
    // ── Outbound escaping ────────────────────────────────────────────────────

    #[test]
    fn escapes_literal_ff_on_the_way_out() {
        assert_eq!(telnet_escape_iac(&[b'a', 0xFF, b'b']), vec![b'a', IAC, IAC, b'b']);
    }

    #[test]
    fn stuffs_bare_cr_with_nul_outside_binary_mode() {
        // NVT rules: a bare CR needs a following NUL, but CR LF is already a
        // complete line ending and must pass through untouched.
        assert_eq!(telnet_escape_outbound(b"a\rb", false), vec![b'a', b'\r', 0, b'b']);
        assert_eq!(telnet_escape_outbound(b"a\r\nb", false), b"a\r\nb".to_vec());
        // In binary mode the byte stream is passed through as-is.
        assert_eq!(telnet_escape_outbound(b"a\rb", true), b"a\rb".to_vec());
    }

    #[test]
    fn escapes_ff_in_both_modes() {
        assert_eq!(telnet_escape_outbound(&[0xFF], true), vec![IAC, IAC]);
        assert_eq!(telnet_escape_outbound(&[0xFF], false), vec![IAC, IAC]);
    }

    // ── RFC 2217 (com-port) profile ──────────────────────────────────────────

    fn com_port_filter() -> TelnetIacFilter {
        TelnetIacFilter::with_com_port(ComPortState::new(
            SerialParams {
                baud_rate: 115200,
                data_bits: 8,
                stop_bits: 1,
                parity: SerialParity::None,
                flow_control: SerialFlowControl::None,
            },
            false,
        ))
    }

    #[test]
    fn com_port_opens_with_binary_sga_and_option_44() {
        let mut filter = com_port_filter();
        assert_eq!(
            filter.initial_negotiation(),
            vec![
                IAC, WILL, TELNET_BINARY,
                IAC, DO, TELNET_BINARY,
                IAC, WILL, TELNET_SGA,
                IAC, DO, TELNET_SGA,
                IAC, WILL, COM_PORT_OPTION,
            ],
        );
    }

    #[test]
    fn com_port_mode_refuses_naws_and_terminal_type() {
        // A device server has no window and no terminal type; offering them
        // only puts noise on the wire.
        let mut filter = com_port_filter();
        assert_eq!(
            filter.push(&[IAC, DO, TELNET_NAWS]).response,
            vec![IAC, WONT, TELNET_NAWS],
        );
        assert_eq!(
            filter.push(&[IAC, DO, TELNET_TERMINAL_TYPE]).response,
            vec![IAC, WONT, TELNET_TERMINAL_TYPE],
        );
    }

    #[test]
    fn do_answering_our_offer_sends_the_parameter_block_not_another_will() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();

        let response = filter.push(&[IAC, DO, COM_PORT_OPTION]).response;

        // The DO answers a WILL we already sent, so repeating it would bounce
        // negotiation back and forth (RFC 854 loop avoidance).
        assert!(!response.starts_with(&[IAC, WILL, COM_PORT_OPTION]));
        assert!(response.starts_with(&subnegotiation(
            SET_BAUDRATE,
            &115200u32.to_be_bytes(),
        )));
        assert!(filter.com_port().expect("com port").negotiated());
    }

    #[test]
    fn binary_flags_track_both_directions() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();
        assert!(!filter.binary_out());

        // Server accepts our WILL BINARY: outbound is now 8-bit clean.
        filter.push(&[IAC, DO, TELNET_BINARY]);
        assert!(filter.binary_out());
        assert!(!filter.binary_both_ways());

        // Server also offers BINARY inbound. It answers our DO, so we must not
        // send another one.
        let response = filter.push(&[IAC, WILL, TELNET_BINARY]).response;
        assert!(response.is_empty());
        assert!(filter.binary_both_ways());
    }

    #[test]
    fn com_port_subnegotiation_survives_a_split_read() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();
        filter.push(&[IAC, DO, COM_PORT_OPTION]);

        // 101 SET-BAUDRATE = 9600, arriving in two chunks.
        filter.push(&[IAC, SB, COM_PORT_OPTION, SET_BAUDRATE + 100, 0, 0]);
        let out = filter.push(&[0x25, 0x80, IAC, SE]);

        assert!(out.data.is_empty());
        let com_port = filter.com_port().expect("com port");
        assert_eq!(com_port.effective().baud_rate, 9600);
        assert_eq!(com_port.requested().baud_rate, 115200);
    }

    #[test]
    fn com_port_unescapes_doubled_ff_inside_a_subnegotiation() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();
        filter.push(&[IAC, DO, COM_PORT_OPTION]);

        // A server reporting 255 baud has to double the 0xFF; the framing layer
        // collapses it before the codec sees the value.
        filter.push(&subnegotiation(SET_BAUDRATE, &255u32.to_be_bytes()));
        assert_eq!(filter.com_port().expect("com port").effective().baud_rate, 255);
    }

    #[test]
    fn dont_on_option_44_degrades_instead_of_failing() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();

        let out = filter.push(&[IAC, DONT, COM_PORT_OPTION, b'h', b'i']);

        // Refusing the option must not cost us the data stream.
        assert_eq!(out.data, b"hi");
        let com_port = filter.com_port().expect("com port");
        assert!(com_port.settled());
        assert!(!com_port.negotiated());
    }

    #[test]
    fn plain_telnet_still_offers_naws_and_sends_no_opening_bytes() {
        // The com-port profile must not leak into ordinary Telnet sessions.
        let mut filter = TelnetIacFilter::with_window_size(80, 24);
        assert!(filter.initial_negotiation().is_empty());
        assert_eq!(
            filter.push(&[IAC, DO, TELNET_NAWS]).response,
            vec![IAC, WILL, TELNET_NAWS, IAC, SB, TELNET_NAWS, 0, 80, 0, 24, IAC, SE],
        );
    }

    #[test]
    fn data_size_reply_updates_the_effective_frame() {
        let mut filter = com_port_filter();
        filter.initial_negotiation();
        filter.push(&[IAC, DO, COM_PORT_OPTION]);
        filter.push(&subnegotiation(SET_DATASIZE + 100, &[7]));
        assert_eq!(filter.com_port().expect("com port").effective().data_bits, 7);
    }
}
