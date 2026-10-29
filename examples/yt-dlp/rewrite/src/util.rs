// Ports of yt_dlp.utils helpers.

use crate::json::{Map, Value};
use crate::pyfmt::{self, Spec, py_str};

/// Python `float(str)`: surrounding whitespace, underscores between digits, inf/nan.
pub fn float_str(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    let lower = t.to_ascii_lowercase();
    let body = lower.trim_start_matches(['+', '-']);
    if matches!(body, "inf" | "infinity" | "nan") {
        return lower.parse::<f64>().ok().or(Some(if body == "nan" { f64::NAN } else if lower.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY }));
    }
    let ok = t.chars().all(|c| c.is_ascii_digit() || "+-.eE_".contains(c));
    if !ok || t.contains("__") || t.starts_with('_') || t.ends_with('_') {
        return None;
    }
    t.replace('_', "").parse().ok()
}

/// `float_or_none(v)`.
pub fn float_or_none(v: &Value) -> Option<f64> {
    match v {
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => v.as_f64(),
        Value::Str(s) => float_str(s),
        _ => None,
    }
}

/// `int_or_none(v)` for JSON values.
pub fn int_or_none(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Bool(b) => Some(*b as i64),
        Value::Float(f) if f.is_finite() => Some(f.trunc() as i64),
        Value::Str(s) => s.trim().parse().ok(),
        _ => None,
    }
}

pub fn format_seconds(secs: f64, delim: &str) -> String {
    let msec = secs * 1000.0;
    let s = (msec / 1000.0).floor();
    let mins = (s / 60.0).floor();
    let sec = s - mins * 60.0;
    let hrs = (mins / 60.0).floor();
    let min = mins - hrs * 60.0;
    if hrs != 0.0 {
        format!("{}{delim}{:02}{delim}{:02}", hrs as i64, min as i64, sec as i64)
    } else if min != 0.0 {
        format!("{}{delim}{:02}", min as i64, sec as i64)
    } else {
        format!("{}", sec as i64)
    }
}

/// YoutubeDL.format_resolution.
pub fn format_resolution(f: &Map, default: Option<&str>) -> Option<String> {
    if f.str_of("vcodec") == Some("none") && f.str_of("acodec") != Some("none") {
        return Some("audio only".into());
    }
    if !f.g("resolution").is_null() {
        return Some(py_str(f.g("resolution")));
    }
    let w = f.g("width");
    let h = f.g("height");
    let d = |v: &Value| int_or_none(v).unwrap_or(0);
    if w.truthy() && h.truthy() {
        return Some(format!("{}x{}", d(w), d(h)));
    } else if h.truthy() {
        return Some(format!("{}p", py_str(h)));
    } else if w.truthy() {
        return Some(format!("{}x?", d(w)));
    }
    default.map(String::from)
}

// ---------------------------------------------------------------- dates

/// Days since 1970-01-01 to (year, month, day).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = if m > 2 { m - 3 } else { m + 9 } as i64;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

#[derive(Clone, Copy, Debug)]
pub struct DateTime {
    pub year: i64,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    pub micro: u32,
}

