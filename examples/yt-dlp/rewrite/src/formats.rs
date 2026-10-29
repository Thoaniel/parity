// Format sorting (utils.FormatSorter) and format selection (YoutubeDL.build_format_selector).

use std::collections::HashMap;

use crate::json::{Map, Value};
use crate::pyfmt::py_str;
use crate::util;

// ---------------------------------------------------------------- Python-number helpers

/// `a - b` / `a + b` on JSON numbers, keeping ints as ints. None on a type error.
pub fn num_op(a: &Value, b: &Value, op: char) -> Option<Value> {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => Some(Value::Int(if op == '+' { x + y } else { x - y })),
        _ if a.is_number() && b.is_number() => {
            let (x, y) = (a.as_f64()?, b.as_f64()?);
            Some(Value::Float(if op == '+' { x + y } else { x - y }))
        }
        _ => None,
    }
}

/// Python `sum(values)`.
pub fn py_sum(values: &[Value]) -> Value {
    values.iter().fold(Value::Int(0), |acc, v| num_op(&acc, v, '+').unwrap_or(acc))
}

/// FormatSorter._fill_sorting_fields
pub fn fill_sorting_fields(f: &mut Map) {
    if !f.g("protocol").truthy() {
        let p = determine_protocol(f);
        f.set("protocol", Value::Str(p));
    }
    if !f.g("ext").truthy() && f.contains("url") {
        let e = util::determine_ext(&py_str(f.g("url")), "unknown_video").to_lowercase();
        f.set("ext", Value::Str(e));
    }
    let ext = f.g("ext").clone();
    if f.str_of("vcodec") == Some("none") {
        let a = if f.str_of("acodec") != Some("none") { ext } else { Value::str("none") };
        f.set("audio_ext", a);
        f.set("video_ext", Value::str("none"));
    } else {
        f.set("video_ext", ext);
        f.set("audio_ext", Value::str("none"));
    }
    if f.g("preference").is_null() && f.str_of("ext") == Some("flv") {
        let vc = f.str_of("vcodec").unwrap_or("").to_string();
        let hevc = vc.starts_with("h265") || vc.starts_with("x265") || vc.starts_with("he") || vc.starts_with("hvc") || vc.starts_with("hev");
        if hevc {
            f.set("preference", Value::Int(-100));
        }
    }
    if f.str_of("vcodec") == Some("none") {
        f.set("vbr", Value::Int(0));
    }
    if f.str_of("acodec") == Some("none") {
        f.set("abr", Value::Int(0));
    }
    let falsy_or = |v: Option<Value>| v.filter(|x| x.truthy()).unwrap_or(Value::Null);
    let get = |f: &Map, k: &str| f.get(k).cloned();
    if !f.g("vbr").truthy() && f.str_of("vcodec") != Some("none") {
        let v = match (get(f, "tbr"), get(f, "abr")) {
            (Some(a), Some(b)) => num_op(&a, &b, '-'),
            _ => None,
        };
        f.set("vbr", falsy_or(v));
    }
    if !f.g("abr").truthy() && f.str_of("acodec") != Some("none") {
        let v = match (get(f, "tbr"), get(f, "vbr")) {
            (Some(a), Some(b)) => num_op(&a, &b, '-'),
            _ => None,
        };
        f.set("abr", falsy_or(v));
    }
    if !f.g("tbr").truthy() {
        let v = match (get(f, "vbr"), get(f, "abr")) {
            (Some(a), Some(b)) => num_op(&a, &b, '+'),
            _ => None,
        };
        f.set("tbr", falsy_or(v));
    }
}

pub fn determine_protocol(f: &Map) -> String {
    if let Some(p) = f.get("protocol").filter(|p| !p.is_null()) {
        return py_str(p);
    }
    let url = py_str(f.g("url"));
    if url.starts_with("rtmp") {
        return "rtmp".into();
    }
    let ext = util::determine_ext(&url, "unknown_video");
    if ext == "m3u8" {
        return if f.g("is_live").truthy() { "m3u8".into() } else { "m3u8_native".into() };
    } else if ext == "f4m" {
        return "f4m".into();
    }
    match url.find(':') {
        Some(i) if url[..i].chars().all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c)) && url[..i].chars().next().is_some_and(|c| c.is_ascii_alphabetic()) => url[..i].to_lowercase(),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------- sorting

