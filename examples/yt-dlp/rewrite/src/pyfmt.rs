// Python's str()/repr() and the printf-style `%` operator, for the values yt-dlp formats.

use crate::json::Value;

/// `repr(float)`: shortest round-trip digits, exponent form outside 1e-4 <= |f| < 1e16.
pub fn float_repr(f: f64) -> String {
    if f.is_nan() {
        return "nan".into();
    }
    if f.is_infinite() {
        return if f > 0.0 { "inf".into() } else { "-inf".into() };
    }
    if f == 0.0 {
        return if f.is_sign_negative() { "-0.0".into() } else { "0.0".into() };
    }
    let e = format!("{:e}", f); // like "-1.2345e3"
    let (mant, exp) = e.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    let neg = mant.starts_with('-');
    let digits: String = mant.chars().filter(|c| c.is_ascii_digit()).collect();
    let sign = if neg { "-" } else { "" };
    if !(-4..16).contains(&exp) {
        let rest = &digits[1..];
        let m = if rest.is_empty() { digits[..1].to_string() } else { format!("{}.{}", &digits[..1], rest) };
        return format!("{sign}{m}e{}{:02}", if exp < 0 { '-' } else { '+' }, exp.abs());
    }
    let n = digits.len() as i32;
    let body = if exp < 0 {
        format!("0.{}{}", "0".repeat((-exp - 1) as usize), digits)
    } else if exp + 1 >= n {
        format!("{}{}.0", digits, "0".repeat((exp + 1 - n) as usize))
    } else {
        let p = (exp + 1) as usize;
        format!("{}.{}", &digits[..p], &digits[p..])
    };
    format!("{sign}{body}")
}

fn is_printable(c: char) -> bool {
    let u = c as u32;
    !(u < 0x20 || (0x7f..=0xa0).contains(&u) || u == 0xad || (0x2028..=0x2029).contains(&u)
        || (0xd800..=0xdfff).contains(&u) || u == 0xfeff || (0xe000..=0xf8ff).contains(&u)
        || (0x200b..=0x200f).contains(&u) || (0x202a..=0x202e).contains(&u) || (0x2060..=0x2064).contains(&u))
}

