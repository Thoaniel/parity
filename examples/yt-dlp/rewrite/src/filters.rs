// --match-filter: utils.match_str / _match_one / match_filter_func.

use crate::json::{Map, Value};
use crate::util;

pub const FORMAT_FIELDS: &[&str] = &[
    "url", "manifest_url", "manifest_stream_number", "ext", "format", "format_id", "format_note", "available_at", "width", "height",
    "aspect_ratio", "resolution", "dynamic_range", "tbr", "abr", "acodec", "asr", "audio_channels", "vbr", "fps", "vcodec", "container",
    "filesize", "filesize_approx", "rows", "columns", "hls_media_playlist_data", "player_url", "protocol", "fragment_base_url",
    "fragments", "is_from_start", "is_dash_periods", "request_data", "preference", "language", "language_preference", "quality",
    "source_preference", "cookies", "http_headers", "stretched_ratio", "no_resume", "has_drm", "extra_param_to_segment_url",
    "extra_param_to_key_url", "hls_aes", "downloader_options", "impersonate", "page_url", "app", "play_path", "tc_url", "flash_version",
    "rtmp_live", "rtmp_conn", "rtmp_protocol", "rtmp_real_time",
];

#[derive(Clone, Copy)]
pub enum Incomplete<'a> {
    All(bool),
    Keys(&'a [&'a str]),
}

impl Incomplete<'_> {
    fn has(&self, k: &str) -> bool {
        match self {
            Incomplete::All(b) => *b,
            Incomplete::Keys(ks) => ks.contains(&k),
        }
    }
}

const STRING_OPS: &[&str] = &["*=", "^=", "$=", "~="];
const OPS: &[&str] = &["*=", "^=", "$=", "~=", "<=", "<", ">=", ">", "="];

fn key_len(s: &str) -> usize {
    s.chars().take_while(|c| c.is_ascii_lowercase() || *c == '_').count()
}

fn match_one(part: &str, d: &Map, incomplete: Incomplete) -> Result<bool, String> {
    let t = part.trim();
    let kl = key_len(t);
    if kl > 0 {
        let key = &t[..kl];
        let mut rest = t[kl..].trim_start();
        let negation = rest.starts_with('!');
        if negation {
            rest = rest[1..].trim_start();
        }
        if let Some(op) = OPS.iter().find(|op| rest.starts_with(**op)) {
            let mut r = &rest[op.len()..];
            let trimmed = r.trim_start();
            let none_inclusive = trimmed.starts_with('?');
            if none_inclusive {
                r = &trimmed[1..];
            }
            let r = r.trim_start();
            if !r.is_empty() {
                let (quote, value) = match r.chars().next() {
                    Some(q @ ('"' | '\'')) if r.len() >= 3 && r.ends_with(q) => (Some(q), r[1..r.len() - 1].to_string()),
                    _ => (None, r.to_string()),
                };
                let value = match quote {
                    Some(q) => value.replace(&format!("\\{q}"), &q.to_string()),
                    None => value,
                };
                let actual = d.g(key);
                let mut numeric: Option<f64> = None;
                if actual.is_number() {
                    numeric = value
                        .trim()
                        .parse::<i64>()
                        .ok()
                        .map(|i| i as f64)
                        .or_else(|| util::parse_filesize(&value))
                        .or_else(|| util::parse_filesize(&format!("{value}B")))
                        .or_else(|| util::parse_duration(&value));
                }
                if numeric.is_some() && STRING_OPS.contains(op) {
                    return Err(format!("Operator {op} only supports string values!"));
                }
                if actual.is_null() {
                    return Ok(incomplete.has(key) || none_inclusive);
                }
                let res = compare(actual, op, &value, numeric)?;
                return Ok(res != negation);
            }
        }
    }
    // Unary: (!)?\s*key
    let (neg, rest) = match t.strip_prefix('!') {
        Some(r) => (true, r.trim_start()),
        None => (false, t),
    };
    if !rest.is_empty() && key_len(rest) == rest.len() {
        let actual = d.g(rest);
        if incomplete.has(rest) && actual.is_null() {
            return Ok(true);
        }
        return Ok(match (neg, actual) {
            (false, Value::Bool(b)) => *b,
            (false, v) => !v.is_null(),
            (true, Value::Bool(b)) => !*b,
            (true, v) => v.is_null(),
        });
    }
    Err(format!("Invalid filter part {}", crate::pyfmt::str_repr(part)))
}

fn compare(actual: &Value, op: &str, value: &str, numeric: Option<f64>) -> Result<bool, String> {
    if let Some(n) = numeric {
        let a = actual.as_f64().unwrap();
        return Ok(match op {
            "<=" => a <= n,
            "<" => a < n,
            ">=" => a >= n,
            ">" => a > n,
            _ => a == n,
        });
    }
    match (op, actual) {
        ("*=", Value::List(l)) => Ok(l.iter().any(|v| v.as_str() == Some(value))),
        ("*=", Value::Str(s)) => Ok(s.contains(value)),
        ("^=", Value::Str(s)) => Ok(s.starts_with(value)),
        ("$=", Value::Str(s)) => Ok(s.ends_with(value)),
        ("~=", Value::Str(s)) => Ok(crate::miniregex::Regex::new(value).is_some_and(|r| r.search(s))),
        ("=", Value::Str(s)) => Ok(s == value),
        ("=", _) => Ok(false),
        (_, Value::Str(s)) => Ok(match op {
            "<=" => s.as_str() <= value,
            "<" => s.as_str() < value,
            ">=" => s.as_str() >= value,
            _ => s.as_str() > value,
        }),
        _ => Err(format!(
            "'{}' not supported between instances of '{}' and 'str'",
            op,
            crate::pyfmt::type_name(actual)
        )),
    }
}

/// `re.split(r'(?<!\\)&', s)` then `\&` -> `&` in each part.
fn split_parts(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut prev = '\0';
    for c in s.chars() {
        if c == '&' && prev != '\\' {
            parts.push(std::mem::take(&mut cur));
        } else {
            cur.push(c);
        }
        prev = c;
    }
    parts.push(cur);
    parts.into_iter().map(|p| p.replace("\\&", "&")).collect()
}

pub fn match_str(filter: &str, d: &Map, incomplete: Incomplete) -> Result<bool, String> {
    for part in split_parts(filter) {
        if !match_one(&part, d, incomplete)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The `_match_func` built by match_filter_func: None when the video passes, else the reason.
pub fn match_filters(filters: &[String], d: &Map, incomplete: Incomplete) -> Result<Option<String>, String> {
    if filters.is_empty() {
        return Ok(None);
    }
    for f in filters {
        if match_str(f, d, incomplete)? {
            return Ok(None);
        }
    }
    let title = [d.g("title"), d.g("id")].into_iter().find(|v| v.truthy()).map(crate::pyfmt::py_str).unwrap_or_else(|| "entry".into());
    let joined = filters.iter().map(|f| f.trim()).collect::<Vec<_>>().join(") | (");
    Ok(Some(format!("{title} does not pass filter ({joined}), skipping ..")))
}
