// JSON values with Python semantics: insertion-ordered objects, int/float kept apart, and
// serialization that matches Python's json.dumps byte for byte.

use crate::pyfmt::float_repr;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    List(Vec<Value>),
    Dict(Map),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Map(pub Vec<(String, Value)>);

impl Map {
    pub fn new() -> Self {
        Map(Vec::new())
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.0.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }
    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|(k, _)| k == key)
    }
    /// Python `d[key] = value`: replaces in place, or appends a new key.
    pub fn set(&mut self, key: &str, value: Value) {
        match self.get_mut(key) {
            Some(v) => *v = value,
            None => self.0.push((key.to_string(), value)),
        }
    }
    pub fn setdefault(&mut self, key: &str, value: Value) {
        if !self.contains(key) {
            self.0.push((key.to_string(), value));
        }
    }
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let i = self.0.iter().position(|(k, _)| k == key)?;
        Some(self.0.remove(i).1)
    }
    /// Python `d.update(other)`.
    pub fn update(&mut self, other: &Map) {
        for (k, v) in &other.0 {
            self.set(k, v.clone());
        }
    }
    /// Python `d.get(key)`, with a missing key and an explicit null both giving Null.
    pub fn g(&self, key: &str) -> &Value {
        self.get(key).unwrap_or(&Value::Null)
    }
    pub fn str_of(&self, key: &str) -> Option<&str> {
        self.g(key).as_str()
    }
}

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_dict(&self) -> Option<&Map> {
        match self {
            Value::Dict(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_dict_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Dict(m) => Some(m),
            _ => None,
        }
    }
    pub fn as_list(&self) -> Option<&Vec<Value>> {
        match self {
            Value::List(l) => Some(l),
            _ => None,
        }
    }
    /// The number as f64 if this is an int or float (bools count, as in Python).
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            Value::Bool(b) => Some(*b as i64 as f64),
            _ => None,
        }
    }
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Int(_) | Value::Float(_) | Value::Bool(_))
    }
    /// Python truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Int(i) => *i != 0,
            Value::Float(f) => *f != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Dict(m) => !m.0.is_empty(),
        }
    }
    pub fn str(s: &str) -> Value {
        Value::Str(s.to_string())
    }
    /// A Python number result: ints stay ints when they are whole and came from ints.
    pub fn num(f: f64) -> Value {
        Value::Float(f)
    }
}

// ---------------------------------------------------------------- parsing

pub fn parse(text: &str) -> Result<Value, String> {
    let mut p = Parser { s: text.as_bytes(), i: 0, text };
    p.ws();
    let v = p.value()?;
    p.ws();
    if p.i != p.s.len() {
        return Err(p.err("Extra data"));
    }
    Ok(v)
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    text: &'a str,
}