impl DateTime {
    pub fn from_timestamp(ts: f64) -> Option<DateTime> {
        if !ts.is_finite() {
            return None;
        }
        let micros = (ts * 1e6).round() as i128;
        let secs = micros.div_euclid(1_000_000) as i64;
        let micro = micros.rem_euclid(1_000_000) as u32;
        let days = secs.div_euclid(86400);
        let rem = secs.rem_euclid(86400);
        let (year, month, day) = civil_from_days(days);
        if !(1..=9999).contains(&year) {
            return None;
        }
        Some(DateTime { year, month, day, hour: (rem / 3600) as u32, minute: (rem / 60 % 60) as u32, second: (rem % 60) as u32, micro })
    }
    pub fn timestamp(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) * 86400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }
    pub fn weekday(&self) -> u32 {
        // Monday = 0
        (days_from_civil(self.year, self.month, self.day) + 3).rem_euclid(7) as u32
    }
    fn yday(&self) -> i64 {
        days_from_civil(self.year, self.month, self.day) - days_from_civil(self.year, 1, 1) + 1
    }
    pub fn strftime(&self, fmt: &str) -> String {
        const DAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
        const MONTHS: [&str; 12] = [
            "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
        ];
        let mut out = String::new();
        let mut it = fmt.chars().peekable();
        while let Some(c) = it.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            let Some(d) = it.next() else {
                out.push('%');
                break;
            };
            let wd = self.weekday();
            match d {
                'Y' => out.push_str(&self.year.to_string()),
                'y' => out.push_str(&format!("{:02}", self.year % 100)),
                'C' => out.push_str(&format!("{:02}", self.year / 100)),
                'm' => out.push_str(&format!("{:02}", self.month)),
                'd' => out.push_str(&format!("{:02}", self.day)),
                'e' => out.push_str(&format!("{:2}", self.day)),
                'H' => out.push_str(&format!("{:02}", self.hour)),
                'I' => out.push_str(&format!("{:02}", if self.hour % 12 == 0 { 12 } else { self.hour % 12 })),
                'M' => out.push_str(&format!("{:02}", self.minute)),
                'S' => out.push_str(&format!("{:02}", self.second)),
                'f' => out.push_str(&format!("{:06}", self.micro)),
                'p' => out.push_str(if self.hour < 12 { "AM" } else { "PM" }),
                'j' => out.push_str(&format!("{:03}", self.yday())),
                'a' => out.push_str(&DAYS[wd as usize][..3]),
                'A' => out.push_str(DAYS[wd as usize]),
                'b' | 'h' => out.push_str(&MONTHS[self.month as usize - 1][..3]),
                'B' => out.push_str(MONTHS[self.month as usize - 1]),
                'w' => out.push_str(&((wd + 1) % 7).to_string()),
                'u' => out.push_str(&(wd + 1).to_string()),
                'D' => out.push_str(&self.strftime("%m/%d/%y")),
                'F' => out.push_str(&self.strftime("%Y-%m-%d")),
                'T' => out.push_str(&self.strftime("%H:%M:%S")),
                'R' => out.push_str(&self.strftime("%H:%M")),
                'c' => out.push_str(&self.strftime("%a %b %e %H:%M:%S %Y")),
                'x' => out.push_str(&self.strftime("%m/%d/%y")),
                'X' => out.push_str(&self.strftime("%H:%M:%S")),
                's' => out.push_str(&self.timestamp().to_string()),
                'z' | 'Z' => {}
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '%' => out.push('%'),
                other => {
                    out.push('%');
                    out.push(other);
                }
            }
        }
        out
    }
}

/// `datetime.strptime(s, '%Y%m%d')`.
pub fn parse_yyyymmdd(s: &str) -> Option<DateTime> {
    if s.len() != 8 || !s.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let y: i64 = s[..4].parse().ok()?;
    let m: u32 = s[4..6].parse().ok()?;
    let d: u32 = s[6..].parse().ok()?;
    let mdays = [31, if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 { 29 } else { 28 }, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    if !(1..=12).contains(&m) || d < 1 || d > mdays[m as usize - 1] || y < 1 {
        return None;
    }
    Some(DateTime { year: y, month: m, day: d, hour: 0, minute: 0, second: 0, micro: 0 })
}

pub fn strftime_or_none(v: &Value, fmt: &str) -> Option<String> {
    let dt = match v {
        Value::Int(_) | Value::Float(_) | Value::Bool(_) => DateTime::from_timestamp(v.as_f64()?)?,
        Value::Str(s) => parse_yyyymmdd(s)?,
        _ => return None,
    };
    Some(dt.strftime(fmt))
}

// ---------------------------------------------------------------- strings

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

/// `shlex.quote`.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".into();
    }
    if s.chars().all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

