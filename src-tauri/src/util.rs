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

/// Decode a freshly read transport chunk and emit it to the owning terminal as
/// a `pty-output:<id>` event. An empty result (the chunk ended on an incomplete
/// multi-byte sequence the decoder is still buffering) is skipped rather than
/// emitting a no-op event. Shared by the local-PTY, serial and telnet read
/// loops so the decode + skip-empty + per-session emit logic lives in one place.
pub fn emit_pty_output(app: &AppHandle, id: &str, decoder: &mut Utf8StreamDecoder, chunk: &[u8]) {
    let output = decoder.push(chunk);
    if output.is_empty() {
        return;
    }
    let _ = app.emit(
        &session_event("pty-output", id),
        PtyOutputEvent {
            id: id.to_string(),
            data: output,
        },
    );
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

#[derive(Default, PartialEq)]
enum TelnetParse {
    /// Copying in-band data.
    #[default]
    Data,
    /// Saw an `IAC` byte; the next byte is a command.
    Iac,
    /// Saw `IAC WILL|WONT|DO|DONT`; the next byte is the option being negotiated.
    Option(u8),
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
/// AuraTerm is a client that drives an interactive shell; it has no use for the
/// negotiable options, so it **politely refuses every one**: `IAC DO x` →
/// `IAC WONT x`, `IAC WILL x` → `IAC DONT x`. The server's `WONT`/`DONT`
/// acknowledgements need no reply (replying could loop). `IAC IAC` is unescaped
/// to a single literal `0xFF` data byte. The parser is stateful so sequences may
/// straddle read boundaries.
#[derive(Default)]
pub struct TelnetIacFilter {
    state: TelnetParse,
}

/// Result of feeding a chunk through [`TelnetIacFilter::push`].
pub struct TelnetFiltered {
    /// In-band data bytes, with all IAC sequences removed.
    pub data: Vec<u8>,
    /// Bytes to write back to the server (negotiation responses); may be empty.
    pub response: Vec<u8>,
}

impl TelnetIacFilter {
    pub fn new() -> Self {
        Self::default()
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
                        self.state = TelnetParse::Subneg;
                    }
                    // Any other 2-byte command (NOP, GA, DM, …): consume and ignore.
                    _ => {
                        self.state = TelnetParse::Data;
                    }
                },
                TelnetParse::Option(verb) => {
                    match verb {
                        // Server asks us to enable an option, or announces it will:
                        // refuse both directions.
                        DO => response.extend_from_slice(&[IAC, WONT, byte]),
                        WILL => response.extend_from_slice(&[IAC, DONT, byte]),
                        // WONT/DONT are acknowledgements; no reply.
                        _ => {}
                    }
                    self.state = TelnetParse::Data;
                }
                TelnetParse::Subneg => {
                    if byte == IAC {
                        self.state = TelnetParse::SubnegIac;
                    }
                    // else: discard subnegotiation payload bytes.
                }
                TelnetParse::SubnegIac => {
                    if byte == SE {
                        self.state = TelnetParse::Data;
                    } else {
                        // IAC inside SB escaping data, or a stray command; stay in subneg.
                        self.state = TelnetParse::Subneg;
                    }
                }
            }
        }

        TelnetFiltered { data, response }
    }
}

#[cfg(test)]
mod tests {
    use super::Utf8StreamDecoder;
    use super::session_event;
    use super::{TelnetIacFilter, DO, DONT, IAC, SB, SE, WILL, WONT};

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
        // Server: IAC DO ECHO(1) -> client refuses: IAC WONT ECHO.
        let out = filter.push(&[IAC, DO, 1]);
        assert!(out.data.is_empty());
        assert_eq!(out.response, vec![IAC, WONT, 1]);
    }

    #[test]
    fn telnet_refuses_will_with_dont() {
        let mut filter = TelnetIacFilter::new();
        // Server: IAC WILL SGA(3) -> client refuses: IAC DONT SGA.
        let out = filter.push(&[IAC, WILL, 3]);
        assert!(out.data.is_empty());
        assert_eq!(out.response, vec![IAC, DONT, 3]);
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
        // IAC DO ECHO split: IAC arrives, then DO 1 in the next read.
        let first = filter.push(&[b'a', IAC]);
        assert_eq!(first.data, b"a");
        assert!(first.response.is_empty());
        let second = filter.push(&[DO, 1, b'b']);
        assert_eq!(second.data, b"b");
        assert_eq!(second.response, vec![IAC, WONT, 1]);
    }
}