impl Parser<'_> {
    fn err(&self, msg: &str) -> String {
        let line = self.text[..self.i].matches('\n').count() + 1;
        let col = self.i - self.text[..self.i].rfind('\n').map(|p| p + 1).unwrap_or(0) + 1;
        format!("{msg}: line {line} column {col} (char {})", self.text[..self.i].chars().count())
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }
    fn value(&mut self) -> Result<Value, String> {
        match self.s.get(self.i) {
            Some(b'{') => {
                self.i += 1;
                let mut m = Map::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Value::Dict(m));
                }
                loop {
                    self.ws();
                    if self.s.get(self.i) != Some(&b'"') {
                        return Err(self.err("Expecting property name enclosed in double quotes"));
                    }
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(self.err("Expecting ':' delimiter"));
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value()?;
                    m.set(&k, v);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Value::Dict(m));
                        }
                        _ => return Err(self.err("Expecting ',' delimiter")),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut l = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Value::List(l));
                }
                loop {
                    self.ws();
                    l.push(self.value()?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Value::List(l));
                        }
                        _ => return Err(self.err("Expecting ',' delimiter")),
                    }
                }
            }
            Some(b'"') => Ok(Value::Str(self.string()?)),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Value::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Value::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Value::Null)
            }
            Some(b'N') if self.s[self.i..].starts_with(b"NaN") => {
                self.i += 3;
                Ok(Value::Float(f64::NAN))
            }
            Some(b'I') if self.s[self.i..].starts_with(b"Infinity") => {
                self.i += 8;
                Ok(Value::Float(f64::INFINITY))
            }
            Some(b'-') if self.s[self.i..].starts_with(b"-Infinity") => {
                self.i += 9;
                Ok(Value::Float(f64::NEG_INFINITY))
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => self.number(),
            _ => Err(self.err("Expecting value")),
        }
    }
    fn number(&mut self) -> Result<Value, String> {
        let start = self.i;
        if self.s[self.i] == b'-' {
            self.i += 1;
        }
        let digits = |p: &mut Self| {
            let st = p.i;
            while p.i < p.s.len() && p.s[p.i].is_ascii_digit() {
                p.i += 1;
            }
            p.i - st
        };
        if digits(self) == 0 {
            self.i = start;
            return Err(self.err("Expecting value"));
        }
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') && self.s.get(self.i + 1).is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
            digits(self);
            float = true;
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if digits(self) == 0 {
                self.i = save;
            } else {
                float = true;
            }
        }
        let t = &self.text[start..self.i];
        if float {
            Ok(Value::Float(t.parse().unwrap()))
        } else {
            match t.parse::<i64>() {
                Ok(i) => Ok(Value::Int(i)),
                Err(_) => Ok(Value::Float(t.parse().unwrap())),
            }
        }
    }
    fn string(&mut self) -> Result<String, String> {
        self.i += 1;
        let mut out = String::new();
        loop {
            let start = self.i;
            while self.i < self.s.len() && self.s[self.i] != b'"' && self.s[self.i] != b'\\' && self.s[self.i] >= 0x20 {
                self.i += 1;
            }
            out.push_str(&self.text[start..self.i]);
            match self.s.get(self.i) {
                Some(b'"') => {
                    self.i += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.i += 1;
                    let c = *self.s.get(self.i).ok_or_else(|| self.err("Unterminated string starting at"))?;
                    self.i += 1;
                    match c {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    cp = 0x10000 + ((cp - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            // Lone surrogates cannot live in a Rust string; use the replacement character.
                            out.push(char::from_u32(cp).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err(self.err("Invalid \\escape")),
                    }
                }
                Some(_) => return Err(self.err("Invalid control character at")),
                None => return Err(self.err("Unterminated string starting at")),
            }
        }
    }
    fn hex4(&mut self) -> Result<u32, String> {
        let h = self.text.get(self.i..self.i + 4).ok_or_else(|| self.err("Invalid \\uXXXX escape"))?;
        let v = u32::from_str_radix(h, 16).map_err(|_| self.err("Invalid \\uXXXX escape"))?;
        self.i += 4;
        Ok(v)
    }
}

// ---------------------------------------------------------------- dumping

pub struct DumpOpts {
    pub indent: Option<usize>,
    pub ensure_ascii: bool,
}

impl Default for DumpOpts {
    fn default() -> Self {
        DumpOpts { indent: None, ensure_ascii: true }
    }
}

/// Python's `json.dumps(value)` with default separators.
pub fn dumps(v: &Value) -> String {
    dumps_with(v, &DumpOpts::default())
}

pub fn dumps_with(v: &Value, o: &DumpOpts) -> String {
    let mut out = String::new();
    dump(v, o, 0, &mut out);
    out
}

fn dump(v: &Value, o: &DumpOpts, level: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Int(i) => out.push_str(&i.to_string()),
        Value::Float(f) => out.push_str(&json_float(*f)),
        Value::Str(s) => dump_str(s, o.ensure_ascii, out),
        Value::List(l) => {
            if l.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (n, item) in l.iter().enumerate() {
                sep(n, o, level + 1, out);
                dump(item, o, level + 1, out);
            }
            close(o, level, out);
            out.push(']');
        }
        Value::Dict(m) => {
            if m.0.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (n, (k, item)) in m.0.iter().enumerate() {
                sep(n, o, level + 1, out);
                dump_str(k, o.ensure_ascii, out);
                out.push_str(": ");
                dump(item, o, level + 1, out);
            }
            close(o, level, out);
            out.push('}');
        }
    }
}

fn sep(n: usize, o: &DumpOpts, level: usize, out: &mut String) {
    match o.indent {
        Some(ind) => {
            if n > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(ind * level));
        }
        None => {
            if n > 0 {
                out.push_str(", ");
            }
        }
    }
}

fn close(o: &DumpOpts, level: usize, out: &mut String) {
    if let Some(ind) = o.indent {
        out.push('\n');
        out.push_str(&" ".repeat(ind * level));
    }
}

pub fn json_float(f: f64) -> String {
    if f.is_nan() {
        "NaN".into()
    } else if f.is_infinite() {
        if f > 0.0 { "Infinity".into() } else { "-Infinity".into() }
    } else {
        float_repr(f)
    }
}

pub fn dump_str(s: &str, ensure_ascii: bool, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c if ensure_ascii && (c as u32) > 0x7e => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
}