pub fn format_decimal_suffix(v: &Value, num_fmt: &str, factor: f64) -> Option<String> {
    let num = float_or_none(v)?;
    if num < 0.0 {
        return None;
    }
    const SUFFIXES: [&str; 9] = ["", "k", "M", "G", "T", "P", "E", "Z", "Y"];
    let exponent = if num == 0.0 { 0 } else { ((num.ln() / factor.ln()) as i64).min(8) as usize };
    let mut suffix = SUFFIXES[exponent].to_string();
    if factor == 1024.0 {
        suffix = match suffix.as_str() {
            "k" => "Ki".into(),
            "" => "".into(),
            s => format!("{s}i"),
        };
    }
    let converted = num / factor.powi(exponent as i32);
    let body = if num_fmt.is_empty() {
        pyfmt::format_value(&Spec::parse("d").unwrap(), &Value::Float(converted)).ok()?
    } else {
        pyfmt::format_value(&Spec::parse(&format!("{num_fmt}f")).unwrap(), &Value::Float(converted)).ok()?
    };
    Some(format!("{body}{suffix}"))
}

/// `sanitize_filename(s, restricted, is_id)`; `is_id: None` is NO_DEFAULT (the new rules).
pub fn sanitize_filename(s: &str, restricted: bool, is_id: Option<bool>) -> String {
    if s.is_empty() {
        return String::new();
    }
    let no_default = is_id.is_none();
    // Handle timestamps: digits(:digits)+ -> underscores
    let chars: Vec<char> = s.chars().collect();
    let mut t: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() {
            let mut j = i;
            while j < chars.len() && chars[j].is_ascii_digit() {
                j += 1;
            }
            let mut end = j;
            let mut k = j;
            while k < chars.len() && chars[k] == ':' && chars.get(k + 1).is_some_and(|c| c.is_ascii_digit()) {
                k += 1;
                while k < chars.len() && chars[k].is_ascii_digit() {
                    k += 1;
                }
                end = k;
            }
            for &c in &chars[i..end] {
                t.push(if c == ':' { '_' } else { c });
            }
            i = end;
        } else {
            t.push(chars[i]);
            i += 1;
        }
    }
    let mut result = String::new();
    for c in t {
        result.push_str(&replace_insane(c, restricted, no_default));
    }
    if no_default {
        result = collapse_substitutes(&result);
        result = strip_substitutes(&result);
    }
    let mut result = result.replace('\0', "");
    if result.is_empty() {
        result = "_".into();
    }
    if is_id == Some(false) {
        while result.contains("__") {
            result = result.replace("__", "_");
        }
        result = result.trim_matches('_').to_string();
        if restricted && result.starts_with("-_") {
            result = result[2..].to_string();
        }
        if let Some(r) = result.strip_prefix('-') {
            result = format!("_{r}");
        }
        result = result.trim_start_matches('.').to_string();
        if result.is_empty() {
            result = "_".into();
        }
    }
    result
}

fn replace_insane(c: char, restricted: bool, no_default: bool) -> String {
    if restricted {
        if let Some(r) = accent_char(c) {
            return r;
        }
    }
    if !restricted && c == '\n' {
        return "\0 ".into();
    }
    if no_default && !restricted && "\"*:<>?|/\\".contains(c) {
        return match c {
            '/' => "\u{29F8}".into(),
            '\\' => "\u{29f9}".into(),
            _ => char::from_u32(c as u32 + 0xfee0).unwrap().to_string(),
        };
    }
    if c == '?' || (c as u32) < 32 || c as u32 == 127 {
        return String::new();
    }
    if c == '"' {
        return if restricted { String::new() } else { "'".into() };
    }
    if c == ':' {
        return if restricted { "\0_\0-".into() } else { "\0 \0-".into() };
    }
    if "\\/|*<>".contains(c) {
        return "\0_".into();
    }
    if restricted && ("!&'()[]{}$;`^,#".contains(c) || c.is_whitespace() || c as u32 > 127) {
        return if is_category_c_or_m(c) { String::new() } else { "\0_".into() };
    }
    c.to_string()
}

