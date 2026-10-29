//! Printable strings in any file: ASCII/UTF-8 runs and UTF-16LE runs, with their file offsets.

use regex::Regex;
use serde::Serialize;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    Utf8,
    Utf16le,
}

#[derive(Debug, Serialize)]
pub struct FoundString {
    pub offset: usize,
    pub encoding: Encoding,
    pub text: String,
}

fn printable(b: u8) -> bool {
    b == b'\t' || (0x20..0x7f).contains(&b)
}

/// Strings of at least `min_len` characters, those matching `filter` only, at most `limit` (0 = all).
pub fn strings(
    data: &[u8],
    min_len: usize,
    filter: Option<&Regex>,
    limit: usize,
) -> Vec<FoundString> {
    let min_len = min_len.max(1);
    let mut out = Vec::new();
    let mut keep = |offset, encoding, text: String| {
        if filter.is_none_or(|f| f.is_match(&text)) {
            out.push(FoundString {
                offset,
                encoding,
                text,
            });
        }
    };

    // UTF-8: printable ASCII plus well-formed multi-byte sequences.
    let mut start = 0;
    let mut i = 0;
    while i <= data.len() {
        let step = if i < data.len() {
            if printable(data[i]) {
                Some(1)
            } else {
                utf8_len(&data[i..])
            }
        } else {
            None
        };
        match step {
            Some(n) => i += n,
            None => {
                if let Ok(text) = std::str::from_utf8(&data[start..i])
                    && text.chars().count() >= min_len
                {
                    keep(start, Encoding::Utf8, text.to_string());
                }
                i += 1;
                start = i;
            }
        }
    }

    // UTF-16LE: printable ASCII code units (the common case in PE resources and Windows strings).
    for parity in 0..2 {
        let mut run_start = None;
        let mut text = String::new();
        let mut i = parity;
        while i + 1 < data.len() + 1 {
            let unit = (i + 1 < data.len()).then(|| (data[i], data[i + 1]));
            match unit {
                Some((lo, 0)) if printable(lo) => {
                    run_start.get_or_insert(i);
                    text.push(lo as char);
                }
                _ => {
                    if let Some(s) = run_start.take()
                        && text.len() >= min_len
                    {
                        keep(s, Encoding::Utf16le, std::mem::take(&mut text));
                    }
                    text.clear();
                }
            }
            i += 2;
        }
    }

    out.sort_by_key(|s| s.offset);
    if limit > 0 {
        out.truncate(limit);
    }
    out
}

fn utf8_len(bytes: &[u8]) -> Option<usize> {
    let n = match bytes[0] {
        0xC2..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF4 => 4,
        _ => return None,
    };
    let seq = bytes.get(..n)?;
    std::str::from_utf8(seq).ok().map(|_| n)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_utf8_and_utf16_runs() {
        let mut data = b"\x00\x01hello world\x00ab\x00h\xc3\xa9llo\xff".to_vec();
        data.extend("WIDE".encode_utf16().flat_map(|u| u.to_le_bytes()));
        data.extend([0, 0]);
        let found = strings(&data, 4, None, 0);
        let texts: Vec<_> = found
            .iter()
            .map(|s| (s.encoding, s.text.as_str()))
            .collect();
        assert!(texts.contains(&(Encoding::Utf8, "hello world")));
        assert!(texts.contains(&(Encoding::Utf8, "héllo")));
        assert!(texts.contains(&(Encoding::Utf16le, "WIDE")));
        assert!(!texts.iter().any(|(_, t)| *t == "ab"));
    }

    #[test]
    fn filters_and_limits() {
        let re = Regex::new("^https?://").unwrap();
        let found = strings(b"x\0https://example.org\0http://a.b\0nope", 3, Some(&re), 1);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].text, "https://example.org");
    }
}
