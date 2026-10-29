// `--verbose`: the debug header YoutubeDL prints on start (`print_debug_header`) and the
// executable probing behind it (`FFmpegPostProcessor`, `get_exe_version`, the JS runtimes).

use std::process::{Command, Stdio};

use regex::Regex;

use crate::pyfmt;

// Facts about the runtime the original runs on that a native binary cannot observe.
pub const RELEASE_GIT_HEAD: &str = "594bd50c2c78ac432f81600d309fdc4e0a92d82c";
const PYTHON_VERSION: &str = "3.14.8";
const OPENSSL_VERSION: &str = "OpenSSL 3.6.5 29 Sep 2026";
const OPTIONAL_LIBRARIES: &str = "certifi-2026.10.19, pycrypto-4.1.0, requests-2.34.2, sqlite3-3.54.0, urllib3-2.8.0, yt_dlp_ejs-0.8.0";
const REQUEST_HANDLERS: &str = "urllib, requests";
pub const EXTRACTOR_COUNT: usize = 1744;

/// `_get_exe_version_output`: stdout and stderr together; None for a failing exit status,
/// Some(None) when the program cannot be run at all (OSError).
fn exe_version_output(exe: &str, args: &[&str], ignore_return_code: bool) -> Option<Option<String>> {
    let out = match Command::new(exe).args(args).stdin(Stdio::piped()).output() {
        Ok(o) => o,
        Err(_) => return Some(None),
    };
    if !ignore_return_code && !out.status.success() {
        return None;
    }
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Some(Some(text))
}

fn detect_exe_version(output: &str, version_re: &str, unrecognized: &str) -> String {
    match Regex::new(version_re).ok().and_then(|re| re.captures(output)) {
        Some(c) => c[1].to_string(),
        None => unrecognized.to_string(),
    }
}

/// `get_exe_version` with the two-valued `unrecognized` default; None when absent or empty.
fn get_exe_version(exe: &str, args: &[&str], version_re: &str) -> Option<String> {
    match exe_version_output(exe, args, false) {
        None => Some("broken".into()),
        Some(None) => None,
        Some(Some(out)) if out.is_empty() => None,
        Some(Some(out)) => Some(detect_exe_version(&out, version_re, "present")),
    }
}

fn version_tuple(v: &str) -> Vec<i64> {
    v.split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect()
}

/// `FFmpegPostProcessor._get_ffmpeg_version`: (version, enabled features).
fn ffmpeg_version(prog: &str) -> Option<(String, Vec<&'static str>)> {
    let out = match exe_version_output(prog, &["-bsfs"], false) {
        Some(Some(out)) if !out.is_empty() => out,
        _ => return None,
    };
    let mut ver = detect_exe_version(&out, r"version\s+([-0-9._a-zA-Z]+)", "present");
    for re in [r"^(?:\d+:)?([0-9.]+)-[0-9]+ubuntu[0-9.]+$", r"^n([0-9.]+)$"] {
        if let Some(c) = Regex::new(re).unwrap().captures(&ver) {
            ver = c[1].to_string();
        }
    }
    if prog != "ffmpeg" {
        return Some((ver, vec![]));
    }
    let lavf = Regex::new(r"(?m)^\s+libavformat\s+(?:[0-9. ]+)\s+/\s+(?P<runtime>[0-9. ]+)")
        .unwrap()
        .captures(&out)
        .map(|c| c["runtime"].replace(' ', ""));
    let needs_adtstoasc = match &lavf {
        None => true,
        Some(v) => version_tuple(v) < version_tuple("57.56.100"),
    };
    let mut features = Vec::new();
    if out.contains("--enable-libfdk-aac") {
        features.push("fdk");
    }
    if needs_adtstoasc {
        features.push("needs_adtstoasc");
    }
    if out.lines().any(|l| l == "setts") {
        features.push("setts");
    }
    Some((ver, features))
}

fn exe_versions() -> String {
    let mut exes: Vec<(&str, String)> = Vec::new();
    if let Some((mut ver, features)) = ffmpeg_version("ffmpeg") {
        if !features.is_empty() {
            ver += &format!(" ({})", features.join(","));
        }
        exes.push(("ffmpeg", ver));
    }
    if let Some((ver, _)) = ffmpeg_version("ffprobe") {
        exes.push(("ffprobe", ver));
    }
    if let Some(v) = get_exe_version("rtmpdump", &["--help"], r"(?i)RTMPDump\s*v?([0-9a-zA-Z._-]+)") {
        exes.push(("rtmpdump", v));
    }
    if let Some(v) = get_exe_version("phantomjs", &["--version"], r"([0-9.]+)") {
        exes.push(("phantomjs", v));
    }
    exes.sort();
    let s: Vec<String> = exes.iter().filter(|(_, v)| !v.is_empty()).map(|(e, v)| format!("{e} {v}")).collect();
    if s.is_empty() { "none".into() } else { s.join(", ") }
}

/// The default `js_runtimes` is `{'deno': {}}`.
fn js_runtimes() -> String {
    let out = match exe_version_output("deno", &["--version"], false) {
        Some(Some(out)) if !out.is_empty() => out,
        _ => return "none".into(),
    };
    let version = detect_exe_version(&out, r"^deno (\S+)", "unknown");
    let supported = version_tuple(&version) >= vec![2, 3, 0];
    format!("deno-{version}{}", if supported { "" } else { " (unsupported)" })
}

