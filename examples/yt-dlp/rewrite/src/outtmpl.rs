// Output templates: YoutubeDL.prepare_outtmpl + evaluate_outtmpl.
//
// Python builds an intermediate `%`-template and a dict, then applies the `%` operator. Here each
// `%(key)fmt` is evaluated and formatted in one step, and the escaping of the remaining text
// (`%%` -> `%`, a lone `%` stays) is applied directly.

use crate::json::{DumpOpts, Map, Value, dumps_with};
use crate::pyfmt::{self, Spec, py_str};
use crate::util;

const STR_FORMAT_TYPES: &str = "diouxXeEfFgGcrsa";
const EXTRA_TYPES: &str = "ljhqBUDS";

pub struct Ctx<'a> {
    pub na: &'a str,
    /// None: no sanitization. Some(restricted, windows_false): sanitize as a filename.
    pub sanitize: Option<Sanitize>,
    pub autonumber_size: usize,
}

#[derive(Clone, Copy)]
pub struct Sanitize {
    pub restricted: bool,
    /// `--no-windows-filenames` given explicitly: only replace `/`.
    pub slash_only: bool,
}

/// A parsed `%(...)fmt` occurrence in the template.
struct Ext {
    start: usize,
    end: usize,
    prefix_pairs: usize,
    key: Option<String>,
    format: String,
}

/// Finds the next match of STR_FORMAT_RE_TMPL (with key `[^)]*`) at or after `from`.
fn find_ext(chars: &[char], from: usize, types: &str) -> Option<Ext> {
    let mut i = from;
    while i < chars.len() {
        if chars[i] != '%' || (i > 0 && chars[i - 1] == '%') {
            i += 1;
            continue;
        }
        let mut r = 0;
        while i + r < chars.len() && chars[i + r] == '%' {
            r += 1;
        }
        if r % 2 == 1 {
            let p = i + r; // just past the single `%`
            if let Some((key, format, end)) = parse_after_percent(chars, p, types) {
                return Some(Ext { start: i, end, prefix_pairs: r / 2, key, format });
            }
        }
        i += r;
    }
    None
}

/// Parses `(key)?format` starting right after a `%`.
fn parse_after_percent(chars: &[char], mut p: usize, types: &str) -> Option<(Option<String>, String, usize)> {
    let mut key = None;
    if chars.get(p) == Some(&'(') {
        let close = chars[p + 1..].iter().position(|&c| c == ')').map(|x| x + p + 1);
        if let Some(close) = close {
            key = Some(chars[p + 1..close].iter().collect());
            p = close + 1;
        }
    }
    let st = p;
    while p < chars.len() && "#0-+ ".contains(chars[p]) {
        p += 1;
    }
    while p < chars.len() && chars[p].is_ascii_digit() {
        p += 1;
    }
    if chars.get(p) == Some(&'.') && chars.get(p + 1).is_some_and(|c| c.is_ascii_digit()) {
        p += 1;
        while p < chars.len() && chars[p].is_ascii_digit() {
            p += 1;
        }
    }
    let before_len = p;
    if p < chars.len() && "hlL".contains(chars[p]) {
        p += 1;
    }
    let ty = |q: usize| chars.get(q).is_some_and(|c| types.contains(*c));
    let end = if ty(p) {
        p + 1
    } else if p != before_len && ty(before_len) {
        before_len + 1
    } else {
        return None;
    };
    Some((key, chars[st..end].iter().collect(), end))
}

/// Unescapes plain template text the way escape_outtmpl + `%` do.
fn literal(text: &[char], out: &mut String) {
    let mut i = 0;
    while i < text.len() {
        if text[i] == '%' {
            let mut r = 0;
            while i + r < text.len() && text[i + r] == '%' {
                r += 1;
            }
            out.push_str(&"%".repeat(r.div_ceil(2)));
            i += r;
        } else {
            out.push(text[i]);
            i += 1;
        }
    }
}