fn is_category_c_or_m(c: char) -> bool {
    let u = c as u32;
    u < 0x20 || (0x7f..0xa0).contains(&u) || u == 0xad || (0x300..0x370).contains(&u) || (0x200b..0x2010).contains(&u)
        || (0x202a..0x202f).contains(&u) || (0x2060..0x2070).contains(&u) || u == 0xfeff || (0xfe00..0xfe10).contains(&u)
        || (0x1ab0..0x1b00).contains(&u) || (0x1dc0..0x1e00).contains(&u) || (0x20d0..0x2100).contains(&u)
        || (0xe000..0xf900).contains(&u)
}

/// ACCENT_CHARS from yt_dlp.utils.
fn accent_char(c: char) -> Option<String> {
    const FROM: &str = "ÂÃÄÀÁÅÆÇÈÉÊËÌÍÎÏÐÑÒÓÔÕÖŐØŒÙÚÛÜŰÝÞßàáâãäåæçèéêëìíîïðñòóôõöőøœùúûüűýþÿ";
    // itertools.chain('AAAAAA', ['AE'], 'CEEEEIIIIDNOOOOOOO', ['OE'], 'UUUUUY', ['TH', 'ss'],
    //                 'aaaaaa', ['ae'], 'ceeeeiiiionooooooo', ['oe'], 'uuuuuy', ['th'], 'y')
    let mut to: Vec<String> = Vec::new();
    let chars = |s: &str, to: &mut Vec<String>| to.extend(s.chars().map(String::from));
    chars("AAAAAA", &mut to);
    to.push("AE".into());
    chars("CEEEEIIIIDNOOOOOOO", &mut to);
    to.push("OE".into());
    chars("UUUUUY", &mut to);
    to.push("TH".into());
    to.push("ss".into());
    chars("aaaaaa", &mut to);
    to.push("ae".into());
    chars("ceeeeiiiionooooooo", &mut to);
    to.push("oe".into());
    chars("uuuuuy", &mut to);
    to.push("th".into());
    to.push("y".into());
    FROM.chars().position(|x| x == c).and_then(|i| to.get(i).cloned())
}

/// `re.sub(r'(\0.)(?:(?=\1)..)+', r'\1', s)`
fn collapse_substitutes(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\0' && i + 1 < c.len() && c[i + 1] != '\n' {
            let pair = (c[i], c[i + 1]);
            let mut j = i + 2;
            while j + 1 < c.len() && (c[j], c[j + 1]) == pair {
                j += 2;
            }
            out.push(pair.0);
            out.push(pair.1);
            i = j;
        } else {
            out.push(c[i]);
            i += 1;
        }
    }
    out
}

/// Tokenizes `(?:\0.|[ _-])*` from p; returns the end and whether the last token was `\0.`.
fn strip_run(c: &[char], p: usize) -> (usize, bool) {
    let mut q = p;
    let mut last_sub = false;
    loop {
        if c.get(q) == Some(&'\0') && q + 1 < c.len() && c[q + 1] != '\n' {
            q += 2;
            last_sub = true;
        } else if c.get(q).is_some_and(|x| " _-".contains(*x)) {
            q += 1;
            last_sub = false;
        } else {
            return (q, last_sub);
        }
    }
}

/// `re.sub(f'^\0.{STRIP_RE}|{STRIP_RE}\0.$', '', s)`
fn strip_substitutes(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut start = 0;
    if c.first() == Some(&'\0') && c.len() > 1 && c[1] != '\n' {
        start = strip_run(&c, 2).0;
    }
    let mut end = c.len();
    for i in start..c.len() {
        // The run from i must reach the end, ending on a `\0.` token. Backtracking lets the
        // greedy run give up trailing tokens only if they are `\0.`, so check the full run.
        let (q, last_sub) = strip_run(&c, i);
        if q == c.len() && last_sub {
            end = i;
            break;
        }
    }
    c[start..end.max(start)].iter().collect()
}

// ---------------------------------------------------------------- sizes and durations