/// `urllib.request.getproxies()` from the environment, plus yt-dlp's http → https compat.
fn proxy_map() -> String {
    let vars: Vec<(String, String)> = std::env::vars().collect();
    let mut proxies: Vec<(String, String)> = Vec::new();
    let set =|proxies: &mut Vec<(String, String)>, k: &str, v: &str| match proxies.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = v.to_string(),
        None => proxies.push((k.to_string(), v.to_string())),
    };
    for (name, value) in &vars {
        let name = name.to_lowercase();
        if !value.is_empty() && name.ends_with("_proxy") {
            set(&mut proxies, &name[..name.len() - 6], value);
        }
    }
    if vars.iter().any(|(n, _)| n == "REQUEST_METHOD") {
        proxies.retain(|(k, _)| k != "http");
    }
    for (name, value) in &vars {
        if name.ends_with("_proxy") {
            let name = name.to_lowercase();
            let key = &name[..name.len() - 6];
            if value.is_empty() {
                proxies.retain(|(k, _)| k != key);
            } else {
                set(&mut proxies, key, value);
            }
        }
    }
    if let Some(http) = proxies.iter().find(|(k, _)| k == "http").map(|(_, v)| v.clone()) {
        if !proxies.iter().any(|(k, _)| k == "https") {
            proxies.push(("https".into(), http));
        }
    }
    let items: Vec<String> = proxies.iter().map(|(k, v)| format!("{}: {}", pyfmt::str_repr(k), pyfmt::str_repr(v))).collect();
    format!("{{{}}}", items.join(", "))
}

unsafe extern "C" {
    fn isatty(fd: i32) -> i32;
    fn gnu_get_libc_version() -> *const std::ffi::c_char;
}

/// `supports_terminal_sequences` on a POSIX system.
fn supports_terminal_sequences(fd: i32) -> bool {
    std::env::var("TERM").is_ok_and(|t| !t.is_empty()) && unsafe { isatty(fd) } == 1
}

fn stream_encoding(fd: i32) -> String {
    let mut info = Vec::new();
    if std::env::var("TERM").unwrap_or_default().to_lowercase() == "dumb" {
        info.push("dumb");
    }
    if !supports_terminal_sequences(fd) {
        info.push("No ANSI");
    }
    if info.is_empty() { "utf-8".into() } else { format!("utf-8 ({})", info.join(",")) }
}

/// `platform.platform()` on Linux: system-release-machine-with-libc.
fn platform() -> String {
    let release = std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().to_string();
    let glibc = unsafe { std::ffi::CStr::from_ptr(gnu_get_libc_version()) }.to_string_lossy().into_owned();
    format!("Linux-{release}-{}-with-glibc{glibc}", std::env::consts::ARCH)
}

fn system_identifier() -> String {
    let glibc = unsafe { std::ffi::CStr::from_ptr(gnu_get_libc_version()) }.to_string_lossy().into_owned();
    format!(
        "Python {PYTHON_VERSION} (CPython {} 64bit) - {} ({OPENSSL_VERSION}, glibc {glibc})",
        std::env::consts::ARCH,
        platform()
    )
}

/// `Config.__str__` for the command line, as `parse_options` prints it when verbose.
pub fn command_line_config(args: &[String]) -> String {
    const PRIVATE_OPTS: &[&str] = &["-p", "--password", "-u", "--username", "--video-password", "--ap-password", "--ap-username"];
    let mut opts: Vec<String> = args
        .iter()
        .map(|o| match o.split_once('=') {
            Some((k, v)) if PRIVATE_OPTS.contains(&k) && !v.is_empty() => format!("{k}=PRIVATE"),
            _ => o.clone(),
        })
        .collect();
    for idx in 0..opts.len() {
        if PRIVATE_OPTS.contains(&opts[idx].as_str()) && idx + 1 < opts.len() {
            opts[idx + 1] = "PRIVATE".into();
        }
    }
    let items: Vec<String> = opts.iter().map(|s| pyfmt::str_repr(s)).collect();
    format!("[debug] Command-line config: [{}]", items.join(", "))
}

/// `YoutubeDL.print_debug_header`, line by line. `screen_is_stderr` is quiet mode.
pub fn debug_header(screen_is_stderr: bool) -> Vec<String> {
    let screen = if screen_is_stderr { 2 } else { 1 };
    let lines = [
        format!(
            "Encodings: locale UTF-8, fs utf-8, pref UTF-8, out {}, error {}, screen {}",
            stream_encoding(1),
            stream_encoding(2),
            stream_encoding(screen)
        ),
        format!("yt-dlp version stable@{} from yt-dlp/yt-dlp [{}]", crate::VERSION, &RELEASE_GIT_HEAD[..9]),
        system_identifier(),
        format!("exe versions: {}", exe_versions()),
        format!("Optional libraries: {OPTIONAL_LIBRARIES}"),
        format!("JS runtimes: {}", js_runtimes()),
        format!("Proxy map: {}", proxy_map()),
        format!("Request Handlers: {REQUEST_HANDLERS}"),
        "Plugin directories: none".to_string(),
    ];
    lines.iter().map(|l| format!("[debug] {l}")).collect()
}