/// `repr(str)`.
pub fn str_repr(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if !is_printable(c) => {
                let u = c as u32;
                if u < 0x100 {
                    out.push_str(&format!("\\x{u:02x}"));
                } else if u < 0x10000 {
                    out.push_str(&format!("\\u{u:04x}"));
                } else {
                    out.push_str(&format!("\\U{u:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// `repr(value)`.
pub fn repr(v: &Value) -> String {
    match v {
        Value::Str(s) => str_repr(s),
        Value::List(l) => format!("[{}]", l.iter().map(repr).collect::<Vec<_>>().join(", ")),
        Value::Dict(m) => format!(
            "{{{}}}",
            m.0.iter().map(|(k, v)| format!("{}: {}", str_repr(k), repr(v))).collect::<Vec<_>>().join(", ")
        ),
        _ => py_str(v),
    }
}

/// `str(value)`.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(b) => if *b { "True" } else { "False" }.into(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => float_repr(*f),
        Value::Str(s) => s.clone(),
        _ => repr(v),
    }
}

/// `ascii(value)`: repr with every non-ASCII character escaped.
pub fn ascii(v: &Value) -> String {
    let mut out = String::new();
    for c in repr(v).chars() {
        let u = c as u32;
        if u < 0x80 {
            out.push(c);
        } else if u < 0x100 {
            out.push_str(&format!("\\x{u:02x}"));
        } else if u < 0x10000 {
            out.push_str(&format!("\\u{u:04x}"));
        } else {
            out.push_str(&format!("\\U{u:08x}"));
        }
    }
    out
}

/// One parsed conversion spec: `%[flags][width][.precision]type`.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub flags: String,
    pub width: Option<usize>,
    pub precision: Option<usize>,
    pub conv: char,
}

impl Spec {
    /// Parses a spec such as `08d` or `.10s` (without the leading `%` and key).
    pub fn parse(s: &str) -> Option<Spec> {
        let b: Vec<char> = s.chars().collect();
        let mut i = 0;
        let mut spec = Spec::default();
        while i < b.len() && "#0- +".contains(b[i]) {
            spec.flags.push(b[i]);
            i += 1;
        }
        let st = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        if i > st {
            spec.width = Some(b[st..i].iter().collect::<String>().parse().ok()?);
        }
        if i < b.len() && b[i] == '.' {
            i += 1;
            let st = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            spec.precision = Some(b[st..i].iter().collect::<String>().parse().unwrap_or(0));
        }
        while i + 1 < b.len() && "hlL".contains(b[i]) {
            i += 1;
        }
        if i + 1 != b.len() {
            return None;
        }
        spec.conv = b[i];
        Some(spec)
    }
}

fn pad(body: String, spec: &Spec, numeric: bool) -> String {
    let w = spec.width.unwrap_or(0);
    let len = body.chars().count();
    if len >= w {
        return body;
    }
    let fill = w - len;
    if spec.flags.contains('-') {
        format!("{body}{}", " ".repeat(fill))
    } else if numeric && spec.flags.contains('0') {
        let (sign, digits) = match body.chars().next() {
            Some(c @ ('-' | '+' | ' ')) => (c.to_string(), body[1..].to_string()),
            _ => (String::new(), body.clone()),
        };
        // Keep a 0x/0o prefix in front of the zeros.
        let (prefix, digits) = if digits.len() >= 2 && matches!(&digits[..2], "0x" | "0X" | "0o") && spec.flags.contains('#') {
            (digits[..2].to_string(), digits[2..].to_string())
        } else {
            (String::new(), digits)
        };
        format!("{sign}{prefix}{}{digits}", "0".repeat(fill))
    } else {
        format!("{}{body}", " ".repeat(fill))
    }
}

fn sign_prefix(neg: bool, flags: &str) -> &'static str {
    if neg {
        "-"
    } else if flags.contains('+') {
        "+"
    } else if flags.contains(' ') {
        " "
    } else {
        ""
    }
}

/// Python `%e` exponent style: at least two exponent digits with a sign.
fn fix_exp(s: &str, upper: bool) -> String {
    let (m, e) = s.split_once('e').unwrap();
    let e: i32 = e.parse().unwrap();
    format!("{m}{}{}{:02}", if upper { 'E' } else { 'e' }, if e < 0 { '-' } else { '+' }, e.abs())
}

pub fn fmt_float(f: f64, conv: char, precision: Option<usize>, flags: &str) -> String {
    let upper = conv.is_ascii_uppercase();
    if f.is_nan() || f.is_infinite() {
        let s = if f.is_nan() { "nan" } else { "inf" };
        let s = if upper { s.to_uppercase() } else { s.to_string() };
        return format!("{}{}", sign_prefix(f.is_sign_negative() && !f.is_nan(), flags), s);
    }
    let p = precision.unwrap_or(6);
    let a = f.abs();
    let body = match conv.to_ascii_lowercase() {
        'f' => {
            let s = format!("{:.*}", p, a);
            if flags.contains('#') && p == 0 { format!("{s}.") } else { s }
        }
        'e' => fix_exp(&format!("{:.*e}", p, a), upper),
        _ => {
            // %g
            let p = if p == 0 { 1 } else { p };
            if a == 0.0 {
                if flags.contains('#') { format!("0.{}", "0".repeat(p - 1)) } else { "0".to_string() }
            } else {
                let e = format!("{:.*e}", p - 1, a);
                let exp: i32 = e.split_once('e').unwrap().1.parse().unwrap();
                let mut s = if exp < -4 || exp >= p as i32 {
                    let (m, _) = e.split_once('e').unwrap();
                    let m = if flags.contains('#') { m.to_string() } else { strip_zeros(m) };
                    fix_exp(&format!("{m}e{exp}"), upper)
                } else {
                    let s = format!("{:.*}", (p as i32 - 1 - exp).max(0) as usize, a);
                    if flags.contains('#') { s } else { strip_zeros(&s) }
                };
                if upper {
                    s = s.to_uppercase();
                }
                s
            }
        }
    };
    format!("{}{}", sign_prefix(f.is_sign_negative(), flags), body)
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// Integer value for %d and friends: Python's int() of a number.
fn to_int(v: &Value) -> Result<i128, String> {
    match v {
        Value::Int(i) => Ok(*i as i128),
        Value::Bool(b) => Ok(*b as i128),
        Value::Float(f) if f.is_finite() => Ok(f.trunc() as i128),
        Value::Float(f) if f.is_nan() => Err("cannot convert float NaN to integer".into()),
        Value::Float(_) => Err("cannot convert float infinity to integer".into()),
        _ => Err(format!("%d format: a real number is required, not {}", type_name(v))),
    }
}

pub fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "NoneType",
        Value::Bool(_) => "bool",
        Value::Int(_) => "int",
        Value::Float(_) => "float",
        Value::Str(_) => "str",
        Value::List(_) => "list",
        Value::Dict(_) => "dict",
    }
}