const FILESIZE_UNITS: &[(&str, f64)] = &[
    ("B", 1.0), ("b", 1.0), ("bytes", 1.0),
    ("KiB", 1024.0), ("KB", 1000.0), ("kB", 1024.0), ("Kb", 1000.0), ("kb", 1000.0), ("kilobytes", 1000.0), ("kibibytes", 1024.0),
    ("MiB", 1048576.0), ("MB", 1e6), ("mB", 1048576.0), ("Mb", 1e6), ("mb", 1e6), ("megabytes", 1e6), ("mebibytes", 1048576.0),
    ("GiB", 1073741824.0), ("GB", 1e9), ("gB", 1073741824.0), ("Gb", 1e9), ("gb", 1e9), ("gigabytes", 1e9), ("gibibytes", 1073741824.0),
    ("TiB", 1099511627776.0), ("TB", 1e12), ("tB", 1099511627776.0), ("Tb", 1e12), ("tb", 1e12), ("terabytes", 1e12), ("tebibytes", 1099511627776.0),
    ("PiB", 1125899906842624.0), ("PB", 1e15), ("pB", 1125899906842624.0), ("Pb", 1e15), ("pb", 1e15), ("petabytes", 1e15), ("pebibytes", 1125899906842624.0),
    ("EiB", 1152921504606846976.0), ("EB", 1e18), ("eB", 1152921504606846976.0), ("Eb", 1e18), ("eb", 1e18), ("exabytes", 1e18), ("exbibytes", 1152921504606846976.0),
    ("ZiB", 1180591620717411303424.0), ("ZB", 1e21), ("zB", 1180591620717411303424.0), ("Zb", 1e21), ("zb", 1e21), ("zettabytes", 1e21), ("zebibytes", 1180591620717411303424.0),
    ("YiB", 1208925819614629174706176.0), ("YB", 1e24), ("yB", 1208925819614629174706176.0), ("Yb", 1e24), ("yb", 1e24), ("yottabytes", 1e24), ("yobibytes", 1208925819614629174706176.0),
];

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Python's round(): half to even.
pub fn py_round(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 { 2.0 * (x / 2.0).round() } else { r }
}

/// `lookup_unit_table(table, s)` (non-strict: prefix match, `,` or `.` decimal).
fn lookup_unit_table(table: &[(&str, f64)], s: &str, strict: bool) -> Option<f64> {
    let c: Vec<char> = s.chars().collect();
    let mut p = 0;
    while p < c.len() && c[p].is_ascii_digit() {
        p += 1;
    }
    if p == 0 {
        return None;
    }
    let mut numstr: String = c[..p].iter().collect();
    let seps: &[char] = if strict { &['.'] } else { &['.', ','] };
    if c.get(p).is_some_and(|x| seps.contains(x)) && c.get(p + 1).is_some_and(|x| x.is_ascii_digit()) {
        let mut q = p + 1;
        while q < c.len() && c[q].is_ascii_digit() {
            q += 1;
        }
        numstr = format!("{}.{}", numstr, c[p + 1..q].iter().collect::<String>());
        p = q;
    }
    while p < c.len() && c[p].is_whitespace() {
        p += 1;
    }
    let rest: String = c[p..].iter().collect();
    for (unit, mult) in table {
        if let Some(after) = rest.strip_prefix(unit) {
            // `\b` after a unit (all units end in a word character)
            if after.chars().next().is_some_and(is_word_char) {
                continue;
            }
            if strict && !after.is_empty() {
                continue;
            }
            let num: f64 = numstr.parse().ok()?;
            return Some(py_round(num * mult));
        }
    }
    None
}

pub fn parse_filesize(s: &str) -> Option<f64> {
    lookup_unit_table(FILESIZE_UNITS, s, false)
}

pub fn parse_bytes(s: &str) -> Option<f64> {
    let table: Vec<(&str, f64)> = ["", "K", "M", "G", "T", "P", "E", "Z", "Y"]
        .iter()
        .enumerate()
        .map(|(i, u)| (*u, 1024f64.powi(i as i32)))
        .collect();
    // The empty unit comes first; with strict matching it only matches a bare number.
    lookup_unit_table(&table, &s.to_uppercase(), true)
}

