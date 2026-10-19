//! Port of streaming UTF-8 decoder (string_decoder).
//!
//! node-pty hands PTY output to JS through `socket.setEncoding('utf8')`, i.e. Node's `StringDecoder('utf8')`, and the
//! daemon/client/cli decode their sockets the same way. This is a byte-for-byte port of Node's C++
//! `StringDecoder::DecodeData`/`FlushData` for UTF-8: an incomplete trailing sequence is held back until the next
//! chunk, a held sequence broken by a non-continuation byte is flushed (as U+FFFD) before the new byte, and invalid
//! input becomes U+FFFD per maximal subpart (V8's decoder; `String::from_utf8_lossy` replaces identically).
//! Vectors recorded from Node live in `tests/data/string_decoder_utf8.json`.

/// Longest UTF-8 sequence Node keeps across chunks (`kIncompleteCharactersEnd`).
const MAX_BUFFERED: usize = 4;

/// TS: node:string_decoder StringDecoder('utf8')
#[derive(Debug, Clone, Default)]
pub struct Utf8Decoder {
    buffer: [u8; MAX_BUFFERED],
    buffered: usize,
    missing: usize,
}

fn decode(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

impl Utf8Decoder {
    /// TS: new StringDecoder('utf8')
    pub fn new() -> Self {
        Self::default()
    }

    /// TS: StringDecoder.write(buffer) — every complete character so far; an incomplete tail is kept for later.
    pub fn write(&mut self, chunk: &[u8]) -> String {
        let mut data = chunk;
        let mut prepend = String::new();

        if self.missing > 0 {
            // A byte that should have continued the held character starts a new one instead: the held bytes (plus
            // the continuation bytes seen before it) are decoded on their own.
            let limit = data.len().min(self.missing);
            if let Some(index) = data[..limit].iter().position(|byte| byte & 0xC0 != 0x80) {
                self.missing = 0;
                self.hold(&data[..index]);
                data = &data[index..];
            }
            let found = data.len().min(self.missing);
            self.hold(&data[..found]);
            data = &data[found..];
            self.missing -= found;
            if self.missing == 0 {
                prepend = decode(&self.buffer[..self.buffered]);
                self.buffered = 0;
            }
        }

        if data.is_empty() {
            return prepend;
        }

        self.find_incomplete_tail(data);
        let keep = data.len() - self.buffered;
        let tail = &data[keep..];
        self.buffer[..tail.len()].copy_from_slice(tail);
        prepend.push_str(&decode(&data[..keep]));
        prepend
    }

    /// TS: StringDecoder.end() — flushes a held incomplete sequence (as U+FFFD) and resets the decoder.
    pub fn end(&mut self) -> String {
        let text = if self.buffered == 0 { String::new() } else { decode(&self.buffer[..self.buffered]) };
        self.buffered = 0;
        self.missing = 0;
        text
    }

    /// Bytes currently held back as an incomplete character.
    pub fn pending(&self) -> &[u8] {
        &self.buffer[..self.buffered]
    }

    fn hold(&mut self, bytes: &[u8]) {
        let start = self.buffered;
        let end = (start + bytes.len()).min(MAX_BUFFERED);
        self.buffer[start..end].copy_from_slice(&bytes[..end - start]);
        self.buffered = end;
    }

    /// Sets `buffered`/`missing` for the character cut off at the end of `data`, exactly as Node scans for it.
    fn find_incomplete_tail(&mut self, data: &[u8]) {
        self.buffered = 0;
        self.missing = 0;
        let Some(&last) = data.last() else { return };
        if last & 0x80 == 0 {
            return;
        }
        for index in (0..data.len()).rev() {
            self.buffered += 1;
            let byte = data[index];
            if byte & 0xC0 == 0x80 {
                // A continuation byte: keep looking for the lead unless the character could not fit anyway.
                if self.buffered >= MAX_BUFFERED || index == 0 {
                    self.buffered = 0;
                    return;
                }
                continue;
            }
            let length = if byte & 0xE0 == 0xC0 {
                2
            } else if byte & 0xF0 == 0xE0 {
                3
            } else if byte & 0xF8 == 0xF0 {
                4
            } else {
                // Not a lead byte of any representable character.
                self.buffered = 0;
                return;
            };
            if self.buffered >= length {
                // Complete (or over-long and invalid anyway): nothing to hold back.
                self.buffered = 0;
                return;
            }
            self.missing = length - self.buffered;
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_chunks(chunks: &[&[u8]]) -> (Vec<String>, String) {
        let mut decoder = Utf8Decoder::new();
        let writes = chunks.iter().map(|chunk| decoder.write(chunk)).collect();
        (writes, decoder.end())
    }

    #[test]
    fn holds_split_multibyte_characters() {
        let euro = "€".as_bytes();
        let (writes, end) = decode_chunks(&[&euro[..1], &euro[1..2], &euro[2..]]);
        assert_eq!(writes, ["", "", "€"]);
        assert_eq!(end, "");
    }

    #[test]
    fn flushes_broken_sequence_before_new_byte() {
        let (writes, end) = decode_chunks(&[&[0xE2, 0x82], b"A"]);
        assert_eq!(writes, ["", "\u{FFFD}A"]);
        assert_eq!(end, "");
    }

    #[test]
    fn end_replaces_incomplete_tail_once() {
        let (writes, end) = decode_chunks(&[&[0xF0, 0x9F, 0x98]]);
        assert_eq!(writes, [""]);
        assert_eq!(end, "\u{FFFD}");
    }

    #[test]
    fn surrogates_and_overlongs_replace_per_byte() {
        let (writes, _) = decode_chunks(&[&[0xED, 0xA0, 0x80, 0xC0, 0x80]]);
        assert_eq!(writes, ["\u{FFFD}".repeat(5)]);
    }

    #[test]
    fn pending_reports_held_bytes() {
        let mut decoder = Utf8Decoder::new();
        assert_eq!(decoder.write(&[0x61, 0xE4, 0xB8]), "a");
        assert_eq!(decoder.pending(), &[0xE4, 0xB8]);
        assert_eq!(decoder.write(&[0xAD]), "中");
        assert!(decoder.pending().is_empty());
    }
}
