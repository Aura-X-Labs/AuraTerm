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

const IAC: u8 = 255; // Interpret As Command
const SB: u8 = 250; // Subnegotiation begin
const SE: u8 = 240; // Subnegotiation end
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
    cols: u16,
    rows: u16,
}

impl Default for TelnetIacFilter {
    fn default() -> Self {
        Self {
            state: TelnetParse::Data,
            subneg_option: None,
            subneg_data: Vec::new(),
            local_enabled: [false; 256],
            remote_enabled: [false; 256],
            cols: 80,
            rows: 24,
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
                        DO if supports_local(byte) => {
                            if !self.local_enabled[byte as usize] {
                                self.local_enabled[byte as usize] = true;
                                response.extend_from_slice(&[IAC, WILL, byte]);
                            }
                            if byte == TELNET_NAWS {
                                response.extend_from_slice(&self.naws_response());
                            }
                        }
                        DO => response.extend_from_slice(&[IAC, WONT, byte]),
                        DONT => self.local_enabled[byte as usize] = false,
                        WILL if supports_remote(byte) => {
                            if !self.remote_enabled[byte as usize] {
                                self.remote_enabled[byte as usize] = true;
                                response.extend_from_slice(&[IAC, DO, byte]);
                            }
                        }
                        WILL => response.extend_from_slice(&[IAC, DONT, byte]),
                        WONT => self.remote_enabled[byte as usize] = false,
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

fn supports_local(option: u8) -> bool {
    matches!(option, TELNET_BINARY | TELNET_SGA | TELNET_TERMINAL_TYPE | TELNET_NAWS)
}

fn supports_remote(option: u8) -> bool {
    matches!(option, TELNET_BINARY | TELNET_ECHO | TELNET_SGA)
}

#[cfg(test)]
mod tests {
    use super::Utf8StreamDecoder;
    use super::session_event;
    use super::{
        TelnetIacFilter, DO, DONT, IAC, SB, SE, TELNET_ECHO, TELNET_NAWS,
        TELNET_TERMINAL_TYPE, TERMINAL_TYPE_SEND, WILL, WONT,
    };

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
}