/// `determine_ext(url)`.
pub fn determine_ext(url: &str, default: &str) -> String {
    if url.is_empty() || !url.contains('.') {
        return default.into();
    }
    let path = url.split('?').next().unwrap_or("");
    let guess = path.rsplit('.').next().unwrap_or("");
    if !guess.is_empty() && guess.chars().all(|c| c.is_ascii_alphanumeric()) {
        return guess.into();
    }
    let trimmed = guess.trim_end_matches('/');
    if KNOWN_EXTENSIONS.contains(&trimmed) {
        return trimmed.into();
    }
    default.into()
}

pub const COMMON_VIDEO: &[&str] = &["avi", "flv", "mkv", "mov", "mp4", "webm"];
pub const COMMON_AUDIO: &[&str] = &["aiff", "alac", "flac", "m4a", "mka", "mp3", "ogg", "opus", "wav"];
pub const STORYBOARDS: &[&str] = &["mhtml"];
const KNOWN_EXTENSIONS: &[&str] = &[
    "3g2", "3gp", "f4v", "mk3d", "divx", "mpg", "ogv", "m4v", "wmv", "avi", "flv", "mkv", "mov", "mp4", "webm", "aac", "ape", "asf",
    "f4a", "f4b", "m4b", "m4r", "oga", "ogx", "spx", "vorbis", "wma", "weba", "aiff", "alac", "flac", "m4a", "mka", "mp3", "ogg",
    "opus", "wav", "f4f", "f4m", "m3u8", "smil", "mpd",
];

/// `parse_duration(s)` for the forms match filters use.
pub fn parse_duration(s: &str) -> Option<f64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // [[[days:]hours:]mins:]secs[.ms]
    let (main, frac) = match s.split_once('.') {
        Some((a, b)) if b.chars().all(|c| c.is_ascii_digit()) && !b.is_empty() => (a, Some(b)),
        Some(_) => (s, None),
        None => (s, None),
    };
    let parts: Vec<&str> = main.split(':').collect();
    if parts.len() <= 4 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) && (parts.len() == 1 || parts[1..].iter().all(|p| p.len() <= 2)) {
        let mut total = 0.0;
        let mults = [1.0, 60.0, 3600.0, 86400.0];
        for (i, p) in parts.iter().rev().enumerate() {
            total += p.parse::<f64>().ok()? * mults[i];
        }
        if let Some(f) = frac {
            total += format!("0.{f}").parse::<f64>().ok()?;
        }
        return Some(total);
    }
    // 1h2m3s style
    let lower = s.to_lowercase();
    let mut total = 0.0;
    let mut num = String::new();
    let mut any = false;
    for c in lower.chars() {
        if c.is_ascii_digit() || c == '.' {
            num.push(c);
        } else if !num.is_empty() {
            let n: f64 = num.parse().ok()?;
            total += n * match c {
                'd' => 86400.0,
                'h' => 3600.0,
                'm' => 60.0,
                's' => 1.0,
                _ => return None,
            };
            num.clear();
            any = true;
        } else if c != ' ' {
            return None;
        }
    }
    if !num.is_empty() {
        return None;
    }
    any.then_some(total)
}