/// `('%' + spec) % value`.
pub fn format_value(spec: &Spec, v: &Value) -> Result<String, String> {
    let flags = spec.flags.as_str();
    match spec.conv {
        's' | 'r' | 'a' => {
            let mut s = match spec.conv {
                's' => py_str(v),
                'r' => repr(v),
                _ => ascii(v),
            };
            if let Some(p) = spec.precision {
                s = s.chars().take(p).collect();
            }
            Ok(pad(s, spec, false))
        }
        'd' | 'i' | 'u' => {
            let n = to_int(v)?;
            let mut body = n.abs().to_string();
            if let Some(p) = spec.precision {
                if body.len() < p {
                    body = format!("{}{body}", "0".repeat(p - body.len()));
                }
            }
            Ok(pad(format!("{}{body}", sign_prefix(n < 0, flags)), spec, true))
        }
        'x' | 'X' | 'o' => {
            let n = match v {
                Value::Int(i) => *i as i128,
                Value::Bool(b) => *b as i128,
                _ => return Err(format!("%{} format: an integer is required, not {}", spec.conv, type_name(v))),
            };
            let a = n.unsigned_abs();
            let (mut body, prefix) = match spec.conv {
                'x' => (format!("{a:x}"), "0x"),
                'X' => (format!("{a:X}"), "0X"),
                _ => (format!("{a:o}"), "0o"),
            };
            if let Some(p) = spec.precision {
                if body.len() < p {
                    body = format!("{}{body}", "0".repeat(p - body.len()));
                }
            }
            let prefix = if flags.contains('#') { prefix } else { "" };
            Ok(pad(format!("{}{prefix}{body}", sign_prefix(n < 0, flags)), spec, true))
        }
        'e' | 'E' | 'f' | 'F' | 'g' | 'G' => {
            let f = match v {
                Value::Int(_) | Value::Float(_) | Value::Bool(_) => v.as_f64().unwrap(),
                _ => return Err(format!("must be real number, not {}", type_name(v))),
            };
            Ok(pad(fmt_float(f, spec.conv, spec.precision, flags), spec, true))
        }
        'c' => {
            let s = match v {
                Value::Int(i) => char::from_u32(*i as u32).map(String::from).ok_or("%c arg not in range(0x110000)")?,
                Value::Str(s) if s.chars().count() == 1 => s.clone(),
                _ => return Err("%c requires an int or a unicode character, not a string of length 0".into()),
            };
            Ok(pad(s, spec, false))
        }
        c => Err(format!("unsupported format character '{c}' (0x{:x})", c as u32)),
    }
}