pub fn evaluate(tmpl: &str, info: &Map, ctx: &Ctx) -> Result<String, String> {
    let info = prepare_info(info, ctx);
    let chars: Vec<char> = tmpl.chars().collect();
    let types = format!("{STR_FORMAT_TYPES}{EXTRA_TYPES}");
    let mut out = String::new();
    let mut pos = 0;
    while let Some(m) = find_ext(&chars, pos, &types) {
        literal(&chars[pos..m.start], &mut out);
        out.push_str(&"%".repeat(m.prefix_pairs));
        match &m.key {
            Some(key) => out.push_str(&create_key(key, &m.format, &info, ctx)?),
            // Python leaves this for escape_outtmpl, which makes it literal.
            None => {
                out.push('%');
                out.extend(m.format.chars());
            }
        }
        pos = m.end;
    }
    literal(&chars[pos..], &mut out);
    Ok(out)
}

fn prepare_info(info: &Map, ctx: &Ctx) -> Map {
    let mut d = info.clone();
    d.remove("__postprocessors");
    d.remove("__pending_error");
    let ds = match d.g("duration") {
        Value::Null => Value::Null,
        v => match v.as_f64() {
            Some(secs) => Value::Str(util::format_seconds(secs, if ctx.sanitize.is_some() { "-" } else { ":" })),
            None => Value::Null,
        },
    };
    d.set("duration_string", ds);
    let an = d.g("autonumber").clone();
    d.set("autonumber", if an.is_null() { Value::Int(0) } else { an });
    if !d.contains("video_autonumber") {
        d.set("video_autonumber", Value::Int(0));
    }
    if d.g("resolution").is_null() {
        let r = util::format_resolution(&d, None);
        d.set("resolution", r.map(Value::Str).unwrap_or(Value::Null));
    }
    d
}

// ---------------------------------------------------------------- INTERNAL_FORMAT_RE