/// `get_compatible_ext`.
pub fn get_compatible_ext(vcodecs: &[Option<String>], acodecs: &[Option<String>], vexts: &[String], aexts: &[String], preferences: Option<&[String]>) -> String {
    let allow_mkv = preferences.is_none_or(|p| p.iter().any(|x| x == "mkv"));
    if allow_mkv && acodecs.len().max(vcodecs.len()) > 1 {
        return "mkv".into();
    }
    let compat = |ext: &str| -> &'static [&'static str] {
        match ext {
            "mp4" => &["av1", "hevc", "avc1", "mp4a", "ac-4", "h264", "aacl", "ec-3"],
            "webm" => &["av1", "vp9", "vp8", "opus", "vrbs", "vp9x", "vp8x"],
            _ => &[],
        }
    };
    let sanitize = |l: &[Option<String>]| -> Option<String> {
        l.first().and_then(|c| c.as_ref()).map(|c| c.split('.').next().unwrap().replace('0', "").to_lowercase())
    };
    let (vcodec, acodec) = (sanitize(vcodecs), sanitize(acodecs));
    let default_order: Vec<String> = vec!["mp4".into(), "webm".into()];
    let prefs: Vec<String> = preferences.map(|p| p.to_vec()).unwrap_or(default_order);
    for ext in &prefs {
        let set = compat(ext);
        let has = |c: &Option<String>| c.as_deref().is_some_and(|c| set.contains(&c));
        if ext == "mkv" || (has(&vcodec) && has(&acodec)) {
            return ext.clone();
        }
    }
    let compatible_exts: [&[&str]; 2] = [&["mp3", "mp4", "m4a", "m4p", "m4b", "m4r", "m4v", "ismv", "isma", "mov"], &["webm", "weba"]];
    let prefs2: Vec<String> = preferences.map(|p| p.to_vec()).unwrap_or_else(|| vexts.to_vec());
    for ext in &prefs2 {
        let mut current: Vec<&str> = vec![ext.as_str()];
        current.extend(vexts.iter().map(|s| s.as_str()));
        current.extend(aexts.iter().map(|s| s.as_str()));
        if ext == "mkv" || current.iter().all(|e| e == ext) || compatible_exts.iter().any(|set| current.iter().all(|e| set.contains(e))) {
            return ext.clone();
        }
    }
    if allow_mkv { "mkv".into() } else { preferences.unwrap().last().unwrap().clone() }
}

/// `remove_terminal_sequences`: drops `ESC[...m` colour codes.
pub fn remove_terminal_sequences(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '\x1b' && c.get(i + 1) == Some(&'[') {
            let n = c[i + 2..].iter().take_while(|x| x.is_ascii_digit() || **x == ';').count();
            if c.get(i + 2 + n) == Some(&'m') {
                i += n + 3;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

/// `render_table`: text after a `\t` is right aligned.
pub fn render_table(header_row: &[String], data: &[Vec<String>], delim: Option<&str>, extra_gap: usize, hide_empty: bool) -> String {
    let width = |s: &str| remove_terminal_sequences(s).replace('\t', "").chars().count();
    let max_lens = |table: &[Vec<String>]| -> Vec<usize> {
        let cols = table.iter().map(|r| r.len()).min().unwrap_or(0);
        (0..cols).map(|i| table.iter().map(|r| width(&r[i])).max().unwrap_or(0)).collect()
    };
    // zip_longest(filter_array, row, fillvalue=True)
    let filter = |row: &[String], keep: &[usize]| -> Vec<String> {
        (0..row.len()).filter(|&i| keep.get(i).is_none_or(|&n| n > 0)).map(|i| row[i].clone()).collect()
    };
    let keep = if hide_empty { max_lens(data) } else { vec![] };
    let header = filter(header_row, &keep);
    let data: Vec<Vec<String>> = data.iter().map(|r| filter(r, &keep)).collect();
    let mut table: Vec<Vec<String>> = std::iter::once(header).chain(data).collect();
    let lens = max_lens(&table);
    let gap = extra_gap + 1;
    if let Some(d) = delim {
        let mut line: Vec<String> = lens.iter().map(|ml| d.repeat(ml + gap)).collect();
        if let Some(last) = line.last_mut() {
            let n = last.chars().count().saturating_sub(gap * d.chars().count());
            *last = last.chars().take(n).collect();
        }
        table.insert(1, line);
    }
    let cell = |pos: usize, text: &str| {
        let ml = lens.get(pos).copied().unwrap_or(0);
        if text.contains('\t') {
            text.replace('\t', &" ".repeat(ml.saturating_sub(width(text)))) + &" ".repeat(gap)
        } else {
            // ' ' * (ml - width + extra_gap): the delimiter row is already gap characters wider
            text.to_string() + &" ".repeat((ml + gap).saturating_sub(width(text)))
        }
    };
    table
        .iter()
        .map(|row| row.iter().enumerate().map(|(pos, text)| cell(pos, text)).collect::<String>().trim_end().to_string())
        .collect::<Vec<_>>()
        .join("\n")
}