#[derive(Clone, Debug, PartialEq)]
enum SV {
    Num(f64),
    Str(String),
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Func {
    AnyNotNone,
    FirstTruthy,
    MinTruthy,
}

#[derive(Clone, Debug, Default)]
struct Setting {
    ty: Option<&'static str>,
    field: Option<Vec<String>>,
    convert: Option<String>,
    order: Option<Vec<Option<&'static str>>>,
    order_free: Option<Vec<Option<&'static str>>>,
    regex: bool,
    visible: Option<bool>,
    forced: bool,
    priority: bool,
    max: Option<f64>,
    default: Option<f64>,
    not_in_list: Option<Vec<Option<&'static str>>>,
    function: Option<Func>,
    reverse: bool,
    closest: bool,
    limit_text: Option<String>,
    limit: Option<SV>,
    deprecated: bool,
    same_limit: bool,
}

const DEFAULT_ORDER: &[&str] = &[
    "hidden", "aud_or_vid", "hasvid", "ie_pref", "lang", "quality", "res", "fps", "hdr:12", "vcodec", "channels", "acodec", "size", "br",
    "asr", "proto", "ext", "hasaud", "source", "id",
];

fn ordered(order: &[Option<&'static str>], regex: bool, field: Option<&str>) -> Setting {
    Setting {
        ty: Some("ordered"),
        regex,
        order: Some(order.to_vec()),
        field: field.map(|f| vec![f.to_string()]),
        ..Default::default()
    }
}

fn default_settings() -> HashMap<String, Setting> {
    let mut s: HashMap<String, Setting> = HashMap::new();
    let some = |v: &[&'static str]| v.iter().map(|x| Some(*x)).collect::<Vec<_>>();
    let mut vcodec = some(&["av0?1", r"vp0?9\.0?2", "vp0?9", "[hx]265|he?vc?", "[hx]264|avc", "vp0?8", "mp4v|h263", "theora", ""]);
    vcodec.extend([None, Some("none")]);
    s.insert("vcodec".into(), ordered(&vcodec, true, None));
    let mut acodec = some(&["[af]lac", "wav|aiff", "opus", "vorbis|ogg", "aac", "mp?4a?", "mp3", "ac-?4", "e-?a?c-?3", "ac-?3", "dts", ""]);
    acodec.extend([None, Some("none")]);
    s.insert("acodec".into(), ordered(&acodec, true, None));
    let mut hdr = some(&["dv", "(hdr)?12", r"(hdr)?10\+", "(hdr)?10", "hlg", "", "sdr"]);
    hdr.push(None);
    s.insert("hdr".into(), ordered(&hdr, true, Some("dynamic_range")));
    s.insert(
        "proto".into(),
        ordered(&some(&["(ht|f)tps", "(ht|f)tp$", "m3u8.*", ".*dash", "websocket_frag", "rtmpe?", "", "ws|websocket", "f4"]), true, Some("protocol")),
    );
    let mut vext = ordered(&some(&["mp4", "mov", "webm", "flv", "", "none"]), false, Some("video_ext"));
    vext.order_free = Some(some(&["webm", "mp4", "mov", "flv", "", "none"]));
    s.insert("vext".into(), vext);
    let mut aext = ordered(&some(&["m4a", "aac", "mp3", "ogg", "opus", "web[am]", "", "none"]), true, Some("audio_ext"));
    aext.order_free = Some(some(&["ogg", "opus", "web[am]", "mp3", "m4a", "aac", "", "none"]));
    s.insert("aext".into(), aext);
    s.insert("hidden".into(), Setting { visible: Some(false), forced: true, ty: Some("extractor"), max: Some(-1000.0), ..Default::default() });
    s.insert(
        "aud_or_vid".into(),
        Setting {
            visible: Some(false),
            forced: true,
            ty: Some("multiple"),
            field: Some(vec!["vcodec".into(), "acodec".into()]),
            function: Some(Func::AnyNotNone),
            ..Default::default()
        },
    );
    s.insert("ie_pref".into(), Setting { priority: true, ty: Some("extractor"), ..Default::default() });
    s.insert(
        "hasvid".into(),
        Setting { priority: true, field: Some(vec!["vcodec".into()]), ty: Some("boolean"), not_in_list: Some(vec![Some("none")]), ..Default::default() },
    );
    s.insert("hasaud".into(), Setting { field: Some(vec!["acodec".into()]), ty: Some("boolean"), not_in_list: Some(vec![Some("none")]), ..Default::default() });
    let conv = |c: &str, field: Option<&str>, default: Option<f64>| Setting {
        convert: Some(c.into()),
        field: field.map(|f| vec![f.to_string()]),
        default,
        ..Default::default()
    };
    s.insert("lang".into(), conv("float", Some("language_preference"), Some(-1.0)));
    s.insert("quality".into(), conv("float", None, Some(-1.0)));
    s.insert("filesize".into(), conv("bytes", None, None));
    s.insert("fs_approx".into(), conv("bytes", Some("filesize_approx"), None));
    s.insert("id".into(), conv("string", Some("format_id"), None));
    for f in ["height", "width", "fps", "tbr", "vbr", "abr", "asr"] {
        s.insert(f.into(), conv("float_none", None, None));
    }
    s.insert("channels".into(), conv("float_none", Some("audio_channels"), None));
    s.insert("source".into(), conv("float", Some("source_preference"), Some(-1.0)));
    s.insert("codec".into(), Setting { ty: Some("combined"), field: Some(vec!["vcodec".into(), "acodec".into()]), ..Default::default() });
    s.insert(
        "br".into(),
        Setting {
            ty: Some("multiple"),
            field: Some(vec!["tbr".into(), "vbr".into(), "abr".into()]),
            convert: Some("float_none".into()),
            function: Some(Func::FirstTruthy),
            ..Default::default()
        },
    );
    s.insert(
        "size".into(),
        Setting {
            ty: Some("multiple"),
            field: Some(vec!["filesize".into(), "fs_approx".into()]),
            convert: Some("bytes".into()),
            function: Some(Func::FirstTruthy),
            ..Default::default()
        },
    );
    s.insert("ext".into(), Setting { ty: Some("combined"), field: Some(vec!["vext".into(), "aext".into()]), ..Default::default() });
    s.insert(
        "res".into(),
        Setting { ty: Some("multiple"), field: Some(vec!["height".into(), "width".into()]), function: Some(Func::MinTruthy), ..Default::default() },
    );
    let alias = |to: &str, deprecated: bool| Setting { ty: Some("alias"), field: Some(vec![to.to_string()]), deprecated, ..Default::default() };
    for (a, to) in [
        ("format_id", "id"),
        ("preference", "ie_pref"),
        ("language_preference", "lang"),
        ("source_preference", "source"),
        ("protocol", "proto"),
        ("filesize_approx", "fs_approx"),
        ("audio_channels", "channels"),
    ] {
        s.insert(a.into(), alias(to, false));
    }
    for (a, to) in [
        ("dimension", "res"),
        ("resolution", "res"),
        ("extension", "ext"),
        ("bitrate", "br"),
        ("total_bitrate", "tbr"),
        ("video_bitrate", "vbr"),
        ("audio_bitrate", "abr"),
        ("framerate", "fps"),
        ("filesize_estimate", "size"),
        ("samplerate", "asr"),
        ("video_ext", "vext"),
        ("audio_ext", "aext"),
        ("video_codec", "vcodec"),
        ("audio_codec", "acodec"),
        ("video", "hasvid"),
        ("has_video", "hasvid"),
        ("audio", "hasaud"),
        ("has_audio", "hasaud"),
        ("extractor", "ie_pref"),
        ("extractor_preference", "ie_pref"),
    ] {
        s.insert(a.into(), alias(to, true));
    }
    s
}

pub struct FormatSorter {
    settings: HashMap<String, Setting>,
    order: Vec<String>,
    use_free_order: bool,
    pub warnings: Vec<String>,
}

/// Python's `re.match(pattern, value)` for the small fixed patterns in the sort tables.
fn order_regex_match(pat: &str, value: &str) -> bool {
    crate::miniregex::Regex::new(pat).map(|r| r.match_start(value)).unwrap_or(false)
}

impl FormatSorter {
    pub fn new(user: &[String], extractor: &[String], prefer_free: bool, force: bool) -> Result<FormatSorter, String> {
        let mut s = FormatSorter { settings: default_settings(), order: Vec::new(), use_free_order: prefer_free, warnings: Vec::new() };
        s.evaluate_params(user, extractor, force)?;
        Ok(s)
    }

    fn ensure(&mut self, field: &str) {
        if !self.settings.contains_key(field) {
            self.warnings.push(format!(
                "Using arbitrary fields ({field}) for format sorting is deprecated and may be removed in a future version"
            ));
            self.settings.insert(field.to_string(), Setting::default());
        }
    }

    fn ty(&mut self, field: &str) -> &'static str {
        self.ensure(field);
        self.settings[field].ty.unwrap_or("field")
    }

    fn field_of(&mut self, field: &str) -> Vec<String> {
        let ty = self.ty(field);
        let s = self.settings.get_mut(field).unwrap();
        if s.field.is_none() {
            s.field = Some(if ty == "extractor" { vec!["preference".into()] } else { vec![field.to_string()] });
        }
        s.field.clone().unwrap()
    }

    fn convert(&mut self, field: &str) -> String {
        let ty = self.ty(field);
        let s = self.settings.get_mut(field).unwrap();
        if s.convert.is_none() {
            s.convert = Some(if ty == "ordered" { "order" } else if !field.is_empty() { "float_string" } else { "ignore" }.into());
        }
        s.convert.clone().unwrap()
    }

    fn resolve_field_value(&mut self, field: &str, value: Option<&str>, convert_none: bool) -> Option<SV> {
        let value = match value {
            None if !convert_none => return None,
            None => None,
            Some(v) => Some(v.to_lowercase()),
        };
        let conversion = self.convert(field);
        match conversion.as_str() {
            "ignore" => None,
            "string" => value.map(SV::Str),
            "float_none" => value.as_deref().and_then(util::float_str).map(SV::Num),
            "bytes" => value.as_deref().and_then(util::parse_bytes).map(SV::Num),
            "order" => {
                let st = &self.settings[field];
                let order_list = match (self.use_free_order, &st.order_free) {
                    (true, Some(of)) => of.clone(),
                    _ => st.order.clone().unwrap_or_default(),
                };
                let use_regex = st.regex;
                let n = order_list.len() as f64;
                let empty_pos = order_list.iter().position(|x| *x == Some("")).map(|p| p as f64).unwrap_or(n + 1.0);
                if let (true, Some(v)) = (use_regex, &value) {
                    for (i, r) in order_list.iter().enumerate() {
                        if let Some(r) = r {
                            if !r.is_empty() && order_regex_match(r, v) {
                                return Some(SV::Num(n - i as f64));
                            }
                        }
                    }
                    Some(SV::Num(n - empty_pos))
                } else {
                    let pos = order_list.iter().position(|x| x.map(String::from) == value).map(|p| p as f64).unwrap_or(empty_pos);
                    Some(SV::Num(n - pos))
                }
            }
            _ => {
                let v = value?;
                if !v.is_empty() && v.chars().all(|c| c.is_numeric()) {
                    v.parse::<f64>().ok().map(SV::Num).or(Some(SV::Str(v)))
                } else {
                    self.settings.get_mut(field).unwrap().convert = Some("string".into());
                    Some(SV::Str(v))
                }
            }
        }
    }

    fn evaluate_params(&mut self, user: &[String], extractor: &[String], force: bool) -> Result<(), String> {
        let mut sort_list: Vec<String> = Vec::new();
        for f in DEFAULT_ORDER {
            let name = f.split(':').next().unwrap();
            if self.settings.get(name).is_some_and(|s| s.forced) {
                sort_list.push(f.to_string());
            }
        }
        if !force {
            for f in DEFAULT_ORDER {
                let name = f.split(':').next().unwrap();
                if self.settings.get(name).is_some_and(|s| s.priority) {
                    sort_list.push(f.to_string());
                }
            }
        }
        sort_list.extend(user.iter().cloned());
        sort_list.extend(extractor.iter().cloned());
        sort_list.extend(DEFAULT_ORDER.iter().map(|s| s.to_string()));

        for item in sort_list {
            // regex: ' *((?P<reverse>\+)?(?P<field>[a-zA-Z0-9_]+)((?P<separator>[~:])(?P<limit>.*?))?)? *$'
            let t = item.trim_start_matches(' ');
            let (reverse, t) = match t.strip_prefix('+') {
                Some(r) => (true, r),
                None => (false, t),
            };
            let flen = t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').count();
            let mut field = t[..flen].to_string();
            let rest = &t[flen..];
            let (separator, limit_text) = match rest.chars().next() {
                Some(c @ ('~' | ':')) => (Some(c), Some(rest[1..].trim_end_matches(' ').to_string())),
                _ => (None, None),
            };
            if flen == 0 || (separator.is_none() && !rest.trim_end_matches(' ').is_empty()) {
                if flen == 0 && !reverse && t.trim().is_empty() {
                    continue;
                }
                return Err(format!("Invalid format sort string \"{item}\" given by extractor"));
            }
            if self.ty(&field) == "alias" {
                let alias = field.clone();
                field = self.field_of(&alias)[0].clone();
                if self.settings[&alias].deprecated {
                    self.warnings.push(format!(
                        "Format sorting alias {alias} is deprecated and may be removed in a future version. Please use {field} instead"
                    ));
                }
            }
            let closest = separator == Some('~');
            let has_limit = limit_text.is_some();
            let has_multiple_fields = self.ty(&field) == "combined";
            let has_multiple_limits = has_limit && has_multiple_fields && !self.settings[&field].same_limit;
            let fields = if has_multiple_fields { self.field_of(&field) } else { vec![field.clone()] };
            let limits: Vec<String> = match &limit_text {
                Some(l) if has_multiple_limits => l.split(':').map(String::from).collect(),
                Some(l) => vec![l.clone()],
                None => vec![],
            };
            for (i, f) in fields.iter().enumerate() {
                let lim = if i < limits.len() {
                    Some(limits[i].clone())
                } else if has_limit && !has_multiple_limits {
                    Some(limits[0].clone())
                } else {
                    None
                };
                self.add_item(f, reverse, closest, lim);
            }
        }
        Ok(())
    }

    fn add_item(&mut self, field: &str, reverse: bool, closest: bool, limit_text: Option<String>) {
        let field = field.to_lowercase();
        if self.order.contains(&field) {
            return;
        }
        self.order.push(field.clone());
        let limit = self.resolve_field_value(&field, limit_text.as_deref(), false);
        self.ensure(&field);
        let s = self.settings.get_mut(&field).unwrap();
        s.reverse = reverse;
        s.closest = limit.is_some() && closest;
        s.limit_text = limit_text;
        s.limit = limit;
    }

    /// The text `--verbose` prints after "Formats sorted by: ".
    pub fn describe(&mut self) -> String {
        let order = self.order.clone();
        let mut parts = Vec::new();
        for f in order {
            let is_order = self.convert(&f) == "order";
            let s = &self.settings[&f];
            if s.visible == Some(false) {
                continue;
            }
            let mut p = format!("{}{}", if s.reverse { "+" } else { "" }, f);
            if let Some(lt) = &s.limit_text {
                let lim = match &s.limit {
                    None => "None".to_string(),
                    Some(SV::Num(n)) => {
                        if n.fract() == 0.0 && is_order { format!("{}", *n as i64) } else { crate::pyfmt::float_repr(*n) }
                    }
                    Some(SV::Str(t)) => t.clone(),
                };
                p.push_str(&format!("{}{}({})", if s.closest { "~" } else { ":" }, lt, lim));
            }
            parts.push(p);
        }
        parts.join(", ")
    }

    fn value_pref(&mut self, field: &str, ty: &str, value: Value) -> Vec<SV> {
        let st = self.settings[field].clone();
        let mut value: Option<SV> = match ty {
            "extractor" => {
                let v = value.as_f64();
                match (v, st.max) {
                    (None, _) => Some(SV::Num(-1.0)),
                    (Some(x), Some(m)) if x >= m => Some(SV::Num(-1.0)),
                    (Some(_), _) => sv_of(&value),
                }
            }
            "boolean" => {
                // `value not in not_in_list` (in_list is never set)
                let not_in = st.not_in_list.clone().unwrap_or(vec![None]);
                let listed = not_in.iter().any(|x| match (x, &value) {
                    (None, Value::Null) => true,
                    (Some(s), Value::Str(v)) => s == v,
                    _ => false,
                });
                Some(SV::Num(if listed { -1.0 } else { 0.0 }))
            }
            "ordered" => {
                let s = value.as_str().map(String::from);
                self.resolve_field_value(field, s.as_deref(), true)
            }
            _ => sv_of(&value),
        };
        let val_num = match &value {
            None => st.default,
            Some(SV::Num(n)) => Some(*n),
            Some(SV::Str(s)) => util::float_str(s).or(st.default),
        };
        let is_num = self.convert(field) != "string" && val_num.is_some();
        if is_num {
            value = val_num.map(SV::Num);
        }
        let (reverse, closest, limit) = (st.reverse, st.closest, st.limit.clone());
        let lim = match &limit {
            Some(SV::Num(n)) => Some(*n),
            _ => None,
        };
        match value {
            None => vec![SV::Num(-10.0), SV::Num(0.0)],
            Some(v) if !is_num => vec![SV::Num(1.0), v, SV::Num(0.0)],
            Some(SV::Num(v)) => {
                if closest && lim.is_some() {
                    let l = lim.unwrap();
                    vec![SV::Num(0.0), SV::Num(-(v - l).abs()), SV::Num(if reverse { v - l } else { l - v })]
                } else if !reverse && lim.is_none_or(|l| v <= l) {
                    vec![SV::Num(0.0), SV::Num(v), SV::Num(0.0)]
                } else if lim.is_none() || (reverse && Some(v) == lim) || lim.is_some_and(|l| v > l) {
                    vec![SV::Num(0.0), SV::Num(-v), SV::Num(0.0)]
                } else {
                    vec![SV::Num(-1.0), SV::Num(v), SV::Num(0.0)]
                }
            }
            Some(v) => vec![SV::Num(1.0), v, SV::Num(0.0)],
        }
    }

    fn field_pref(&mut self, f: &Map, field: &str) -> Vec<SV> {
        let mut ty = self.ty(field);
        let value = if ty == "multiple" {
            ty = "field";
            let actual = self.field_of(field);
            let vals: Vec<Value> = actual
                .iter()
                .map(|a| {
                    let k = self.field_of(a)[0].clone();
                    f.g(&k).clone()
                })
                .collect();
            match self.settings[field].function {
                Some(Func::AnyNotNone) => Value::Int(vals.iter().any(|v| v.as_str() != Some("none")) as i64),
                Some(Func::FirstTruthy) => vals.into_iter().find(|v| v.truthy()).unwrap_or(Value::Null),
                Some(Func::MinTruthy) => vals
                    .into_iter()
                    .filter(|v| v.truthy())
                    .min_by(|a, b| a.as_f64().unwrap_or(0.0).partial_cmp(&b.as_f64().unwrap_or(0.0)).unwrap())
                    .unwrap_or(Value::Int(0)),
                None => Value::Null,
            }
        } else {
            let k = self.field_of(field)[0].clone();
            f.g(&k).clone()
        };
        self.value_pref(field, ty, value)
    }

    fn preference(&mut self, f: &mut Map) -> Vec<Vec<SV>> {
        fill_sorting_fields(f);
        let order = self.order.clone();
        order.iter().map(|field| self.field_pref(f, field)).collect()
    }

    /// Sorts worst to best, as `formats.sort(key=calculate_preference)`.
    pub fn sort(&mut self, formats: &mut Vec<Map>) {
        let mut keyed: Vec<(Vec<Vec<SV>>, Map)> = formats
            .drain(..)
            .map(|mut f| {
                let k = self.preference(&mut f);
                (k, f)
            })
            .collect();
        keyed.sort_by(|a, b| cmp_key(&a.0, &b.0));
        formats.extend(keyed.into_iter().map(|(_, f)| f));
    }
}

fn sv_of(v: &Value) -> Option<SV> {
    match v {
        Value::Null => None,
        Value::Str(s) => Some(SV::Str(s.clone())),
        v if v.is_number() => Some(SV::Num(v.as_f64().unwrap())),
        v => Some(SV::Str(py_str(v))),
    }
}

fn cmp_sv(a: &SV, b: &SV) -> std::cmp::Ordering {
    use std::cmp::Ordering::*;
    match (a, b) {
        (SV::Num(x), SV::Num(y)) => x.partial_cmp(y).unwrap_or(Equal),
        (SV::Str(x), SV::Str(y)) => x.cmp(y),
        (SV::Num(_), SV::Str(_)) => Less,
        (SV::Str(_), SV::Num(_)) => Greater,
    }
}

fn cmp_key(a: &[Vec<SV>], b: &[Vec<SV>]) -> std::cmp::Ordering {
    for (x, y) in a.iter().zip(b) {
        for (p, q) in x.iter().zip(y) {
            let o = cmp_sv(p, q);
            if o != std::cmp::Ordering::Equal {
                return o;
            }
        }
        let o = x.len().cmp(&y.len());
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

// ---------------------------------------------------------------- selection

#[derive(Clone, Debug)]
pub enum Sel {
    Single(String, Vec<String>),
    PickFirst(Box<Sel>, Box<Sel>, Vec<String>),
    Merge(Box<Sel>, Box<Sel>, Vec<String>),
    Group(Vec<Sel>, Vec<String>),
}

impl Sel {
    fn filters_mut(&mut self) -> &mut Vec<String> {
        match self {
            Sel::Single(_, f) | Sel::PickFirst(_, _, f) | Sel::Merge(_, _, f) | Sel::Group(_, f) => f,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Name(String, usize),
    Op(char, usize),
    Filter(String),
}

fn syntax_error(note: &str, spec: &str, col: usize) -> String {
    format!("Invalid format specification: {note}\n\t{spec}\n\t{}^", " ".repeat(col))
}

fn tokenize(spec: &str) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = spec.chars().collect();
    let mut toks = Vec::new();
    let mut cur = String::new();
    let mut cur_start = 0;
    let mut depth = 0i32;
    let mut i = 0;
    let flush = |cur: &mut String, toks: &mut Vec<Tok>, start: usize| {
        if !cur.is_empty() {
            toks.push(Tok::Name(std::mem::take(cur), start));
        }
    };
    while i < c.len() {
        let ch = c[i];
        if ch == '[' {
            flush(&mut cur, &mut toks, cur_start);
            let close = c[i + 1..].iter().position(|&x| x == ']').map(|p| p + i + 1);
            let Some(close) = close else {
                return Err(syntax_error("Missing closing/opening brackets or parenthesis", spec, c.len()));
            };
            let mut inner = String::new();
            let mut quote: Option<char> = None;
            for &x in &c[i + 1..close] {
                match quote {
                    Some(q) => {
                        inner.push(x);
                        if x == q {
                            quote = None;
                        }
                    }
                    None if x == '"' || x == '\'' => {
                        quote = Some(x);
                        inner.push(x);
                    }
                    None if x.is_whitespace() => {}
                    None => inner.push(x),
                }
            }
            toks.push(Tok::Filter(inner));
            i = close + 1;
            continue;
        }
        if "/+,()".contains(ch) {
            flush(&mut cur, &mut toks, cur_start);
            if ch == '(' {
                depth += 1;
            } else if ch == ')' {
                depth -= 1;
            }
            toks.push(Tok::Op(ch, i));
        } else if ch == ']' {
            return Err(syntax_error("Missing closing/opening brackets or parenthesis", spec, c.len()));
        } else if !ch.is_whitespace() {
            if cur.is_empty() {
                cur_start = i;
            }
            cur.push(ch);
        }
        i += 1;
    }
    flush(&mut cur, &mut toks, cur_start);
    if depth != 0 {
        return Err(syntax_error("Missing closing/opening brackets or parenthesis", spec, c.len()));
    }
    Ok(toks)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    pos: usize,
    spec: &'a str,
}

impl Parser<'_> {
    fn next(&mut self) -> Option<Tok> {
        let t = self.toks.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }
    fn restore(&mut self) {
        self.pos -= 1;
    }

    fn parse(&mut self, inside_merge: bool, inside_choice: bool, inside_group: bool) -> Result<Vec<Sel>, String> {
        let mut selectors = Vec::new();
        let mut current: Option<Sel> = None;
        while let Some(tok) = self.next() {
            match tok {
                Tok::Name(s, _) => current = Some(Sel::Single(s, vec![])),
                Tok::Filter(f) => {
                    let sel = current.get_or_insert_with(|| Sel::Single("best".into(), vec![]));
                    sel.filters_mut().push(f);
                }
                Tok::Op(op, start) => match op {
                    ')' => {
                        if !inside_group {
                            self.restore();
                        }
                        break;
                    }
                    '/' | ',' if inside_merge => {
                        self.restore();
                        break;
                    }
                    ',' if inside_choice => {
                        self.restore();
                        break;
                    }
                    ',' => {
                        let Some(c) = current.take() else {
                            return Err(syntax_error("\",\" must follow a format selector", self.spec, start));
                        };
                        selectors.push(c);
                    }
                    '/' => {
                        let Some(first) = current.take() else {
                            return Err(syntax_error("\"/\" must follow a format selector", self.spec, start));
                        };
                        let second = self.parse(false, true, false)?;
                        current = Some(Sel::PickFirst(Box::new(first), Box::new(Sel::Group(second, vec![])), vec![]));
                    }
                    '(' => {
                        if current.is_some() {
                            return Err(syntax_error("Unexpected \"(\"", self.spec, start));
                        }
                        let group = self.parse(false, false, true)?;
                        current = Some(Sel::Group(group, vec![]));
                    }
                    '+' => {
                        let Some(first) = current.take() else {
                            return Err(syntax_error("Unexpected \"+\"", self.spec, start));
                        };
                        let second = self.parse(true, false, false)?;
                        if second.is_empty() {
                            return Err(syntax_error("Expected a selector", self.spec, start));
                        }
                        current = Some(Sel::Merge(Box::new(first), Box::new(Sel::Group(second, vec![])), vec![]));
                    }
                    o => return Err(syntax_error(&format!("Operator not recognized: \"{o}\""), self.spec, start)),
                },
            }
        }
        if let Some(c) = current {
            selectors.push(c);
        }
        Ok(selectors)
    }
}

pub fn parse_format_spec(spec: &str) -> Result<Vec<Sel>, String> {
    let toks = tokenize(spec)?;
    let mut p = Parser { toks, pos: 0, spec };
    let sels = p.parse(false, false, false)?;
    // Validate the filters up front, as Python builds them with the selector.
    fn check(s: &Sel) -> Result<(), String> {
        let (fs, subs): (&Vec<String>, Vec<&Sel>) = match s {
            Sel::Single(_, f) => (f, vec![]),
            Sel::PickFirst(a, b, f) | Sel::Merge(a, b, f) => (f, vec![a.as_ref(), b.as_ref()]),
            Sel::Group(g, f) => (f, g.iter().collect()),
        };
        for f in fs {
            FormatFilter::parse(f)?;
        }
        subs.into_iter().try_for_each(check)
    }
    sels.iter().try_for_each(check)?;
    Ok(sels)
}

// ---------------------------------------------------------------- filters

#[derive(Debug)]
enum FilterOp {
    Num(String, f64),
    Str { op: String, neg: bool, value: String },
}

pub struct FormatFilter {
    key: String,
    none_inclusive: bool,
    op: FilterOp,
}

impl FormatFilter {
    pub fn parse(spec: &str) -> Result<FormatFilter, String> {
        let t = spec.trim();
        // numeric: key op ?value
        let klen = t.chars().take_while(|c| c.is_alphanumeric() || "_.-".contains(*c)).count();
        let key: String = t.chars().take(klen).collect();
        let rest = t[key.len()..].trim_start();
        if !key.is_empty() {
            for op in ["<=", ">=", "!=", "<", ">", "="] {
                if let Some(r) = rest.strip_prefix(op) {
                    let (ni, r) = match r.trim_start().strip_prefix('?') {
                        Some(x) => (true, x.trim()),
                        None => (false, r.trim()),
                    };
                    if !r.is_empty() && is_num_value(r) {
                        let v = match r.parse::<f64>() {
                            Ok(v) if r.chars().all(|c| c.is_ascii_digit() || c == '.') => Some(v),
                            _ => util::parse_filesize(r).or_else(|| util::parse_filesize(&format!("{r}B"))),
                        };
                        let Some(v) = v else {
                            return Err(format!("Invalid value {} in format specification {}", crate::pyfmt::str_repr(r), crate::pyfmt::str_repr(spec)));
                        };
                        return Ok(FormatFilter { key, none_inclusive: ni, op: FilterOp::Num(op.into(), v) });
                    }
                    break;
                }
            }
        }
        // string: key !?op ?value
        let klen = t.chars().take_while(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).count();
        let key: String = t.chars().take(klen).collect();
        let mut rest = t[key.len()..].trim_start();
        let neg = rest.starts_with('!');
        if neg {
            rest = rest[1..].trim_start();
        }
        if !key.is_empty() {
            for op in ["^=", "$=", "*=", "~=", "="] {
                if let Some(r) = rest.strip_prefix(op) {
                    let mut r = r.trim_start();
                    let ni = r.starts_with('?');
                    if ni {
                        r = r[1..].trim_start();
                    }
                    let r = r.trim_end();
                    let value = if let Some(q) = r.chars().next().filter(|c| *c == '"' || *c == '\'') {
                        if r.len() < 2 || !r.ends_with(q) {
                            break;
                        }
                        let inner = &r[1..r.len() - 1];
                        if inner.is_empty() {
                            break;
                        }
                        unescape_quotes(inner)
                    } else {
                        if r.is_empty() || !r.chars().all(|c| c.is_alphanumeric() || "_.-".contains(c)) {
                            break;
                        }
                        r.to_string()
                    };
                    return Ok(FormatFilter { key, none_inclusive: ni, op: FilterOp::Str { op: op.into(), neg, value } });
                }
            }
        }
        Err(format!("Invalid filter specification {}", crate::pyfmt::str_repr(spec)))
    }

    pub fn matches(&self, f: &Map) -> bool {
        let actual = f.g(&self.key);
        if actual.is_null() {
            return self.none_inclusive;
        }
        match &self.op {
            FilterOp::Num(op, v) => {
                let Some(a) = actual.as_f64() else {
                    return op == "!=";
                };
                match op.as_str() {
                    "<" => a < *v,
                    "<=" => a <= *v,
                    ">" => a > *v,
                    ">=" => a >= *v,
                    "=" => a == *v,
                    _ => a != *v,
                }
            }
            FilterOp::Str { op, neg, value } => {
                let a = py_str(actual);
                let r = match op.as_str() {
                    "=" => actual.as_str() == Some(value.as_str()),
                    "^=" => a.starts_with(value.as_str()),
                    "$=" => a.ends_with(value.as_str()),
                    "*=" => a.contains(value.as_str()),
                    _ => crate::miniregex::Regex::new(value).map(|r| r.search(&a)).unwrap_or(false),
                };
                r != *neg
            }
        }
    }
}

fn is_num_value(r: &str) -> bool {
    // [0-9.]+(?:[kKmMgGtTpPeEzZyY]i?[Bb]?)?
    let n = r.chars().take_while(|c| c.is_ascii_digit() || *c == '.').count();
    if n == 0 {
        return false;
    }
    let mut rest = &r[n..];
    if rest.is_empty() {
        return true;
    }
    let Some(c) = rest.chars().next() else { return true };
    if !"kKmMgGtTpPeEzZyY".contains(c) {
        return false;
    }
    rest = &rest[1..];
    rest = rest.strip_prefix('i').unwrap_or(rest);
    rest = rest.strip_prefix(['B', 'b']).unwrap_or(rest);
    rest.is_empty()
}

fn unescape_quotes(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' && matches!(it.peek(), Some('\\' | '"' | '\'')) {
            out.push(it.next().unwrap());
        } else {
            out.push(c);
        }
    }
    out
}

// ---------------------------------------------------------------- evaluation

pub struct SelectOpts {
    pub allow_multiple_video: bool,
    pub allow_multiple_audio: bool,
    pub merge_output_format: Option<String>,
    pub prefer_free_formats: bool,
}

struct SelCtx<'a> {
    formats: Vec<Map>,
    has_merged_format: bool,
    incomplete_formats: bool,
    opts: &'a SelectOpts,
}

fn codec(f: &Map, k: &str) -> Option<String> {
    f.str_of(k).map(String::from)
}

fn is_none_codec(f: &Map, k: &str) -> bool {
    f.str_of(k) == Some("none")
}

pub fn select_formats(formats: &[Map], sels: &[Sel], opts: &SelectOpts) -> Vec<Map> {
    let ctx = SelCtx {
        formats: formats.to_vec(),
        has_merged_format: formats.iter().any(|f| !is_none_codec(f, "acodec") && !is_none_codec(f, "vcodec")),
        incomplete_formats: formats.iter().all(|f| is_none_codec(f, "vcodec")) || formats.iter().all(|f| is_none_codec(f, "acodec")),
        opts,
    };
    run_list(sels, &ctx)
}

fn run_list(sels: &[Sel], ctx: &SelCtx) -> Vec<Map> {
    sels.iter().flat_map(|s| run(s, ctx)).collect()
}

fn run(sel: &Sel, ctx: &SelCtx) -> Vec<Map> {
    let filters = match sel {
        Sel::Single(_, f) | Sel::PickFirst(_, _, f) | Sel::Merge(_, _, f) | Sel::Group(_, f) => f,
    };
    let mut formats = ctx.formats.clone();
    for f in filters {
        let ff = FormatFilter::parse(f).expect("validated");
        formats.retain(|x| ff.matches(x));
    }
    let sub = SelCtx { formats, has_merged_format: ctx.has_merged_format, incomplete_formats: ctx.incomplete_formats, opts: ctx.opts };
    match sel {
        Sel::Group(g, _) => run_list(g, &sub),
        Sel::PickFirst(a, b, _) => {
            let first = run(a, &sub);
            if !first.is_empty() {
                return first;
            }
            run(b, &sub)
        }
        Sel::Merge(a, b, _) => {
            let xs = run(a, &sub);
            let ys = run(b, &sub);
            let mut out = Vec::new();
            for x in &xs {
                for y in &ys {
                    out.push(merge(x, y, sub.opts));
                }
            }
            out
        }
        Sel::Single(spec, _) => run_single(spec, &sub),
    }
}

fn run_single(spec: &str, ctx: &SelCtx) -> Vec<Map> {
    let spec = if spec.is_empty() { "best" } else { spec };
    let has_av = |f: &Map| !is_none_codec(f, "vcodec") || !is_none_codec(f, "acodec");
    if spec == "all" {
        return ctx.formats.iter().rev().cloned().collect();
    }
    if spec == "mergeall" {
        let formats: Vec<&Map> = ctx.formats.iter().filter(|f| has_av(f)).collect();
        let Some(last) = formats.last() else { return vec![] };
        let mut merged = (*last).clone();
        for f in formats[..formats.len() - 1].iter().rev() {
            merged = merge(&merged, f, ctx.opts);
        }
        return vec![merged];
    }
    let mut format_fallback = false;
    let mut separate_fallback: Option<Box<dyn Fn(&Map) -> bool>> = None;
    let mut format_reverse = true;
    let mut format_idx = 1usize;
    let filter_f: Box<dyn Fn(&Map) -> bool>;
    if let Some((bw, ty, modified, n)) = parse_best_worst(spec) {
        format_idx = n;
        format_reverse = bw == 'b';
        format_fallback = ty.is_none() && !modified;
        let base: Box<dyn Fn(&Map) -> bool> = match (ty, modified) {
            (Some(t), true) => Box::new(move |f: &Map| !is_none_codec(f, &format!("{t}codec"))),
            (Some(t), false) => {
                let not = if t == 'v' { 'a' } else { 'v' };
                Box::new(move |f: &Map| is_none_codec(f, &format!("{not}codec")))
            }
            (None, false) => Box::new(|f: &Map| !is_none_codec(f, "vcodec") && !is_none_codec(f, "acodec")),
            (None, true) => Box::new(|_: &Map| true),
        };
        filter_f = Box::new(move |f: &Map| base(f) && has_av(f));
    } else {
        let s = spec.to_string();
        if util::COMMON_AUDIO.contains(&spec) {
            filter_f = Box::new(move |f: &Map| f.str_of("ext") == Some(s.as_str()) && !is_none_codec(f, "acodec"));
        } else if util::COMMON_VIDEO.contains(&spec) || spec == "3gp" {
            let s2 = s.clone();
            filter_f = Box::new(move |f: &Map| f.str_of("ext") == Some(s.as_str()) && !is_none_codec(f, "acodec") && !is_none_codec(f, "vcodec"));
            separate_fallback = Some(Box::new(move |f: &Map| f.str_of("ext") == Some(s2.as_str()) && !is_none_codec(f, "vcodec")));
        } else if util::STORYBOARDS.contains(&spec) {
            filter_f = Box::new(move |f: &Map| f.str_of("ext") == Some(s.as_str()) && is_none_codec(f, "acodec") && is_none_codec(f, "vcodec"));
        } else {
            filter_f = Box::new(move |f: &Map| f.str_of("format_id") == Some(s.as_str()));
        }
    }
    let mut matches: Vec<&Map> = ctx.formats.iter().filter(|f| filter_f(f)).collect();
    if matches.is_empty() {
        if format_fallback && ctx.incomplete_formats {
            matches = ctx.formats.iter().filter(|f| has_av(f)).collect();
        } else if let (Some(sf), false) = (&separate_fallback, ctx.has_merged_format) {
            matches = ctx.formats.iter().filter(|f| sf(f)).collect();
        }
    }
    if format_reverse {
        matches.reverse();
    }
    matches.get(format_idx - 1).map(|f| vec![(*f).clone()]).unwrap_or_default()
}

/// `(?P<bw>best|worst|b|w)(?P<type>video|audio|v|a)?(?P<mod>\*)?(?:\.(?P<n>[1-9]\d*))?$`
fn parse_best_worst(spec: &str) -> Option<(char, Option<char>, bool, usize)> {
    let (bw, rest) = if let Some(r) = spec.strip_prefix("best") {
        ('b', r)
    } else if let Some(r) = spec.strip_prefix("worst") {
        ('w', r)
    } else if let Some(r) = spec.strip_prefix('b') {
        ('b', r)
    } else if let Some(r) = spec.strip_prefix('w') {
        ('w', r)
    } else {
        return None;
    };
    let (ty, rest) = if let Some(r) = rest.strip_prefix("video") {
        (Some('v'), r)
    } else if let Some(r) = rest.strip_prefix("audio") {
        (Some('a'), r)
    } else if let Some(r) = rest.strip_prefix('v') {
        (Some('v'), r)
    } else if let Some(r) = rest.strip_prefix('a') {
        (Some('a'), r)
    } else {
        (None, rest)
    };
    let (modified, rest) = match rest.strip_prefix('*') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    let n = if rest.is_empty() {
        1
    } else {
        let d = rest.strip_prefix('.')?;
        if d.is_empty() || d.starts_with('0') || !d.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        d.parse().ok()?
    };
    Some((bw, ty, modified, n))
}

fn requested_or_self(f: &Map) -> Vec<Map> {
    match f.get("requested_formats").and_then(|v| v.as_list()) {
        Some(l) => l.iter().filter_map(|v| v.as_dict().cloned()).collect(),
        None => vec![f.clone()],
    }
}

fn ordered_set(items: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for i in items {
        if !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

pub fn merge(f1: &Map, f2: &Map, opts: &SelectOpts) -> Map {
    let mut info: Vec<Map> = requested_or_self(f1);
    info.extend(requested_or_self(f2));

    if !opts.allow_multiple_video || !opts.allow_multiple_audio {
        let mut no_more_video = false;
        let mut no_more_audio = false;
        // Mirrors popping from the list while enumerating it: the item after a removed one is skipped.
        let mut i = 0;
        while i < info.len() {
            let fi = &info[i];
            if is_none_codec(fi, "acodec") && is_none_codec(fi, "vcodec") {
                info.remove(i);
                i += 1;
                continue;
            }
            let mut removed = false;
            for (allow, codec_key, no_more) in
                [(opts.allow_multiple_audio, "acodec", &mut no_more_audio), (opts.allow_multiple_video, "vcodec", &mut no_more_video)]
            {
                if !allow && !is_none_codec(&info[i], codec_key) {
                    if *no_more {
                        info.remove(i);
                        removed = true;
                        break;
                    }
                    *no_more = true;
                }
            }
            let _ = removed;
            i += 1;
        }
    }

    if info.len() == 1 {
        return info.remove(0);
    }

    let video: Vec<&Map> = info.iter().filter(|f| !is_none_codec(f, "vcodec")).collect();
    let audio: Vec<&Map> = info.iter().filter(|f| !is_none_codec(f, "acodec")).collect();
    let only_video = if video.len() == 1 { Some(video[0].clone()) } else { None };
    let only_audio = if audio.len() == 1 { Some(audio[0].clone()) } else { None };

    let prefs: Option<Vec<String>> = match &opts.merge_output_format {
        Some(m) => Some(m.split('/').map(String::from).collect()),
        None if opts.prefer_free_formats => Some(vec!["webm".into(), "mkv".into()]),
        None => None,
    };
    let output_ext = util::get_compatible_ext(
        &video.iter().map(|f| codec(f, "vcodec")).collect::<Vec<_>>(),
        &audio.iter().map(|f| codec(f, "acodec")).collect::<Vec<_>>(),
        &video.iter().map(|f| py_str(f.g("ext"))).collect::<Vec<_>>(),
        &audio.iter().map(|f| py_str(f.g("ext"))).collect::<Vec<_>>(),
        prefs.as_deref(),
    );

    let filtered = |keys: &[&str]| -> Vec<Value> {
        info.iter()
            .filter_map(|f| keys.iter().map(|k| f.g(k)).find(|v| !v.is_null()).cloned())
            .filter(|v| v.truthy())
            .collect()
    };
    let join = |vals: Vec<Value>| vals.iter().map(py_str).collect::<Vec<_>>().join("+");
    let or_none = |s: String| if s.is_empty() { Value::Null } else { Value::Str(s) };

    let mut d = Map::new();
    d.set("requested_formats", Value::List(info.iter().cloned().map(Value::Dict).collect()));
    d.set("format", Value::Str(join(filtered(&["format"]))));
    d.set("format_id", Value::Str(join(filtered(&["format_id"]))));
    d.set("ext", Value::Str(output_ext));
    d.set("protocol", Value::Str(info.iter().map(determine_protocol).collect::<Vec<_>>().join("+")));
    d.set("language", or_none(ordered_set(filtered(&["language"]).iter().map(py_str).collect()).join("+")));
    d.set("format_note", or_none(ordered_set(filtered(&["format_note"]).iter().map(py_str).collect()).join("+")));
    let fs = py_sum(&filtered(&["filesize", "filesize_approx"]));
    d.set("filesize_approx", if fs.truthy() { fs } else { Value::Null });
    d.set("tbr", py_sum(&filtered(&["tbr", "vbr", "abr"])));

    if let Some(v) = &only_video {
        let res = match v.get("resolution").filter(|r| r.truthy()) {
            Some(r) => r.clone(),
            None => Value::Str(util::format_resolution(v, Some("unknown")).unwrap()),
        };
        for (k, val) in [
            ("width", v.g("width").clone()),
            ("height", v.g("height").clone()),
            ("resolution", res),
            ("fps", v.g("fps").clone()),
            ("dynamic_range", v.g("dynamic_range").clone()),
            ("vcodec", v.g("vcodec").clone()),
            ("vbr", v.g("vbr").clone()),
            ("stretched_ratio", v.g("stretched_ratio").clone()),
            ("aspect_ratio", v.g("aspect_ratio").clone()),
        ] {
            d.set(k, val);
        }
    }
    if let Some(a) = &only_audio {
        for k in ["acodec", "abr", "asr", "audio_channels"] {
            d.set(k, a.g(k).clone());
        }
    }
    d
}