struct Internal {
    negate: bool,
    fields: String,
    maths: String,
    strf: Option<String>,
    alternate: Option<String>,
    replacement: Option<String>,
    default: Option<String>,
    remaining: String,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `-?\d+` at p; returns end.
fn match_num(c: &[char], p: usize) -> Option<usize> {
    let mut q = p;
    if c.get(q) == Some(&'-') {
        q += 1;
    }
    let st = q;
    while q < c.len() && c[q].is_ascii_digit() {
        q += 1;
    }
    (q > st).then_some(q)
}

/// FIELD_INNER_RE: `\w+ | -?\d+ | (-?\d+)?(:(-?\d+)?){1,2}`
fn match_inner(c: &[char], p: usize) -> Option<usize> {
    let mut q = p;
    while q < c.len() && is_word(c[q]) {
        q += 1;
    }
    let word = (q > p).then_some(q);
    let num = match_num(c, p);
    // slice form
    let mut s = match_num(c, p).unwrap_or(p);
    let mut colons = 0;
    while colons < 2 && c.get(s) == Some(&':') {
        s += 1;
        colons += 1;
        if let Some(e) = match_num(c, s) {
            s = e;
        }
    }
    let slice = (colons > 0).then_some(s);
    // Python's alternation takes the first alternative that lets the rest match; the longest
    // candidate is what makes the rest match in practice.
    [word, num, slice].into_iter().flatten().max()
}

/// `\w*(?:\.INNER)*` (the field form allowed inside `{...}`).
fn match_subfield(c: &[char], p: usize) -> usize {
    let mut q = p;
    while q < c.len() && is_word(c[q]) {
        q += 1;
    }
    while c.get(q) == Some(&'.') {
        match match_inner(c, q + 1) {
            Some(e) => q = e,
            None => break,
        }
    }
    q
}

/// FIELD_RE.
fn match_field(c: &[char], p: usize) -> usize {
    let mut q = p;
    while q < c.len() && is_word(c[q]) {
        q += 1;
    }
    loop {
        if c.get(q) != Some(&'.') {
            break;
        }
        if c.get(q + 1) == Some(&'{') {
            let mut r = match_subfield(c, q + 2);
            while c.get(r) == Some(&',') {
                r = match_subfield(c, r + 1);
            }
            if c.get(r) == Some(&'}') {
                q = r + 1;
                continue;
            }
            break;
        }
        match match_inner(c, q + 1) {
            Some(e) => q = e,
            None => break,
        }
    }
    q
}

/// `-?\d+(?:\.\d+)?`
fn match_number(c: &[char], p: usize) -> Option<usize> {
    let mut q = match_num(c, p)?;
    if c.get(q) == Some(&'.') && c.get(q + 1).is_some_and(|x| x.is_ascii_digit()) {
        q += 1;
        while q < c.len() && c[q].is_ascii_digit() {
            q += 1;
        }
    }
    Some(q)
}

/// MATH_FIELD_RE, as re.match picks it: the field alternative first, even when empty.
fn match_math_field(c: &[char], p: usize) -> usize {
    let f = match_field(c, p);
    if f > p {
        return f;
    }
    match_number(c, p).unwrap_or(p)
}

/// `remaining` group: `(,[^|&)]+)?(&(.*?))?(\|(.*?))?$`
fn match_remaining(c: &[char], p: usize) -> Option<(Option<String>, Option<String>, Option<String>)> {
    let mut q = p;
    let mut alternate = None;
    if c.get(q) == Some(&',') && !(q > 0 && c[q - 1] == '\\') {
        let mut r = q + 1;
        while r < c.len() && !"|&)".contains(c[r]) {
            r += 1;
        }
        if r > q + 1 {
            alternate = Some(c[q..r].iter().collect());
            q = r;
        }
    }
    let mut replacement = None;
    if c.get(q) == Some(&'&') {
        let rest = &c[q + 1..];
        let bar = rest.iter().position(|&x| x == '|');
        let end = bar.map(|b| q + 1 + b).unwrap_or(c.len());
        replacement = Some(c[q + 1..end].iter().collect());
        q = end;
    }
    let mut default = None;
    if c.get(q) == Some(&'|') {
        default = Some(c[q + 1..].iter().collect());
        q = c.len();
    }
    (q == c.len()).then_some((alternate, replacement, default))
}

fn match_internal(key: &str) -> Option<Internal> {
    let c: Vec<char> = key.chars().collect();
    let mut p = 0;
    let negate = c.first() == Some(&'-');
    if negate {
        p = 1;
    }
    let fe = match_field(&c, p);
    let fields: String = c[p..fe].iter().collect();
    p = fe;
    let ms = p;
    while p < c.len() && "+-*".contains(c[p]) {
        let e = match_math_field(&c, p + 1);
        p = e;
    }
    let maths: String = c[ms..p].iter().collect();
    // Optional `>strf_format` (lazy), then the remaining part to the end.
    if c.get(p) == Some(&'>') {
        for e in p + 2..=c.len() {
            if let Some((alternate, replacement, default)) = match_remaining(&c, e) {
                return Some(Internal {
                    negate,
                    fields,
                    maths,
                    strf: Some(c[p + 1..e].iter().collect()),
                    alternate,
                    replacement,
                    default,
                    remaining: c[e..].iter().collect(),
                });
            }
        }
    }
    let (alternate, replacement, default) = match_remaining(&c, p)?;
    Some(Internal { negate, fields, maths, strf: None, alternate, replacement, default, remaining: c[p..].iter().collect() })
}

// ---------------------------------------------------------------- traversal

#[derive(Clone, Debug)]
enum Key {
    Str(String),
    Int(i64),
    Slice(Option<i64>, Option<i64>, Option<i64>),
    All,
    Dict(Vec<(String, Vec<Key>)>),
}

fn int_or_none(s: &str) -> Option<i64> {
    let t = s.trim();
    let t2 = t.strip_prefix(['+', '-']).unwrap_or(t);
    if t2.is_empty() || !t2.chars().all(|c| c.is_ascii_digit() || c == '_') {
        return None;
    }
    t.replace('_', "").parse().ok()
}

fn from_user_input(f: &str) -> Key {
    if f == ":" {
        Key::All
    } else if f.contains(':') {
        let parts: Vec<Option<i64>> = f.split(':').map(int_or_none).collect();
        Key::Slice(parts[0], parts.get(1).copied().flatten(), parts.get(2).copied().flatten())
    } else if let Some(i) = int_or_none(f) {
        Key::Int(i)
    } else {
        Key::Str(f.to_string())
    }
}

/// `re.split(r'\.({.+?})\.?', fields)`, keeping the `{...}` groups.
fn split_fields(fields: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = fields;
    loop {
        let found = rest.find(".{").and_then(|i| rest[i + 2..].find('}').filter(|&j| j > 0).map(|j| (i, i + 2 + j)));
        match found {
            Some((i, close)) => {
                parts.push(rest[..i].to_string());
                parts.push(rest[i + 1..=close].to_string());
                rest = &rest[close + 1..];
                if let Some(r) = rest.strip_prefix('.') {
                    rest = r;
                }
            }
            None => {
                parts.push(rest.to_string());
                break;
            }
        }
    }
    parts
}

fn parse_path(fields: &str) -> Vec<Key> {
    let mut fs: Vec<String> = Vec::new();
    for x in split_fields(fields) {
        if x.starts_with('{') {
            fs.push(x);
        } else {
            fs.extend(x.split('.').map(String::from));
        }
    }
    if fs.first().is_some_and(|f| f.is_empty()) {
        fs.remove(0);
    }
    if fs.last().is_some_and(|f| f.is_empty()) {
        fs.pop();
    }
    fs.iter()
        .map(|f| {
            if let Some(inner) = f.strip_prefix('{') {
                let inner = inner.strip_suffix('}').unwrap_or(inner);
                Key::Dict(inner.split(',').map(|k| (k.to_string(), k.split('.').map(from_user_input).collect())).collect())
            } else {
                from_user_input(f)
            }
        })
        .collect()
}

fn py_index(len: usize, i: i64) -> Option<usize> {
    let j = if i < 0 { i + len as i64 } else { i };
    (0..len as i64).contains(&j).then_some(j as usize)
}

fn py_slice<T: Clone>(items: &[T], start: Option<i64>, stop: Option<i64>, step: Option<i64>) -> Vec<T> {
    let len = items.len() as i64;
    let step = step.unwrap_or(1);
    if step == 0 {
        return Vec::new();
    }
    let adj = |v: i64, lo: i64, hi: i64| {
        let v = if v < 0 { v + len } else { v };
        v.clamp(lo, hi)
    };
    let mut out = Vec::new();
    if step > 0 {
        let s = start.map(|v| adj(v, 0, len)).unwrap_or(0);
        let e = stop.map(|v| adj(v, 0, len)).unwrap_or(len);
        let mut i = s;
        while i < e {
            out.push(items[i as usize].clone());
            i += step;
        }
    } else {
        let s = start.map(|v| adj(v, -1, len - 1)).unwrap_or(len - 1);
        let e = stop.map(|v| adj(v, -1, len - 1)).unwrap_or(-1);
        let mut i = s;
        while i > e {
            out.push(items[i as usize].clone());
            i += step;
        }
    }
    out
}

fn empty_or_none(v: &Value) -> bool {
    matches!(v, Value::Null) || matches!(v, Value::Dict(m) if m.0.is_empty())
}

/// apply_path: returns the result objects and whether any key branched.
fn apply_path(obj: &Value, path: &[Key]) -> (Vec<Value>, bool, bool) {
    let mut objs = vec![obj.clone()];
    let mut branched = false;
    for key in path {
        let mut next = Vec::new();
        for o in &objs {
            let (b, res) = apply_key(key, o);
            branched |= b;
            next.extend(res);
        }
        objs = next;
    }
    (objs, branched, matches!(path.last(), Some(Key::Dict(_))))
}

fn apply_key(key: &Key, obj: &Value) -> (bool, Vec<Value>) {
    if obj.is_null() {
        return match key {
            Key::All | Key::Slice(..) => (true, vec![]),
            _ => (false, vec![Value::Null]),
        };
    }
    match key {
        Key::All => match obj {
            Value::Dict(m) => (true, m.0.iter().map(|(_, v)| v.clone()).collect()),
            Value::List(l) => (true, l.clone()),
            _ => (false, vec![Value::Str(py_str(obj))]),
        },
        Key::Dict(items) => {
            let mut m = Map::new();
            for (k, p) in items {
                let v = traverse_inner(obj, p, false);
                if !v.is_null() {
                    m.set(k, v);
                }
            }
            (false, vec![if m.0.is_empty() { Value::Null } else { Value::Dict(m) }])
        }
        _ if matches!(obj, Value::Dict(_)) => {
            let m = obj.as_dict().unwrap();
            let r = match key {
                Key::Str(s) => m.g(s).clone(),
                _ => Value::Null,
            };
            (false, vec![r])
        }
        Key::Int(i) => match obj {
            Value::List(l) => (false, vec![py_index(l.len(), *i).map(|j| l[j].clone()).unwrap_or(Value::Null)]),
            _ => {
                let s: Vec<char> = py_str(obj).chars().collect();
                (false, vec![py_index(s.len(), *i).map(|j| Value::Str(s[j].to_string())).unwrap_or(Value::Null)])
            }
        },
        Key::Slice(a, b, c) => match obj {
            Value::List(l) => (true, py_slice(l, *a, *b, *c)),
            _ => {
                let s: Vec<char> = py_str(obj).chars().collect();
                (false, vec![Value::Str(py_slice(&s, *a, *b, *c).into_iter().collect())])
            }
        },
        Key::Str(_) => (false, vec![Value::Null]),
    }
}

fn traverse_inner(obj: &Value, path: &[Key], allow_empty: bool) -> Value {
    let (results, branched, is_dict) = apply_path(obj, path);
    let results: Vec<Value> = results.into_iter().filter(|v| !empty_or_none(v)).collect();
    if branched {
        if !results.is_empty() {
            return Value::List(results);
        }
        return if allow_empty { Value::List(vec![]) } else { Value::Null };
    }
    match results.into_iter().next() {
        Some(v) => v,
        None if allow_empty && is_dict => Value::Dict(Map::new()),
        None => Value::Null,
    }
}

fn traverse_infodict(info: &Map, fields: &str) -> Value {
    let path = parse_path(fields);
    traverse_inner(&Value::Dict(info.clone()), &path, true)
}

// ---------------------------------------------------------------- evaluation

fn get_value(m: &Internal, info: &Map, ctx: &Ctx) -> Value {
    let mut value = traverse_infodict(info, &m.fields);
    if m.negate {
        value = match util::float_or_none(&value) {
            Some(f) => Value::Float(-f),
            None => value,
        };
    }
    if !m.maths.is_empty() {
        let mut cur = util::float_or_none(&value);
        let c: Vec<char> = m.maths.chars().collect();
        let mut p = 0;
        while p < c.len() {
            let op = c[p];
            let e = match_math_field(&c, p + 1);
            let item: String = c[p + 1..e].iter().collect();
            p = e;
            let (item, mult) = match item.strip_prefix('-') {
                Some(r) => (r.to_string(), -1.0),
                None => (item, 1.0),
            };
            let offset = util::float_str(&item).or_else(|| util::float_or_none(&traverse_infodict(info, &item)));
            cur = match (cur, offset) {
                (Some(v), Some(o)) => Some(match op {
                    '+' => v + mult * o,
                    '-' => v - mult * o,
                    _ => v * (mult * o),
                }),
                _ => return Value::Null,
            };
        }
        value = cur.map(Value::Float).unwrap_or(Value::Null);
    }
    if let Some(f) = &m.strf {
        value = util::strftime_or_none(&value, &f.replace("\\,", ",")).map(Value::Str).unwrap_or(Value::Null);
    }
    if ctx.sanitize.is_some() && value == Value::Str(String::new()) {
        value = Value::Null;
    }
    value
}

/// Python `str.format(replacement, value)` restricted to numbered fields, as _ReplacementFormatter.
fn replacement_format(repl: &str, value: &Value) -> Option<String> {
    let c: Vec<char> = repl.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        match c[i] {
            '{' if c.get(i + 1) == Some(&'{') => {
                out.push('{');
                i += 2;
            }
            '}' if c.get(i + 1) == Some(&'}') => {
                out.push('}');
                i += 2;
            }
            '{' => {
                let close = c[i..].iter().position(|&x| x == '}')? + i;
                let inner: String = c[i + 1..close].iter().collect();
                let (name, spec) = inner.split_once(':').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((inner, None));
                let (name, conv) = name.split_once('!').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((name, None));
                if name.is_empty() || !name.chars().all(|x| x.is_ascii_digit()) {
                    return None;
                }
                let s = match conv.as_deref() {
                    Some("r") => pyfmt::repr(value),
                    Some("a") => pyfmt::ascii(value),
                    _ => py_str(value),
                };
                match spec.as_deref() {
                    None | Some("") => out.push_str(&s),
                    Some(_) => out.push_str(&s), // format specs on replacements are rare; keep the text
                }
                i = close + 1;
            }
            '}' => return None,
            ch => {
                out.push(ch);
                i += 1;
            }
        }
    }
    Some(out)
}

