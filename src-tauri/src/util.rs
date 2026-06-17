//! Shared low-level utilities used across transport modules and persistence.

use std::fs;
use std::io::Write;
use std::path::Path;

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

#[cfg(test)]
mod tests {
    use super::Utf8StreamDecoder;

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
}