fn variadic(v: &Value) -> Vec<Value> {
    match v {
        Value::List(l) => l.clone(),
        Value::Dict(m) => m.0.iter().map(|(k, _)| Value::Str(k.clone())).collect(),
        _ => vec![v.clone()],
    }
}

fn create_key(key: &str, format: &str, info: &Map, ctx: &Ctx) -> Result<String, String> {
    let na = ctx.na.to_string();
    let mut value = Value::Null;
    let mut replacement: Option<String> = None;
    let mut default = na.clone();
    let mut last_field = String::new();
    let mut mobj = match_internal(key);
    while let Some(m) = mobj {
        if let Some(d) = &m.default {
            default = d.clone();
        }
        value = get_value(&m, info, ctx);
        last_field = m.fields.clone();
        replacement = m.replacement.clone();
        if value.is_null() && m.alternate.is_some() {
            mobj = match_internal(&m.remaining[1..]);
        } else {
            break;
        }
    }

    if let (false, Some(r)) = (value.is_null(), &replacement) {
        match replacement_format(r, &value) {
            Some(s) => value = Value::Str(s),
            None => {
                value = Value::Null;
                default = na.clone();
            }
        }
    }

    let mut fmt = format.to_string();
    if fmt == "s" && matches!(value, Value::Int(_)) {
        let size = match last_field.as_str() {
            "playlist_index" => Some(py_str(info.g("__last_playlist_index")).len().max(1)),
            "playlist_autonumber" => Some(match info.g("n_entries") {
                Value::Null => 1,
                v => py_str(v).len(),
            }),
            "autonumber" => Some(ctx.autonumber_size),
            _ => None,
        };
        if let Some(n) = size {
            fmt = format!("0{n}d");
        }
    }

    let spec0 = Spec::parse(&fmt).ok_or_else(|| format!("unsupported format {fmt}"))?;
    let flags = spec0.flags.clone();
    let conv = spec0.conv;
    let str_fmt = format!("{}s", &fmt[..fmt.len() - 1]);
    let mut fmt_out = fmt.clone();

    if value.is_null() {
        value = Value::Str(default.clone());
        fmt_out = "s".into();
    } else {
        match conv {
            'l' => {
                let delim = if flags.contains('#') { "\n" } else { ", " };
                value = Value::Str(variadic(&value).iter().map(py_str).collect::<Vec<_>>().join(delim));
                fmt_out = str_fmt.clone();
            }
            'j' => {
                let o = DumpOpts { indent: flags.contains('#').then_some(4), ensure_ascii: !flags.contains('+') };
                value = Value::Str(dumps_with(&value, &o));
                fmt_out = str_fmt.clone();
            }
            'h' => {
                value = Value::Str(util::escape_html(&py_str(&value)));
                fmt_out = str_fmt.clone();
            }
            'q' => {
                let items = if flags.contains('#') { variadic(&value) } else { vec![value.clone()] };
                value = Value::Str(items.iter().map(|v| util::shell_quote(&py_str(v))).collect::<Vec<_>>().join(" "));
                fmt_out = str_fmt.clone();
            }
            'B' => {
                let bytes = py_str(&value).into_bytes();
                let spec = Spec::parse(&str_fmt).unwrap();
                let mut b = bytes;
                if let Some(p) = spec.precision {
                    b.truncate(p);
                }
                let w = spec.width.unwrap_or(0);
                if b.len() < w {
                    let fill = vec![b' '; w - b.len()];
                    b = if spec.flags.contains('-') { [b, fill].concat() } else { [fill, b].concat() };
                }
                value = Value::Str(String::from_utf8_lossy(&b).replace('\u{FFFD}', ""));
                fmt_out = "s".into();
            }
            'U' => {
                // Unicode normalization tables are not carried; text is passed through.
                fmt_out = str_fmt.clone();
            }
            'D' => {
                let num_fmt = fmt[..fmt.len() - 1].replace('#', "");
                let factor = if flags.contains('#') { 1024.0 } else { 1000.0 };
                value = match util::format_decimal_suffix(&value, &num_fmt, factor) {
                    Some(s) => Value::Str(s),
                    None => Value::Null,
                };
                fmt_out = "s".into();
                if value.is_null() {
                    // `'%s' % None`
                    value = Value::Str("None".into());
                }
            }
            'S' => {
                value = Value::Str(util::sanitize_filename(&py_str(&value), flags.contains('#'), None));
                fmt_out = str_fmt.clone();
            }
            'c' => {
                if value.truthy() {
                    value = Value::Str(py_str(&value).chars().next().map(String::from).unwrap_or_default());
                } else {
                    fmt_out = str_fmt.clone();
                }
            }
            'r' | 's' | 'a' => {}
            _ => match util::float_or_none(&value) {
                Some(f) => value = Value::Float(f),
                None => {
                    value = Value::Str(default.clone());
                    fmt_out = "s".into();
                }
            },
        }
    }

    if let Some(s) = ctx.sanitize {
        let last = fmt_out.chars().last().unwrap();
        if last == 'r' {
            value = Value::Str(pyfmt::repr(&value));
            fmt_out = str_fmt.clone();
        } else if last == 'a' {
            value = Value::Str(pyfmt::ascii(&value));
            fmt_out = str_fmt.clone();
        }
        if "csra".contains(fmt_out.chars().last().unwrap()) {
            let text = py_str(&value);
            value = Value::Str(if s.slash_only {
                text.replace('/', "\u{29F8}").replace('\0', "")
            } else {
                util::sanitize_filename(&text, s.restricted, None)
            });
        }
    }

    let spec = Spec::parse(&fmt_out).ok_or_else(|| format!("unsupported format {fmt_out}"))?;
    pyfmt::format_value(&spec, &value)
}
