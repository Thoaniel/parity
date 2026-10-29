// The Rust rewrite of yt-dlp, written until `parity replay` reports no divergence.

mod debug;
mod filters;
mod formats;
mod json;
mod miniregex;
mod outtmpl;
mod pyfmt;
mod util;

use std::collections::HashMap;
use std::io::Write;
use std::process::exit;

use formats::{FormatSorter, SelectOpts};
use json::{Map, Value};
use pyfmt::py_str;

pub const VERSION: &str = "2026.08.19";
const USAGE: &str = "Usage: yt-dlp [OPTIONS] URL [URL...]";
const DEFAULT_OUTTMPL: &str = "%(title)s [%(id)s].%(ext)s";

// ---------------------------------------------------------------- options

#[derive(Default)]
struct Opts {
    load_info_json: Option<String>,
    format: Option<String>,
    format_sort: Vec<String>,
    format_sort_force: bool,
    merge_output_format: Option<String>,
    prefer_free_formats: bool,
    allow_multiple_video: bool,
    allow_multiple_audio: bool,
    /// --print, keyed by WHEN
    forceprint: Vec<(String, String)>,
    print_to_file: Vec<(String, String, String)>,
    match_filter: Vec<String>,
    outtmpl: Option<String>,
    paths: HashMap<String, String>,
    restrict_filenames: bool,
    windows_filenames: Option<bool>,
    trim_filenames: usize,
    na_placeholder: Option<String>,
    autonumber_size: Option<usize>,
    autonumber_start: Option<i64>,
    simulate: Option<bool>,
    quiet: Option<bool>,
    no_warnings: bool,
    verbose: bool,
    gettitle: bool,
    getid: bool,
    geturl: bool,
    getthumbnail: bool,
    getdescription: bool,
    getduration: bool,
    getfilename: bool,
    getformat: bool,
    dumpjson: bool,
    dump_single_json: bool,
    ignore_no_formats_error: bool,
    useid: bool,
    list_thumbnails: bool,
    listformats: bool,
    /// `listformats_table` is False
    listformats_old: bool,
    allow_unplayable_formats: bool,
    list_impersonate_targets: bool,
    list_extractors: bool,
    list_extractor_descriptions: bool,
    ap_list_mso: bool,
    /// --add-headers, keys lowercased, as the dict `_dict_from_options_callback` builds.
    headers: Map,
    user_agent: Option<String>,
    referer: Option<String>,
    /// Deprecated switches and arguments as given (`_deprecated_options`).
    deprecated_options: Vec<String>,
    /// --cache-dir / --no-cache-dir: None is the default location, Some(None) is disabled.
    cachedir: Option<Option<String>>,
    rm_cachedir: bool,
    urls: Vec<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum Arity {
    Flag,
    One,
    Two,
    Three,
}

impl Arity {
    fn count(self) -> usize {
        match self {
            Arity::Flag => 0,
            Arity::One => 1,
            Arity::Two => 2,
            Arity::Three => 3,
        }
    }
}

/// Every option create_parser() defines (options.py), as (names, nargs).
/// Names in one entry belong to the same optparse Option object.
#[rustfmt::skip]
const OPTION_TABLE: &[(&[&str], Arity)] = {
    use Arity::*;
    &[
        // General
        (&["-h", "--help"], Flag), (&["--version"], Flag), (&["-U", "--update"], Flag), (&["--no-update"], Flag),
        (&["--update-to"], One), (&["-i", "--ignore-errors"], Flag), (&["--no-abort-on-error"], Flag),
        (&["--abort-on-error", "--no-ignore-errors"], Flag), (&["--list-extractors"], Flag), (&["--extractor-descriptions"], Flag),
        (&["--use-extractors", "--ies"], One), (&["--force-generic-extractor"], Flag), (&["--default-search"], One),
        (&["--ignore-config", "--no-config"], Flag), (&["--no-config-locations"], Flag), (&["--config-locations"], One),
        (&["--plugin-dirs"], One), (&["--no-plugin-dirs"], Flag), (&["--js-runtimes"], One), (&["--no-js-runtimes"], Flag),
        (&["--remote-components"], One), (&["--no-remote-components"], Flag), (&["--flat-playlist"], Flag),
        (&["--no-flat-playlist"], Flag), (&["--live-from-start"], Flag), (&["--no-live-from-start"], Flag),
        (&["--wait-for-video"], One), (&["--no-wait-for-video"], Flag), (&["--mark-watched"], Flag), (&["--no-mark-watched"], Flag),
        (&["--no-colors", "--no-colours"], Flag), (&["--color"], One), (&["--compat-options"], One), (&["--alias"], Two),
        (&["-t", "--preset-alias"], One),
        // Network
        (&["--proxy"], One), (&["--socket-timeout"], One), (&["--source-address"], One), (&["--impersonate"], One),
        (&["--list-impersonate-targets"], Flag), (&["-4", "--force-ipv4"], Flag), (&["-6", "--force-ipv6"], Flag),
        (&["--enable-file-urls"], Flag),
        // Geo-restriction
        (&["--geo-verification-proxy"], One), (&["--xff"], One), (&["--geo-bypass"], Flag), (&["--no-geo-bypass"], Flag),
        (&["--geo-bypass-country"], One), (&["--geo-bypass-ip-block"], One),
        // Video selection
        (&["--playlist-start"], One), (&["--playlist-end"], One), (&["-I", "--playlist-items"], One), (&["--match-title"], One),
        (&["--reject-title"], One), (&["--min-filesize"], One), (&["--max-filesize"], One), (&["--date"], One),
        (&["--datebefore"], One), (&["--dateafter"], One), (&["--min-views"], One), (&["--max-views"], One),
        (&["--match-filters"], One), (&["--no-match-filters"], Flag), (&["--break-match-filters"], One),
        (&["--no-break-match-filters"], Flag), (&["--no-playlist"], Flag), (&["--yes-playlist"], Flag), (&["--age-limit"], One),
        (&["--download-archive"], One), (&["--no-download-archive"], Flag), (&["--max-downloads"], One),
        (&["--break-on-existing"], Flag), (&["--no-break-on-existing"], Flag), (&["--break-on-reject"], Flag),
        (&["--break-per-input"], Flag), (&["--no-break-per-input"], Flag), (&["--skip-playlist-after-errors"], One),
        // Authentication
        (&["-u", "--username"], One), (&["-p", "--password"], One), (&["-2", "--twofactor"], One), (&["-n", "--netrc"], Flag),
        (&["--netrc-location"], One), (&["--netrc-cmd"], One), (&["--video-password"], One), (&["--ap-mso"], One),
        (&["--ap-username"], One), (&["--ap-password"], One), (&["--ap-list-mso"], Flag), (&["--client-certificate"], One),
        (&["--client-certificate-key"], One), (&["--client-certificate-password"], One),
        // Video format
        (&["-f", "--format"], One), (&["-S", "--format-sort"], One), (&["--format-sort-reset"], Flag),
        (&["--format-sort-force", "--S-force"], Flag), (&["--no-format-sort-force"], Flag), (&["--video-multistreams"], Flag),
        (&["--no-video-multistreams"], Flag), (&["--audio-multistreams"], Flag), (&["--no-audio-multistreams"], Flag),
        (&["--all-formats"], Flag), (&["--prefer-free-formats"], Flag), (&["--no-prefer-free-formats"], Flag),
        (&["--check-formats"], Flag), (&["--check-all-formats"], Flag), (&["--no-check-formats"], Flag),
        (&["-F", "--list-formats"], Flag), (&["--list-formats-as-table"], Flag),
        (&["--list-formats-old", "--no-list-formats-as-table"], Flag), (&["--merge-output-format"], One),
        (&["--allow-unplayable-formats"], Flag), (&["--no-allow-unplayable-formats"], Flag),
        // Subtitles
        (&["--write-subs", "--write-srt"], Flag), (&["--no-write-subs", "--no-write-srt"], Flag),
        (&["--write-auto-subs", "--write-automatic-subs"], Flag), (&["--no-write-auto-subs", "--no-write-automatic-subs"], Flag),
        (&["--all-subs"], Flag), (&["--list-subs"], Flag), (&["--sub-format"], One), (&["--sub-langs", "--srt-langs"], One),
        // Download
        (&["-N", "--concurrent-fragments"], One), (&["-r", "--limit-rate", "--rate-limit"], One), (&["--throttled-rate"], One),
        (&["-R", "--retries"], One), (&["--file-access-retries"], One), (&["--fragment-retries"], One), (&["--retry-sleep"], One),
        (&["--skip-unavailable-fragments", "--no-abort-on-unavailable-fragments"], Flag),
        (&["--abort-on-unavailable-fragments", "--no-skip-unavailable-fragments"], Flag), (&["--keep-fragments"], Flag),
        (&["--no-keep-fragments"], Flag), (&["--buffer-size"], One), (&["--resize-buffer"], Flag), (&["--no-resize-buffer"], Flag),
        (&["--http-chunk-size"], One), (&["--test"], Flag), (&["--playlist-reverse"], Flag), (&["--no-playlist-reverse"], Flag),
        (&["--playlist-random"], Flag), (&["--lazy-playlist"], Flag), (&["--no-lazy-playlist"], Flag),
        (&["--hls-prefer-native"], Flag), (&["--hls-prefer-ffmpeg"], Flag), (&["--hls-use-mpegts"], Flag),
        (&["--no-hls-use-mpegts"], Flag), (&["--download-sections"], One), (&["--downloader", "--external-downloader"], One),
        (&["--downloader-args", "--external-downloader-args"], One),
        // Workarounds
        (&["--encoding"], One), (&["--legacy-server-connect"], Flag), (&["--no-check-certificates"], Flag),
        (&["--prefer-insecure", "--prefer-unsecure"], Flag), (&["--user-agent"], One), (&["--referer"], One),
        (&["--add-headers"], One), (&["--bidi-workaround"], Flag), (&["--sleep-requests"], One),
        (&["--sleep-interval", "--min-sleep-interval"], One), (&["--max-sleep-interval"], One), (&["--sleep-subtitles"], One),
        // Verbosity and simulation
        (&["-q", "--quiet"], Flag), (&["--no-quiet"], Flag), (&["--no-warnings"], Flag), (&["-s", "--simulate"], Flag),
        (&["--no-simulate"], Flag), (&["--ignore-no-formats-error"], Flag), (&["--no-ignore-no-formats-error"], Flag),
        (&["--skip-download", "--no-download"], Flag), (&["-O", "--print"], One), (&["--print-to-file"], Two),
        (&["-g", "--get-url"], Flag), (&["-e", "--get-title"], Flag), (&["--get-id"], Flag), (&["--get-thumbnail"], Flag),
        (&["--get-description"], Flag), (&["--get-duration"], Flag), (&["--get-filename"], Flag), (&["--get-format"], Flag),
        (&["-j", "--dump-json"], Flag), (&["-J", "--dump-single-json"], Flag), (&["--print-json"], Flag),
        (&["--force-write-archive", "--force-write-download-archive", "--force-download-archive"], Flag), (&["--newline"], Flag),
        (&["--no-progress"], Flag), (&["--progress"], Flag), (&["--console-title"], Flag), (&["--progress-template"], One),
        (&["--progress-delta"], One), (&["-v", "--verbose"], Flag), (&["--dump-pages"], Flag), (&["--write-pages"], Flag),
        (&["--load-pages"], Flag), (&["--print-traffic"], Flag),
        // Filesystem
        (&["-a", "--batch-file"], One), (&["--no-batch-file"], Flag), (&["--id"], Flag), (&["-P", "--paths"], One),
        (&["-o", "--output"], One), (&["--output-na-placeholder"], One), (&["--autonumber-size"], One),
        (&["--autonumber-start"], One), (&["--restrict-filenames"], Flag), (&["--no-restrict-filenames"], Flag),
        (&["--windows-filenames"], Flag), (&["--no-windows-filenames"], Flag), (&["--trim-filenames", "--trim-file-names"], One),
        (&["-w", "--no-overwrites"], Flag), (&["--force-overwrites", "--yes-overwrites"], Flag), (&["--no-force-overwrites"], Flag),
        (&["-c", "--continue"], Flag), (&["--no-continue"], Flag), (&["--part"], Flag), (&["--no-part"], Flag), (&["--mtime"], Flag),
        (&["--no-mtime"], Flag), (&["--write-description"], Flag), (&["--no-write-description"], Flag),
        (&["--write-info-json"], Flag), (&["--no-write-info-json"], Flag), (&["--write-playlist-metafiles"], Flag),
        (&["--no-write-playlist-metafiles"], Flag), (&["--clean-info-json", "--clean-infojson"], Flag),
        (&["--no-clean-info-json", "--no-clean-infojson"], Flag), (&["--write-comments", "--get-comments"], Flag),
        (&["--no-write-comments", "--no-get-comments"], Flag), (&["--load-info-json"], One), (&["--cookies"], One),
        (&["--no-cookies"], Flag), (&["--cookies-from-browser"], One), (&["--no-cookies-from-browser"], Flag),
        (&["--cache-dir"], One), (&["--no-cache-dir"], Flag), (&["--rm-cache-dir"], Flag),
        // Thumbnail
        (&["--write-thumbnail"], Flag), (&["--no-write-thumbnail"], Flag), (&["--write-all-thumbnails"], Flag),
        (&["--list-thumbnails"], Flag),
        // Internet shortcut
        (&["--write-link"], Flag), (&["--write-url-link"], Flag), (&["--write-webloc-link"], Flag), (&["--write-desktop-link"], Flag),
        // Post-processing
        (&["-x", "--extract-audio"], Flag), (&["--audio-format"], One), (&["--audio-quality"], One), (&["--remux-video"], One),
        (&["--recode-video"], One), (&["--postprocessor-args", "--ppa"], One), (&["-k", "--keep-video"], Flag),
        (&["--no-keep-video"], Flag), (&["--post-overwrites"], Flag), (&["--no-post-overwrites"], Flag), (&["--embed-subs"], Flag),
        (&["--no-embed-subs"], Flag), (&["--embed-thumbnail"], Flag), (&["--no-embed-thumbnail"], Flag),
        (&["--embed-metadata", "--add-metadata"], Flag), (&["--no-embed-metadata", "--no-add-metadata"], Flag),
        (&["--embed-chapters", "--add-chapters"], Flag), (&["--no-embed-chapters", "--no-add-chapters"], Flag),
        (&["--embed-info-json"], Flag), (&["--no-embed-info-json"], Flag), (&["--metadata-from-title"], One),
        (&["--parse-metadata"], One), (&["--replace-in-metadata"], Three), (&["--xattrs", "--xattr"], Flag),
        (&["--concat-playlist"], One), (&["--fixup"], One), (&["--ffmpeg-location"], One), (&["--exec"], One),
        (&["--no-exec"], Flag), (&["--exec-before-download"], One), (&["--no-exec-before-download"], Flag),
        (&["--convert-subs", "--convert-sub", "--convert-subtitles"], One), (&["--convert-thumbnails"], One),
        (&["--split-chapters", "--split-tracks"], Flag), (&["--no-split-chapters", "--no-split-tracks"], Flag),
        (&["--remove-chapters"], One), (&["--no-remove-chapters"], Flag), (&["--force-keyframes-at-cuts"], Flag),
        (&["--no-force-keyframes-at-cuts"], Flag), (&["--use-postprocessor"], One),
        // SponsorBlock
        (&["--sponsorblock-mark"], One), (&["--sponsorblock-remove"], One), (&["--sponsorblock-chapter-title"], One),
        (&["--no-sponsorblock"], Flag), (&["--sponsorblock-api"], One),
        // Extractor
        (&["--extractor-retries"], One), (&["--allow-dynamic-mpd", "--no-ignore-dynamic-mpd"], Flag),
        (&["--ignore-dynamic-mpd", "--no-allow-dynamic-mpd"], Flag), (&["--hls-split-discontinuity"], Flag),
        (&["--no-hls-split-discontinuity"], Flag), (&["--extractor-args"], One),
        // Deprecated switches and arguments
        (&["--xattr-set-filesize"], Flag), (&["--dump-user-agent"], Flag), (&["--youtube-include-dash-manifest"], Flag),
        (&["--no-youtube-skip-dash-manifest"], Flag), (&["--youtube-skip-dash-manifest"], Flag),
        (&["--no-youtube-include-dash-manifest"], Flag), (&["--youtube-include-hls-manifest"], Flag),
        (&["--no-youtube-skip-hls-manifest"], Flag), (&["--youtube-skip-hls-manifest"], Flag),
        (&["--no-youtube-include-hls-manifest"], Flag), (&["--youtube-print-sig-code"], Flag), (&["--sponskrub"], Flag),
        (&["--no-sponskrub"], Flag), (&["--sponskrub-cut"], Flag), (&["--no-sponskrub-cut"], Flag), (&["--sponskrub-force"], Flag),
        (&["--no-sponskrub-force"], Flag), (&["--prefer-avconv"], Flag), (&["--no-prefer-ffmpeg"], Flag),
        (&["--prefer-ffmpeg"], Flag), (&["--no-prefer-avconv"], Flag), (&["-C"], Flag), (&["--call-home"], Flag),
        (&["--no-call-home"], Flag), (&["--include-ads"], Flag), (&["--no-include-ads"], Flag), (&["--write-annotations"], Flag),
        (&["--no-write-annotations"], Flag), (&["--sponskrub-location"], One), (&["--sponskrub-args"], One),
        (&["--cn-verification-proxy"], One),
    ]
};

/// Index into OPTION_TABLE of the option that owns this exact name.
fn option_index(name: &str) -> Option<usize> {
    OPTION_TABLE.iter().position(|(names, _)| names.contains(&name))
}

/// How many values the option with this exact name takes.
fn option_arity(name: &str) -> Option<Arity> {
    option_index(name).map(|i| OPTION_TABLE[i].1)
}

/// optparse's _match_abbrev for long options, with yt-dlp's _match_long_opt override:
/// a prefix whose completions all belong to one option resolves to the first of them.
fn match_long_opt(opt: &str) -> String {
    if option_index(opt).is_some() {
        return opt.to_string();
    }
    let mut possibilities: Vec<&str> =
        OPTION_TABLE.iter().flat_map(|(names, _)| names.iter().copied()).filter(|n| n.starts_with("--") && n.starts_with(opt)).collect();
    match possibilities.len() {
        0 => usage_error(&format!("no such option: {opt}")),
        1 => possibilities[0].to_string(),
        _ => {
            possibilities.sort();
            let first = option_index(possibilities[0]);
            if possibilities.iter().all(|p| option_index(p) == first) {
                return possibilities[0].to_string();
            }
            usage_error(&format!("ambiguous option: {opt} ({}?)", possibilities.join(", ")))
        }
    }
}

const POSTPROCESS_WHEN: &[&str] = &["pre_process", "after_filter", "video", "before_dl", "post_process", "after_move", "after_video", "playlist"];

/// `[WHEN:]VALUE` as _dict_from_options_callback with when_prefix.
fn split_when(value: &str) -> (String, String) {
    if let Some((k, v)) = value.split_once(':') {
        let kl = k.to_lowercase();
        if POSTPROCESS_WHEN.contains(&kl.as_str()) {
            return (kl, v.to_string());
        }
    }
    ("video".into(), value.to_string())
}

const PRESET_ALIASES: &[(&str, &[&str])] = &[
    ("mp3", &["-f", "ba[acodec^=mp3]/ba/b", "-x", "--audio-format", "mp3"]),
    ("aac", &["-f", "ba[acodec^=aac]/ba[acodec^=mp4a.40.]/ba/b", "-x", "--audio-format", "aac"]),
    ("mp4", &["--merge-output-format", "mp4", "--remux-video", "mp4", "-S", "vcodec:h264,lang,quality,res,fps,hdr:12,acodec:aac"]),
    ("mkv", &["--merge-output-format", "mkv", "--remux-video", "mkv"]),
    ("sleep", &["--sleep-subtitles", "5", "--sleep-requests", "0.75", "--sleep-interval", "10", "--max-sleep-interval", "20"]),
];

fn usage_error(msg: &str) -> ! {
    // main() prints OptParseError as f'\n{e}'
    eprint!("\n{USAGE}\n\nyt-dlp: error: {msg}\n");
    exit(2);
}

fn parse_args(args: &[String]) -> Opts {
    let mut o = Opts::default();
    let mut args: Vec<String> = args.to_vec();
    let mut i = 0;
    let mut only_positional = false;
    while i < args.len() {
        let a = &args[i];
        i += 1;
        if only_positional || a == "-" || !a.starts_with('-') {
            o.urls.push(a.clone());
            continue;
        }
        if a == "--" {
            only_positional = true;
            continue;
        }
        let (name, inline): (String, Option<String>) = if a.starts_with("--") {
            match a.split_once('=') {
                Some((n, v)) => (match_long_opt(n), Some(v.to_string())),
                None => (match_long_opt(a), None),
            }
        } else if a.chars().count() > 2 {
            let n: String = a.chars().take(2).collect();
            match option_arity(&n) {
                Some(Arity::Flag) | None => (a.clone(), None),
                Some(_) => (n.clone(), Some(a[n.len()..].to_string())),
            }
        } else {
            (a.clone(), None)
        };
        let Some(arity) = option_arity(&name) else {
            usage_error(&format!("no such option: {name}"));
        };
        let mut vals: Vec<String> = Vec::new();
        let need = arity.count();
        if let Some(v) = inline {
            if arity == Arity::Flag {
                usage_error(&format!("{name} option does not take a value"));
            }
            vals.push(v);
        }
        while vals.len() < need {
            if i >= args.len() {
                if need == 1 {
                    usage_error(&format!("{name} option requires 1 argument"));
                }
                usage_error(&format!("{name} option requires {need} arguments"));
            }
            vals.push(args[i].clone());
            i += 1;
        }
        let v = vals.first().cloned().unwrap_or_default();
        match name.as_str() {
            "-t" | "--preset-alias" => {
                if v.is_empty() {
                    continue;
                }
                match PRESET_ALIASES.iter().find(|(k, _)| *k == v) {
                    Some((_, expansion)) => {
                        args.splice(i..i, expansion.iter().map(|s| s.to_string()));
                    }
                    None => usage_error(&format!("option {name}: Unknown preset alias: {v}")),
                }
            }
            "--load-info-json" => o.load_info_json = Some(v),
            "--list-extractors" => o.list_extractors = true,
            "--extractor-descriptions" => o.list_extractor_descriptions = true,
            "--ap-list-mso" => o.ap_list_mso = true,
            "--user-agent" => o.user_agent = Some(v),
            "--referer" => o.referer = Some(v),
            "--add-headers" => {
                // _dict_from_options_callback: `(?is)(?P<keys>[\w-]+):(?P<val>.*)$`, keys lowercased
                let key_len = v.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')).unwrap_or(v.len());
                if key_len == 0 || !v[key_len..].starts_with(':') {
                    usage_error(&format!("wrong {name} formatting; it should be FIELD:VALUE, not \"{v}\""));
                }
                o.headers.set(&v[..key_len].to_lowercase(), Value::str(&v[key_len + 1..]));
            }
            "--cache-dir" => o.cachedir = Some(Some(v)),
            "--no-cache-dir" => o.cachedir = Some(None),
            "--rm-cache-dir" => o.rm_cachedir = true,
            "-f" | "--format" => o.format = Some(v),
            "-S" | "--format-sort" => o.format_sort.extend(v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty())),
            "--format-sort-force" | "--S-force" => o.format_sort_force = true,
            "--no-format-sort-force" => o.format_sort_force = false,
            "--merge-output-format" => o.merge_output_format = Some(v),
            "--prefer-free-formats" => o.prefer_free_formats = true,
            "--no-prefer-free-formats" => o.prefer_free_formats = false,
            "--video-multistreams" => o.allow_multiple_video = true,
            "--no-video-multistreams" => o.allow_multiple_video = false,
            "--audio-multistreams" => o.allow_multiple_audio = true,
            "--no-audio-multistreams" => o.allow_multiple_audio = false,
            "--print" | "-O" => o.forceprint.push(split_when(&v)),
            "--print-to-file" => {
                let (when, tmpl) = split_when(&v);
                o.print_to_file.push((when, tmpl, vals[1].clone()));
            }
            "--match-filters" | "--match-filter" => o.match_filter.push(v),
            "--no-match-filters" => o.match_filter.clear(),
            "-o" | "--output" => {
                let (k, t) = match v.split_once(':') {
                    Some((k, t)) if OUTTMPL_TYPES.contains(&k.to_lowercase().as_str()) => (k.to_lowercase(), t.to_string()),
                    _ => ("default".into(), v.clone()),
                };
                if k == "default" {
                    o.outtmpl = Some(t);
                }
            }
            "-P" | "--paths" => {
                let (k, p) = match v.split_once(':') {
                    Some((k, p)) if k.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') && !k.is_empty() => {
                        (k.to_lowercase(), p.to_string())
                    }
                    _ => ("home".into(), v.clone()),
                };
                o.paths.insert(k, p);
            }
            "--restrict-filenames" => o.restrict_filenames = true,
            "--no-restrict-filenames" => o.restrict_filenames = false,
            "--windows-filenames" => o.windows_filenames = Some(true),
            "--no-windows-filenames" => o.windows_filenames = Some(false),
            "--trim-filenames" | "--trim-file-names" => match v.parse() {
                Ok(n) => o.trim_filenames = n,
                Err(_) => usage_error(&format!("option {name}: invalid integer value: {}", pyfmt::str_repr(&v))),
            },
            "--output-na-placeholder" => o.na_placeholder = Some(v),
            "--autonumber-size" => o.autonumber_size = v.parse().ok(),
            "--autonumber-start" => o.autonumber_start = v.parse().ok(),
            "-s" | "--simulate" => o.simulate = Some(true),
            "--no-simulate" => o.simulate = Some(false),
            "-q" | "--quiet" => o.quiet = Some(true),
            "--no-quiet" => o.quiet = Some(false),
            "--no-warnings" => o.no_warnings = true,
            "-v" | "--verbose" => o.verbose = true,
            "-e" | "--get-title" => o.gettitle = true,
            "--get-id" => o.getid = true,
            "-g" | "--get-url" => o.geturl = true,
            "--get-thumbnail" => o.getthumbnail = true,
            "--get-description" => o.getdescription = true,
            "--get-duration" => o.getduration = true,
            "--get-filename" => o.getfilename = true,
            "--get-format" => o.getformat = true,
            "-j" | "--dump-json" => o.dumpjson = true,
            "-J" | "--dump-single-json" => o.dump_single_json = true,
            "--ignore-no-formats-error" => o.ignore_no_formats_error = true,
            "--no-ignore-no-formats-error" => o.ignore_no_formats_error = false,
            "--id" => o.useid = true,
            "--list-thumbnails" => o.list_thumbnails = true,
            "-F" | "--list-formats" => o.listformats = true,
            "--list-formats-as-table" => o.listformats_old = false,
            "--list-formats-old" | "--no-list-formats-as-table" => o.listformats_old = true,
            "--allow-unplayable-formats" => o.allow_unplayable_formats = true,
            "--no-allow-unplayable-formats" => o.allow_unplayable_formats = false,
            "--list-impersonate-targets" => o.list_impersonate_targets = true,
            n if DEPRECATED_OPTIONS.contains(&n) => o.deprecated_options.push(name.clone()),
            "--version" => {
                // optparse's version action prints and exits as soon as it is parsed
                println!("{VERSION}");
                exit(0);
            }
            "-h" | "--help" => {
                // optparse's help action likewise prints the formatted help and exits right away
                let mut stdout = std::io::stdout();
                let _ = stdout.write_all(HELP.as_bytes());
                let _ = stdout.flush();
                exit(0);
            }
            _ => {}
        }
    }
    o
}

/// options.py's deprecated_switches and deprecated_arguments: parsed, recorded, otherwise ignored.
const DEPRECATED_OPTIONS: &[&str] = &[
    "--xattr-set-filesize", "--dump-user-agent", "--youtube-include-dash-manifest", "--no-youtube-skip-dash-manifest",
    "--youtube-skip-dash-manifest", "--no-youtube-include-dash-manifest", "--youtube-include-hls-manifest",
    "--no-youtube-skip-hls-manifest", "--youtube-skip-hls-manifest", "--no-youtube-include-hls-manifest",
    "--youtube-print-sig-code", "--sponskrub", "--no-sponskrub", "--sponskrub-cut", "--no-sponskrub-cut", "--sponskrub-force",
    "--no-sponskrub-force", "--prefer-avconv", "--no-prefer-ffmpeg", "--prefer-ffmpeg", "--no-prefer-avconv", "-C", "--call-home",
    "--no-call-home", "--include-ads", "--no-include-ads", "--write-annotations", "--no-write-annotations", "--sponskrub-location",
    "--sponskrub-args", "--cn-verification-proxy",
];

const OUTTMPL_TYPES: &[&str] = &[
    "subtitle", "thumbnail", "description", "annotation", "infojson", "link", "pl_thumbnail", "pl_description", "pl_infojson", "chapter",
    "pl_video",
];

// ---------------------------------------------------------------- the downloader

struct Ydl {
    o: Opts,
    quiet: bool,
    simulate: bool,
    /// simulate == 'list_only'
    list_only: bool,
    retcode: i32,
    num_downloads: i64,
    num_videos: i64,
    out: std::io::Stdout,
    /// `_printed_messages`, for only_once output
    printed_messages: std::collections::HashSet<String>,
    /// `params['http_headers']`: std_headers updated with the user's headers.
    http_headers: Map,
}

impl Ydl {
    fn to_stdout(&mut self, msg: &str) {
        let _ = writeln!(self.out, "{msg}");
    }
    fn to_screen(&mut self, msg: &str) {
        if self.quiet && !self.o.verbose {
            return;
        }
        // Screen output goes to stdout unless quiet (then stderr, but quiet suppresses it).
        let _ = writeln!(self.out, "{msg}");
    }
    fn report_warning(&mut self, msg: &str) {
        if self.o.no_warnings {
            if self.o.verbose {
                eprintln!("[debug] WARNING: {msg}");
            }
        } else {
            let _ = self.out.flush();
            eprintln!("WARNING: {msg}");
        }
    }
    fn report_error(&mut self, msg: &str) {
        let _ = self.out.flush();
        eprintln!("ERROR: {msg}");
        self.retcode = 1;
    }

    fn tmpl_ctx(&self, sanitize: bool) -> outtmpl::Ctx<'_> {
        outtmpl::Ctx {
            na: self.o.na_placeholder.as_deref().unwrap_or("NA"),
            sanitize: sanitize.then_some(outtmpl::Sanitize {
                restricted: self.o.restrict_filenames,
                slash_only: !self.o.restrict_filenames && self.o.windows_filenames == Some(false),
            }),
            autonumber_size: self.o.autonumber_size.unwrap_or(5),
        }
    }

    fn with_counters(&self, info: &Map) -> Map {
        let mut d = info.clone();
        d.set("autonumber", Value::Int(self.o.autonumber_start.unwrap_or(1) - 1 + self.num_downloads));
        d.set("video_autonumber", Value::Int(self.num_videos));
        d
    }

    fn evaluate(&mut self, tmpl: &str, info: &Map) -> String {
        let d = self.with_counters(info);
        match outtmpl::evaluate(tmpl, &d, &self.tmpl_ctx(false)) {
            Ok(s) => s,
            Err(e) => {
                self.report_error(&format!("Error in output template: {e} (encoding: 'utf-8')"));
                String::new()
            }
        }
    }

    fn prepare_filename(&mut self, info: &Map) -> Option<String> {
        let tmpl = self.o.outtmpl.clone().unwrap_or_else(|| {
            if self.o.restrict_filenames { DEFAULT_OUTTMPL.replace(" - ", " ").replace(' ', "-") } else { DEFAULT_OUTTMPL.into() }
        });
        let d = self.with_counters(info);
        let mut filename = match outtmpl::evaluate(&expand_path(&tmpl), &d, &self.tmpl_ctx(true)) {
            Ok(s) => s,
            Err(e) => {
                self.report_error(&format!("Error in output template: {e} (encoding: 'utf-8')"));
                return None;
            }
        };
        if filename.is_empty() {
            return None;
        }
        if self.o.trim_filenames > 0 {
            let parts: Vec<&str> = filename.rsplitn(3, '.').collect();
            let mut parts: Vec<String> = parts.into_iter().rev().map(String::from).collect();
            parts[0] = parts[0].chars().take(self.o.trim_filenames).collect();
            filename = parts.into_iter().filter(|p| !p.is_empty()).collect::<Vec<_>>().join(".");
        }
        if filename == "-" {
            return Some(filename);
        }
        Some(self.get_output_path("", &filename))
    }

    fn get_output_path(&self, dir_type: &str, filename: &str) -> String {
        let home = expand_path(self.o.paths.get("home").map(|s| s.trim()).unwrap_or(""));
        let sub = if dir_type.is_empty() { String::new() } else { expand_path(self.o.paths.get(dir_type).map(|s| s.trim()).unwrap_or("")) };
        path_join(&[&home, &sub, filename])
    }

    // ------------------------------------------------------------ processing

    fn process_video_result(&mut self, mut info: Map) -> Result<(), String> {
        self.num_videos += 1;
        let id_ok = info.g("id").truthy();
        if !info.contains("id") {
            return Err("Missing \"id\" field in extractor result".into());
        } else if !id_ok {
            return Err("Extractor failed to obtain \"id\"".into());
        }
        if let Some(v) = info.get("id").filter(|v| !v.is_null() && !matches!(v, Value::Str(_))).cloned() {
            self.report_warning("\"id\" field is not a string - forcing string conversion, there is an error in extractor");
            info.set("id", Value::Str(py_str(&v)));
        }
        if info.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0) <= 0.0 && info.get("duration").is_some_and(|v| v.truthy()) {
            info.remove("duration");
            self.report_warning("\"duration\" field is negative, there is an error in extractor");
        } else if info.get("duration").is_some_and(|v| !v.truthy() && !v.is_null()) {
            info.remove("duration");
        }
        if !info.contains("playlist") {
            info.set("playlist", Value::Null);
            info.set("playlist_index", Value::Null);
        }
        sanitize_thumbnails(&mut info);
        if let Some(t) = info.get("thumbnail").filter(|t| t.truthy()).cloned() {
            info.set("thumbnail", t);
        } else if let Some(Value::List(ts)) = info.get("thumbnails").filter(|t| t.truthy()).cloned() {
            if let Some(url) = ts.last().and_then(|t| t.as_dict()).map(|t| t.g("url").clone()) {
                info.set("thumbnail", url);
            }
        }
        if info.g("display_id").is_null() && info.contains("id") {
            let id = info.g("id").clone();
            info.set("display_id", id);
        }
        self.fill_common_fields(&mut info);
        info.set("requested_subtitles", Value::Null);

        let mut formats: Vec<Map> = match info.get("formats") {
            Some(Value::List(l)) => l.iter().filter_map(|f| f.as_dict().cloned()).collect(),
            _ => vec![],
        };
        let has_drm = formats.iter().any(|f| f.g("has_drm").truthy() && f.str_of("has_drm") != Some("maybe"));
        info.set("_has_drm", if has_drm { Value::Bool(true) } else { Value::Null });
        formats.retain(|f| !f.g("has_drm").truthy() || f.str_of("has_drm") == Some("maybe"));
        if !formats.is_empty() && formats.iter().all(|f| f.str_of("acodec") == Some("none") && f.str_of("vcodec") == Some("none")) {
            let pre = if has_drm { "This video is DRM protected and " } else { "" };
            let msg = format!("{pre}only images are available for download. Use --list-formats to see them");
            let mut c = msg.chars();
            let cap = c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default();
            self.report_warning(&cap);
        }
        let mut wellformed = Vec::new();
        for f in formats {
            if !f.g("url").truthy() {
                self.report_warning("\"url\" field is missing or empty - skipping format, there is an error in extractor");
                continue;
            }
            wellformed.push(f);
        }
        let mut formats = wellformed;
        if formats.is_empty() {
            let msg = if has_drm { "This video is DRM protected" } else { "No video formats found!" };
            if !self.o.ignore_no_formats_error {
                return Err(msg.into());
            }
            self.report_warning(msg);
        }
        let duration = info.g("duration").clone();
        for f in formats.iter_mut() {
            formats::fill_sorting_fields(f);
            let ext = py_str(f.g("ext"));
            if ["aac", "opus", "mp3", "flac", "vorbis"].contains(&ext.as_str()) && f.g("acodec").is_null() {
                f.set("acodec", Value::Str(ext.clone()));
            }
            if f.g("resolution").is_null() {
                let r = util::format_resolution(f, None);
                f.set("resolution", r.map(Value::Str).unwrap_or(Value::Null));
            }
            if f.g("dynamic_range").is_null() && f.str_of("vcodec") != Some("none") {
                f.set("dynamic_range", Value::str("SDR"));
            }
            if f.g("aspect_ratio").is_null() {
                let ar = match (f.g("width").as_f64(), f.g("height").as_f64()) {
                    (Some(w), Some(h)) if h != 0.0 => Value::Float(round2(w / h)),
                    _ => Value::Null,
                };
                f.set("aspect_ratio", ar);
            }
            if !f.contains("manifest_url") || f.g("manifest_url").is_null() {
                if !f.g("filesize").truthy() && !f.g("filesize_approx").truthy() {
                    let v = match (f.g("tbr").as_f64(), duration.as_f64()) {
                        (Some(t), Some(d)) => Value::Int((d * t * (1000.0 / 8.0)).trunc() as i64),
                        _ => Value::Null,
                    };
                    f.set("filesize_approx", v);
                }
            }
            let headers = self.calc_headers(f, &info);
            f.set("http_headers", Value::Dict(headers));
        }

        // Safeguard against old/insecure infojson when using --load-info-json
        if info.g("http_headers").truthy() {
            let mut h = Map::new();
            header_update(&mut h, info.g("http_headers"));
            h.remove("Cookie");
            info.set("http_headers", Value::Dict(h));
        }
        info.remove("__x_forwarded_for_ip");

        let extractor_sort: Vec<String> = match info.get("_format_sort_fields") {
            Some(Value::List(l)) => l.iter().map(py_str).collect(),
            _ => vec![],
        };
        let mut sorter = match FormatSorter::new(&self.o.format_sort, &extractor_sort, self.o.prefer_free_formats, self.o.format_sort_force) {
            Ok(s) => s,
            Err(e) => return Err(e),
        };
        for w in std::mem::take(&mut sorter.warnings) {
            self.deprecated_feature(&w);
        }
        sorter.sort(&mut formats);

        // Sanitize and group by format_id
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, f) in formats.iter_mut().enumerate() {
            let id = if !f.g("format_id").truthy() {
                i.to_string()
            } else {
                py_str(f.g("format_id")).chars().map(|c| if c.is_whitespace() || ",/+[]()".contains(c) { '_' } else { c }).collect()
            };
            f.set("format_id", Value::Str(id.clone()));
            match groups.iter_mut().find(|(k, _)| *k == id) {
                Some((_, v)) => v.push(i),
                None => groups.push((id, vec![i])),
            }
        }
        let common_exts: Vec<&str> = util::COMMON_AUDIO.iter().chain(util::COMMON_VIDEO).chain(&["3gp"]).chain(util::STORYBOARDS).copied().collect();
        for (id, idxs) in &groups {
            let ambiguous = idxs.len() > 1;
            for (n, &i) in idxs.iter().enumerate() {
                let f = &mut formats[i];
                if ambiguous {
                    f.set("format_id", Value::Str(format!("{id}-{n}")));
                }
                let fid = py_str(f.g("format_id"));
                if Some(fid.as_str()) != f.str_of("ext") && common_exts.contains(&fid.as_str()) {
                    f.set("format_id", Value::Str(format!("f{fid}")));
                }
                if f.g("format").is_null() {
                    let note = match f.g("format_note") {
                        Value::Null => String::new(),
                        v => format!(" ({})", py_str(v)),
                    };
                    let s = format!("{} - {}{}", py_str(f.g("format_id")), util::format_resolution(f, Some("unknown")).unwrap(), note);
                    f.set("format", Value::Str(s));
                }
            }
        }
        info.set("formats", Value::List(formats.iter().cloned().map(Value::Dict).collect()));

        if let Some(reason) = self.match_entry(&info, filters::Incomplete::Keys(filters::FORMAT_FIELDS))? {
            let _ = reason;
            return Ok(());
        }

        if self.o.list_thumbnails {
            self.list_thumbnails(&info);
        }
        if self.o.listformats {
            self.list_formats(&info);
        }
        if self.list_only {
            // Without this printing, -F --print-json will not work
            self.forced_printings(&mut info, None, true);
            return Ok(());
        }

        let spec = match &self.o.format {
            Some(f) => f.clone(),
            None => self.default_format_spec(&info),
        };
        let sels = formats::parse_format_spec(&spec)?;
        let sopts = SelectOpts {
            allow_multiple_video: self.o.allow_multiple_video,
            allow_multiple_audio: self.o.allow_multiple_audio,
            merge_output_format: self.o.merge_output_format.clone(),
            prefer_free_formats: self.o.prefer_free_formats,
        };
        let mut to_download = formats::select_formats(&formats, &sels, &sopts);
        if to_download.is_empty() {
            if !self.o.ignore_no_formats_error {
                return Err(format!(
                    "[{}] {}: Requested format is not available. Use --list-formats for a list of available formats",
                    py_str(info.g("extractor")),
                    py_str(info.g("id"))
                ));
            }
            self.report_warning("Requested format is not available");
            to_download = vec![Map::new()];
        }
        // download=True and the default requested_ranges ({},)
        if to_download.last().is_some_and(|f| !f.0.is_empty()) {
            let ids: Vec<String> = to_download.iter().map(|f| py_str(f.g("format_id"))).collect();
            let msg = format!("[info] {}: Downloading {} format(s): {}", py_str(info.g("id")), to_download.len(), ids.join(", "));
            self.to_screen(&msg);
        }
        for fmt in &to_download {
            let mut new_info = info.clone();
            new_info.update(fmt);
            self.process_info(new_info)?;
        }
        Ok(())
    }

    fn default_format_spec(&self, _info: &Map) -> String {
        let prefer_best = self.o.outtmpl.as_deref() == Some("-") || !ffmpeg_available();
        if prefer_best {
            "best/bestvideo+bestaudio".into()
        } else if self.o.allow_multiple_audio {
            "bestvideo+bestaudio/best".into()
        } else {
            "bestvideo*+bestaudio/best".into()
        }
    }

    fn list_thumbnails(&mut self, info: &Map) {
        let id = py_str(info.g("id"));
        let thumbs: Vec<Map> = match info.get("thumbnails") {
            Some(Value::List(l)) => l.iter().filter_map(|t| t.as_dict().cloned()).collect(),
            _ => vec![],
        };
        if thumbs.is_empty() {
            self.to_screen(&format!("{id} has no thumbnails"));
            return;
        }
        let or_unknown = |v: &Value| if v.truthy() { py_str(v) } else { "unknown".into() };
        let rows: Vec<Vec<String>> =
            thumbs.iter().map(|t| vec![py_str(t.g("id")), or_unknown(t.g("width")), or_unknown(t.g("height")), py_str(t.g("url"))]).collect();
        let header: Vec<String> = ["ID", "Width", "Height", "URL"].iter().map(|s| s.to_string()).collect();
        self.to_screen(&format!("[info] Available thumbnails for {id}:"));
        let table = util::render_table(&header, &rows, None, 0, false);
        self.to_stdout(&table);
    }

    fn deprecated_feature(&mut self, msg: &str) {
        // to_stderr(..., only_once=True)
        let line = format!("Deprecated Feature: {msg}");
        if self.printed_messages.insert(line.clone()) {
            let _ = self.out.flush();
            eprintln!("{line}");
        }
    }

    fn write_debug(&mut self, msg: &str) {
        if self.o.verbose {
            let _ = self.out.flush();
            eprintln!("[debug] {msg}");
        }
    }

    /// `list_formats` through `__list_table`.
    fn list_formats(&mut self, info: &Map) {
        let id = py_str(info.g("id"));
        match self.render_formats_table(info) {
            None => self.to_screen(&format!("{id} has no formats")),
            Some(table) => {
                self.to_screen(&format!("[info] Available formats for {id}:"));
                self.to_stdout(&table);
            }
        }
    }

    /// `render_formats_table` without colours: every `_format_out` styling is plain text and the
    /// box-drawing delimiters fall back to `|` and `-`.
    fn render_formats_table(&self, info: &Map) -> Option<String> {
        let formats: Vec<Map> = match info.get("formats") {
            None | Some(Value::Null) => {
                if info.g("url").truthy() && info.str_of("_type").unwrap_or("video") == "video" {
                    vec![info.clone()]
                } else {
                    vec![]
                }
            }
            Some(Value::List(l)) => l.iter().filter_map(|f| f.as_dict().cloned()).collect(),
            Some(_) => vec![],
        };
        if formats.is_empty() {
            return None;
        }
        let shown = |f: &&Map| f.g("preference").as_f64().is_none_or(|p| p >= -1000.0);
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<String>>();
        if self.o.listformats_old {
            let table: Vec<Vec<String>> = formats
                .iter()
                .filter(shown)
                .map(|f| {
                    vec![
                        format_field(f.g("format_id"), "", None),
                        format_field(f.g("ext"), "", None),
                        util::format_resolution(f, Some("unknown")).unwrap_or_default(),
                        format_note(f),
                    ]
                })
                .collect();
            return Some(util::render_table(&s(&["format code", "extension", "resolution", "note"]), &table, None, 1, false));
        }
        let simplified_codec = |f: &Map, field: &str| -> String {
            let codec = f.g(field);
            if !codec.truthy() {
                return "unknown".into();
            }
            let codec = py_str(codec);
            if codec != "none" {
                return codec.split('.').take(4).collect::<Vec<_>>().join(".");
            }
            if field == "vcodec" && f.str_of("acodec") == Some("none") {
                "images".into()
            } else if field == "acodec" && f.str_of("vcodec") == Some("none") {
                String::new()
            } else if field == "vcodec" {
                "audio only".into()
            } else {
                "video only".into()
            }
        };
        let rounded = |v: &Value, tmpl: &str| -> String {
            match v.as_f64() {
                Some(x) if v.truthy() => tmpl.replace("{}", &(util::py_round(x) as i64).to_string()),
                _ => String::new(),
            }
        };
        let bytes = |v: &Value| util::format_decimal_suffix(v, ".2", 1024.0).map(|s| s + "B").unwrap_or_else(|| "N/A".into());
        let delim = "|".to_string();
        let duration = info.g("duration");
        let table: Vec<Vec<String>> = formats
            .iter()
            .filter(shown)
            .map(|f| {
                // format_field(f, func=format_resolution, ignore=('audio only', 'images')) compares the
                // format dict itself against `ignore`, so nothing is ever ignored.
                let resolution = util::format_resolution(f, Some("unknown")).unwrap_or_default();
                let dynamic_range = match f.g("dynamic_range") {
                    Value::Null => String::new(),
                    v if py_str(v) == "SDR" => String::new(),
                    v => py_str(v).replace("HDR", ""),
                };
                let filesize = if f.g("filesize").truthy() {
                    format!(" \t{}", bytes(f.g("filesize")))
                } else if f.g("filesize_approx").truthy() {
                    format!("≈\t{}", bytes(f.g("filesize_approx")))
                } else {
                    match (f.g("tbr").as_f64(), duration.as_f64()) {
                        (Some(t), Some(d)) if (d * t * (1000.0 / 8.0)).trunc() != 0.0 => {
                            format!("~\t{}", bytes(&Value::Int((d * t * (1000.0 / 8.0)).trunc() as i64)))
                        }
                        _ => String::new(),
                    }
                };
                let drm = match f.g("has_drm") {
                    v if v.as_str() == Some("maybe") => "Maybe DRM",
                    v if v.truthy() => "DRM",
                    _ => "",
                };
                let container = match f.g("container") {
                    Value::Null => String::new(),
                    v if *v == *f.g("ext") => String::new(),
                    v => format_field(v, "", None),
                };
                let more = join_nonempty(
                    &[
                        if ["f4f", "f4m"].contains(&f.str_of("ext").unwrap_or("")) { "UNSUPPORTED".into() } else { String::new() },
                        drm.into(),
                        if f.g("__needs_testing").truthy() { "Untested".into() } else { String::new() },
                        format_field(f.g("format_note"), "", None),
                        container,
                    ],
                    ", ",
                );
                vec![
                    format_field(f.g("format_id"), "", None),
                    format_field(f.g("ext"), "", None),
                    resolution,
                    rounded(f.g("fps"), "\t{}"),
                    dynamic_range,
                    format_field(f.g("audio_channels"), "\t", None),
                    delim.clone(),
                    filesize,
                    rounded(f.g("tbr"), "\t{}k"),
                    shorten_protocol_name(f.str_of("protocol").unwrap_or("")).to_string(),
                    delim.clone(),
                    simplified_codec(f, "vcodec"),
                    rounded(f.g("vbr"), "\t{}k"),
                    simplified_codec(f, "acodec"),
                    rounded(f.g("abr"), "\t{}k"),
                    if f.g("asr").truthy() {
                        util::format_decimal_suffix(f.g("asr"), "", 1000.0).map(|s| format!("\t{s}")).unwrap_or_default()
                    } else {
                        String::new()
                    },
                    join_nonempty(&[format_field(f.g("language"), "[", Some("]")), more], " "),
                ]
            })
            .collect();
        let header = s(&[
            "ID", "EXT", "RESOLUTION", "\tFPS", "HDR", "CH", "|", "\tFILESIZE", "\tTBR", "PROTO", "|", "VCODEC", "\tVBR", "ACODEC", "\tABR",
            "\tASR", "MORE INFO",
        ]);
        Some(util::render_table(&header, &table, Some("-"), 0, true))
    }

    fn fill_common_fields(&mut self, info: &mut Map) {
        let title = info.g("title").clone();
        info.set("fulltitle", title.clone());
        if !title.truthy() {
            if title != Value::str("") {
                self.report_warning("Extractor failed to obtain \"title\". Creating a generic title instead");
            }
            let t = format!("{} video #{}", py_str(info.g("extractor")).replace(':', "-"), py_str(info.g("id")));
            info.set("title", Value::Str(t));
        }
        if let Some(d) = info.get("duration").filter(|d| !d.is_null()).and_then(|d| d.as_f64()) {
            info.set("duration_string", Value::Str(util::format_seconds(d, ":")));
        }
        for (ts, date) in [("timestamp", "upload_date"), ("release_timestamp", "release_date"), ("modified_timestamp", "modified_date")] {
            if info.g(date).is_null() && !info.g(ts).is_null() {
                let v = util::strftime_or_none(info.g(ts), "%Y%m%d").map(Value::Str).unwrap_or(Value::Null);
                info.set(date, v);
            }
        }
        if !info.g("release_year").truthy() {
            let ry = info
                .str_of("release_date")
                .and_then(|d| d.chars().take(4).collect::<String>().trim().parse::<i64>().ok())
                .map(Value::Int)
                .unwrap_or(Value::Null);
            info.set("release_year", ry);
        }
        let mut live_status = info.g("live_status").as_str().map(String::from);
        if live_status.is_none() {
            for key in ["is_live", "was_live"] {
                if info.g(key) == &Value::Bool(false) {
                    continue;
                }
                if info.g(key).truthy() {
                    live_status = Some(key.into());
                }
                break;
            }
            if ["is_live", "was_live"].iter().all(|k| info.g(k) == &Value::Bool(false)) {
                live_status = Some("not_live".into());
            }
        }
        if let Some(ls) = &live_status {
            info.set("live_status", Value::Str(ls.clone()));
            for key in ["is_live", "was_live"] {
                if info.g(key).is_null() {
                    info.set(key, Value::Bool(ls == key));
                }
            }
            if ls == "post_live" {
                info.set("was_live", Value::Bool(true));
            }
        }
        for field in ["chapter", "season", "episode"] {
            let num = info.g(&format!("{field}_number")).clone();
            if !num.is_null() && !info.g(field).truthy() {
                let mut c = field.chars();
                let cap = c.next().unwrap().to_uppercase().collect::<String>() + c.as_str();
                info.set(field, Value::Str(format!("{cap} {}", util::int_or_none(&num).unwrap_or(0))));
            }
        }
        for (old, new) in [("album_artist", "album_artists"), ("artist", "artists"), ("composer", "composers"), ("creator", "creators"), ("genre", "genres")] {
            if info.contains(new) && info.contains(old) {
                continue;
            }
            if let Some(Value::Str(s)) = info.get(old).filter(|v| v.truthy()).cloned() {
                info.set(new, Value::List(s.split(", ").map(Value::str).collect()));
            } else if let Some(Value::List(l)) = info.get(new).filter(|v| v.truthy()).cloned() {
                let joined = l.iter().map(|v| py_str(v).replace(',', "\u{FF0C}")).collect::<Vec<_>>().join(", ");
                info.set(old, Value::Str(joined));
            }
        }
    }

    /// Returns the rejection reason, if any.
    fn match_entry(&mut self, info: &Map, incomplete: filters::Incomplete) -> Result<Option<String>, String> {
        let reason = filters::match_filters(&self.o.match_filter, info, incomplete)?;
        if let Some(r) = &reason {
            self.to_screen(&format!("[download] {r}"));
        }
        Ok(reason)
    }

    fn process_info(&mut self, mut info: Map) -> Result<(), String> {
        if !info.contains("format") && info.contains("ext") {
            let e = info.g("ext").clone();
            info.set("format", e);
        }
        if self.match_entry(&info, filters::Incomplete::All(false))?.is_some() {
            return Ok(());
        }
        self.num_downloads += 1;
        let full_filename = self.prepare_filename(&info);
        info.set("_filename", full_filename.clone().map(Value::Str).unwrap_or(Value::Null));
        let incomplete = !info.contains("format");
        self.forced_printings(&mut info, full_filename, incomplete);
        if self.simulate {
            return Ok(());
        }
        Err("downloading is not supported".into())
    }

    fn forced_printings(&mut self, info: &mut Map, filename: Option<String>, incomplete: bool) {
        if let Some(f) = filename {
            info.set("filename", Value::Str(f));
        }
        let mut copy = info.clone();
        if !copy.contains("filename") {
            let f = self.prepare_filename(info);
            copy.set("filename", f.map(Value::Str).unwrap_or(Value::Null));
        }
        if let Some(Value::List(rf)) = info.get("requested_formats").filter(|v| !v.is_null()) {
            let urls: Vec<String> =
                rf.iter().filter_map(|f| f.as_dict()).map(|f| py_str(f.g("url")) + &f.str_of("play_path").unwrap_or("").to_string()).collect();
            copy.set("urls", Value::Str(urls.join("\n")));
        } else if info.g("url").truthy() {
            copy.set("urls", Value::Str(py_str(info.g("url")) + info.str_of("play_path").unwrap_or("")));
        }
        let prints: Vec<String> = self.o.forceprint.iter().filter(|(w, _)| w == "video").map(|(_, t)| t.clone()).collect();
        for t in prints {
            let s = self.evaluate(&format_tmpl(&t), &copy);
            self.to_stdout(&s);
        }
        let to_file: Vec<(String, String)> =
            self.o.print_to_file.iter().filter(|(w, _, _)| w == "video").map(|(_, t, f)| (t.clone(), f.clone())).collect();
        for (t, file_tmpl) in to_file {
            let d = self.with_counters(info);
            let filename = match outtmpl::evaluate(&expand_path(&file_tmpl), &d, &self.tmpl_ctx(true)) {
                Ok(f) => self.get_output_path("", &f),
                Err(_) => continue,
            };
            let tmpl = format_tmpl(&t);
            self.to_screen(&format!("[info] Writing {} to: {filename}", pyfmt::str_repr(&tmpl)));
            let text = self.evaluate(&tmpl, &copy);
            if let Some(dir) = std::path::Path::new(&filename).parent().filter(|p| !p.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut fh) = std::fs::OpenOptions::new().append(true).create(true).open(&filename) {
                let _ = writeln!(fh, "{text}");
            }
        }

        let print_field = |ydl: &mut Ydl, enabled: bool, field: &str, actual: &str, optional: bool| {
            if enabled && (!copy.g(field).is_null() || (!optional && !incomplete)) {
                let s = py_str(copy.g(actual));
                ydl.to_stdout(&s);
            }
        };
        print_field(self, self.o.gettitle, "title", "title", false);
        print_field(self, self.o.getid, "id", "id", false);
        print_field(self, self.o.geturl, "url", "urls", false);
        print_field(self, self.o.getthumbnail, "thumbnail", "thumbnail", true);
        print_field(self, self.o.getdescription, "description", "description", true);
        print_field(self, self.o.getfilename, "filename", "filename", false);
        if self.o.getduration {
            if let Some(d) = copy.get("duration").filter(|d| !d.is_null()).and_then(|d| d.as_f64()) {
                let s = util::format_seconds(d, ":");
                self.to_stdout(&s);
            }
        }
        print_field(self, self.o.getformat, "format", "format", false);
        if self.o.dumpjson {
            let s = json::dumps(&sanitize_info(info));
            self.to_stdout(&s);
        }
    }
}

/// `format_field(obj, field, f'{prefix}%s{suffix}')`: empty for a falsy value.
fn format_field(v: &Value, prefix: &str, suffix: Option<&str>) -> String {
    if v.truthy() { format!("{prefix}{}{}", py_str(v), suffix.unwrap_or("")) } else { String::new() }
}

fn join_nonempty(values: &[String], delim: &str) -> String {
    values.iter().filter(|v| !v.is_empty()).cloned().collect::<Vec<_>>().join(delim)
}

/// `downloader.shorten_protocol_name` (simplify=False).
fn shorten_protocol_name(proto: &str) -> &str {
    match proto {
        "m3u8_native" => "m3u8",
        "m3u8" => "m3u8F",
        "rtmp_ffmpeg" => "rtmpF",
        "http_dash_segments" => "dash",
        "http_dash_segments_generator" => "dashG",
        "websocket_frag" => "WSfrag",
        p => p,
    }
}

/// `YoutubeDL._format_note`, the note column of `--list-formats-old`.
fn format_note(f: &Map) -> String {
    let mut res = String::new();
    let sep = |res: &mut String, s: &str| {
        if !res.is_empty() {
            res.push_str(s);
        }
    };
    let pct = |spec: &str, v: &Value| pyfmt::format_value(&pyfmt::Spec::parse(spec).unwrap(), v).unwrap_or_else(|_| py_str(v));
    if ["f4f", "f4m"].contains(&f.str_of("ext").unwrap_or("")) {
        res += "(unsupported)";
    }
    if f.g("language").truthy() {
        sep(&mut res, " ");
        res += &format!("[{}]", py_str(f.g("language")));
    }
    if !f.g("format_note").is_null() {
        sep(&mut res, " ");
        res += &py_str(f.g("format_note"));
    }
    if !f.g("tbr").is_null() {
        sep(&mut res, ", ");
        res += &format!("{}k", pct(">4d", &Value::Int(f.g("tbr").as_f64().unwrap_or(0.0) as i64)));
    }
    if !f.g("container").is_null() {
        sep(&mut res, ", ");
        res += &format!("{} container", py_str(f.g("container")));
    }
    let vcodec = f.g("vcodec");
    if !vcodec.is_null() && vcodec.as_str() != Some("none") {
        sep(&mut res, ", ");
        res += &py_str(vcodec);
        if !f.g("vbr").is_null() {
            res += "@";
        }
    } else if !f.g("vbr").is_null() && !f.g("abr").is_null() {
        res += "video@";
    }
    if !f.g("vbr").is_null() {
        res += &format!("{}k", pct(">4d", &Value::Int(f.g("vbr").as_f64().unwrap_or(0.0) as i64)));
    }
    if !f.g("fps").is_null() {
        sep(&mut res, ", ");
        res += &format!("{}fps", py_str(f.g("fps")));
    }
    if !f.g("acodec").is_null() {
        sep(&mut res, ", ");
        if f.str_of("acodec") == Some("none") {
            res += "video only";
        } else {
            res += &pct("<5", &Value::Str(py_str(f.g("acodec"))));
        }
    } else if !f.g("abr").is_null() {
        sep(&mut res, ", ");
        res += "audio";
    }
    if !f.g("abr").is_null() {
        res += &format!("@{}k", pct(">3d", &Value::Int(f.g("abr").as_f64().unwrap_or(0.0) as i64)));
    }
    if !f.g("asr").is_null() {
        res += &format!(" ({}Hz)", pct(">5d", &Value::Int(f.g("asr").as_f64().unwrap_or(0.0) as i64)));
    }
    let bytes = |v: &Value| util::format_decimal_suffix(v, ".2", 1024.0).map(|s| s + "B").unwrap_or_else(|| "N/A".into());
    if !f.g("filesize").is_null() {
        sep(&mut res, ", ");
        res += &bytes(f.g("filesize"));
    } else if !f.g("filesize_approx").is_null() {
        sep(&mut res, ", ");
        res += &format!("~{}", bytes(f.g("filesize_approx")));
    }
    res
}

fn round2(x: f64) -> f64 {
    // Python round(x, 2) rounds the exact binary value half-to-even; formatting does the same.
    format!("{:.2}", x).parse().unwrap()
}

/// `_forceprint`'s format_tmpl: bare field names become `%(field)s`.
fn format_tmpl(tmpl: &str) -> String {
    // ([\w.:,]|-\d|(?P<dict>{([\w.:,]|-\d)+}))+=?
    let c: Vec<char> = tmpl.chars().collect();
    let simple = |c: &[char], i: usize| -> Option<usize> {
        let ch = *c.get(i)?;
        if ch.is_alphanumeric() || ch == '_' || ".:,".contains(ch) {
            Some(i + 1)
        } else if ch == '-' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            Some(i + 2)
        } else {
            None
        }
    };
    let mut i = 0;
    let mut has_dict = false;
    let end = if c.last() == Some(&'=') { c.len() - 1 } else { c.len() };
    let mut ok = end > 0;
    while ok && i < end {
        if let Some(n) = simple(&c, i) {
            i = n;
        } else if c[i] == '{' {
            let mut j = i + 1;
            let mut any = false;
            while let Some(n) = simple(&c, j) {
                j = n;
                any = true;
            }
            if any && c.get(j) == Some(&'}') && j < end {
                has_dict = true;
                i = j + 1;
            } else {
                ok = false;
            }
        } else {
            ok = false;
        }
    }
    if !ok {
        return tmpl.to_string();
    }
    let mut t = tmpl.to_string();
    let mut fmt = "%({})s".to_string();
    if t.starts_with('{') {
        t = format!(".{t}");
        fmt = "%({})j".into();
    }
    if let Some(s) = t.strip_suffix('=') {
        t = s.to_string();
        fmt = "{0} = %({0})#j".into();
    }
    let items: Vec<String> = if has_dict { vec![t] } else { t.split(',').map(String::from).collect() };
    items.iter().map(|x| fmt.replace("{0}", x).replace("{}", x)).collect::<Vec<_>>().join("\n")
}

fn sanitize_thumbnails(info: &mut Map) {
    let mut thumbs = match info.get("thumbnails") {
        Some(Value::List(l)) => l.clone(),
        Some(Value::Null) | None => match info.get("thumbnail").filter(|t| t.truthy()) {
            Some(t) => {
                let mut m = Map::new();
                m.set("url", t.clone());
                vec![Value::Dict(m)]
            }
            None => return,
        },
        _ => return,
    };
    if thumbs.is_empty() {
        return;
    }
    let key = |t: &Value| {
        let t = t.as_dict().cloned().unwrap_or_default();
        let n = |k: &str| t.get(k).filter(|v| !v.is_null()).and_then(|v| v.as_f64()).unwrap_or(-1.0);
        (n("preference"), n("width"), n("height"), py_str(t.get("id").filter(|v| !v.is_null()).unwrap_or(&Value::str(""))), py_str(t.g("url")))
    };
    thumbs.sort_by(|a, b| key(a).partial_cmp(&key(b)).unwrap_or(std::cmp::Ordering::Equal));
    for (i, t) in thumbs.iter_mut().enumerate() {
        if let Some(m) = t.as_dict_mut() {
            if m.g("id").is_null() {
                m.set("id", Value::Str(i.to_string()));
            }
            if m.g("width").truthy() && m.g("height").truthy() {
                let r = format!("{}x{}", util::int_or_none(m.g("width")).unwrap_or(0), util::int_or_none(m.g("height")).unwrap_or(0));
                m.set("resolution", Value::Str(r));
            }
        }
    }
    info.set("thumbnails", Value::List(thumbs));
}

fn ffmpeg_available() -> bool {
    let path = std::env::var("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|d| d.join("ffmpeg").is_file())
}

/// `expand_path`: `~` and environment variables.
fn expand_path(s: &str) -> String {
    let mut out = s.to_string();
    if out.starts_with('~') {
        let end = out.find('/').unwrap_or(out.len());
        if end == 1 {
            if let Ok(h) = std::env::var("HOME") {
                out = format!("{h}{}", &out[1..]);
            }
        }
    }
    // os.path.expandvars: $name and ${name}, left alone when unset
    let c: Vec<char> = out.chars().collect();
    let mut res = String::new();
    let mut i = 0;
    while i < c.len() {
        if c[i] == '$' {
            if c.get(i + 1) == Some(&'{') {
                if let Some(close) = c[i + 2..].iter().position(|&x| x == '}') {
                    let name: String = c[i + 2..i + 2 + close].iter().collect();
                    if let Ok(v) = std::env::var(&name) {
                        res.push_str(&v);
                        i += close + 3;
                        continue;
                    }
                }
            } else {
                let n = c[i + 1..].iter().take_while(|x| x.is_ascii_alphanumeric() || **x == '_').count();
                if n > 0 {
                    let name: String = c[i + 1..i + 1 + n].iter().collect();
                    if let Ok(v) = std::env::var(&name) {
                        res.push_str(&v);
                        i += n + 1;
                        continue;
                    }
                }
            }
        }
        res.push(c[i]);
        i += 1;
    }
    res
}

/// `os.path.join`.
fn path_join(parts: &[&str]) -> String {
    let mut out = String::new();
    for p in parts {
        if p.starts_with('/') {
            out = p.to_string();
        } else if out.is_empty() || out.ends_with('/') {
            out.push_str(p);
        } else {
            out.push('/');
            out.push_str(p);
        }
    }
    out
}

/// Python's `str.title()`: a cased character is uppercased after an uncased one, lowercased otherwise.
fn py_title(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_cased = false;
    for c in s.chars() {
        let cased = c.is_lowercase() || c.is_uppercase();
        if cased && !prev_cased {
            out.extend(c.to_uppercase());
        } else if cased {
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
        prev_cased = cased;
    }
    out
}

/// `HTTPHeaderDict.update`: keys stored title-cased, values as stripped strings.
fn header_update(headers: &mut Map, other: &Value) {
    if let Value::Dict(m) = other {
        for (k, v) in &m.0 {
            let s = match v {
                Value::Null => "None".to_string(),
                v => py_str(v),
            };
            headers.set(&py_title(k), Value::Str(s.trim().to_string()));
        }
    }
}

/// `std_headers`. `random_user_agent` picks a Chrome major version from 145 to 151 on every run;
/// this uses the newest one.
fn std_headers() -> Map {
    let mut h = Map::new();
    for (k, v) in [
        ("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36"),
        ("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8"),
        ("Accept-Language", "en-us,en;q=0.5"),
        ("Sec-Fetch-Mode", "navigate"),
    ] {
        h.set(k, Value::str(v));
    }
    h
}

/// The defaults `sanitize_info` sets on the info dict before filtering it.
fn set_sanitize_defaults(m: &mut Map) {
    m.setdefault("epoch", Value::Int(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)));
    m.setdefault("_type", Value::str("video"));
    let mut version = Map::new();
    version.set("version", Value::str(VERSION));
    version.set("current_git_head", Value::Null);
    version.set("release_git_head", Value::str(debug::RELEASE_GIT_HEAD));
    version.set("repository", Value::str("yt-dlp/yt-dlp"));
    m.setdefault("_version", Value::Dict(version));
}

/// sanitize_info(info) (remove_private_keys=False)
fn sanitize_info(info: &Map) -> Value {
    let mut m = info.clone();
    set_sanitize_defaults(&mut m);
    Value::Dict(m)
}

/// sanitize_info(info, remove_private_keys=True)
fn clean_info(v: &Value) -> Value {
    const REJECT: &[&str] = &[
        "requested_downloads", "requested_formats", "requested_subtitles", "requested_entries", "entries", "filepath", "_filename",
        "filename", "infojson_filename", "original_url", "playlist_autonumber",
    ];
    match v {
        Value::Dict(m) => Value::Dict(Map(
            m.0.iter()
                .filter(|(k, v)| !(v.is_null() || k.starts_with("__") || REJECT.contains(&k.as_str())))
                .map(|(k, v)| (k.clone(), clean_info(v)))
                .collect(),
        )),
        Value::List(l) => Value::List(l.iter().map(clean_info).collect()),
        v => v.clone(),
    }
}

/// The parts of `validate_options` that apply to the supported options; returns its warnings.
fn validate_options(o: &mut Opts) -> Vec<String> {
    let mut warnings = Vec::new();
    // Common mistake: -f best
    if o.format.as_deref() == Some("best") {
        warnings.push(
            [
                "\"-f best\" selects the best pre-merged format which is often not the best option",
                "To let yt-dlp download and merge the best available formats, simply do not pass any format selection",
                "If you know what you are doing and want only the best pre-merged format, use \"-f b\" instead to suppress this warning",
            ]
            .join(".\n         "),
        );
    }
    // Common mistake: -f mp4
    if o.format.as_deref() == Some("mp4") {
        warnings.push(
            [
                "\"-f mp4\" selects the best pre-merged mp4 format which is often not what's intended",
                "Pre-merged mp4 formats are not available from all sites, or may only be available in lower quality",
                "To prioritize the best h264 video and aac audio in an mp4 container, use \"-t mp4\" instead",
                "If you know what you are doing and want a pre-merged mp4 format, use \"-f b[ext=mp4]\" instead to suppress this warning",
            ]
            .join(".\n         "),
        );
    }
    // report_conflict('--id', 'useid', '--output', 'outtmpl', val2=opts.outtmpl.get('default'))
    if o.outtmpl.as_deref().is_some_and(|t| !t.is_empty()) {
        if o.useid {
            warnings.push("--id is ignored since --output was given".into());
        }
        o.useid = false;
    }
    if o.useid {
        o.outtmpl = Some("%(id)s.%(ext)s".into());
    }
    warnings
}

/// The extractor registry in `list_extractor_classes` order, one IE_NAME per line, with
/// " (CURRENTLY BROKEN)" after extractors whose `_WORKING` is False.
const EXTRACTORS: &str = include_str!("../data/extractors.txt");
/// `adobepass.MSO_INFO`: an mso id, spaces, then its name, per line.
const MSO_INFO: &str = include_str!("../data/mso.txt");
/// `InfoExtractor.description(markdown=False)` of every working, non-hidden extractor, in
/// `list_extractor_classes` order. A search extractor's line ends in a tab and its SEARCH_KEY,
/// where the random search example goes.
const EXTRACTOR_DESCRIPTIONS: &str = include_str!("../data/extractor_descriptions.txt");
/// optparse's `format_help()` for the option parser, as laid out at COLUMNS=120.
const HELP: &str = include_str!("../data/help.txt");

/// Python seeds `random` from OS entropy, so each run picks fresh examples; std's RandomState
/// is seeded the same way.
fn random_below(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_usize(n);
    (h.finish() % n as u64) as usize
}

/// `print_extractor_information`'s `--extractor-descriptions` listing, without a final newline.
fn extractor_descriptions() -> String {
    const SEARCHES: &[&str] = &[
        "cute kittens", "slithering pythons", "falling cat", "angry poodle", "purple fish", "running tortoise",
        "sleeping bunny", "burping cow",
    ];
    const COUNTS: &[&str] = &["", "5", "10", "all"];
    EXTRACTOR_DESCRIPTIONS
        .lines()
        .map(|line| match line.split_once('\t') {
            Some((desc, key)) => {
                let count = COUNTS[random_below(COUNTS.len())];
                let search = SEARCHES[random_below(SEARCHES.len())];
                format!("{desc} (e.g. \"{key}{count}:{search}\")")
            }
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `print_extractor_information`; true when it printed something and yt-dlp should stop.
fn print_extractor_information(o: &Opts) -> bool {
    let out = if o.list_extractors {
        // Matching URLs to extractors needs their _VALID_URL patterns, which the rewrite lacks:
        // every URL is listed under the generic extractor, as one no other extractor suits.
        let mut out = String::new();
        for line in EXTRACTORS.lines() {
            out.push_str(line);
            out.push('\n');
            if line == "generic" {
                for url in o.urls.iter().fold(Vec::<&String>::new(), |mut seen, u| {
                    if !seen.contains(&u) {
                        seen.push(u);
                    }
                    seen
                }) {
                    out.push_str(&format!("  {url}\n"));
                }
            }
        }
        out
    } else if o.list_extractor_descriptions {
        extractor_descriptions()
    } else if o.ap_list_mso {
        let rows: Vec<Vec<String>> = MSO_INFO
            .lines()
            .map(|l| {
                let (id, name) = l.split_once(' ').unwrap_or((l, ""));
                vec![id.to_string(), name.trim_start().to_string()]
            })
            .collect();
        let header = vec!["mso".to_string(), "mso name".to_string()];
        format!("Supported TV Providers:\n{}\n", util::render_table(&header, &rows, None, 0, false))
    } else {
        return false;
    };
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(out.as_bytes());
    let _ = stdout.flush();
    true
}

/// `YoutubeDL.warn_if_short_id`: a bare YouTube ID starting with a dash looks like an option.
fn warn_if_short_id(ydl: &mut Ydl, argv: &[String]) {
    let is_id = |a: &str| a.len() == 11 && a.starts_with('-') && a[1..].chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !argv.iter().any(|a| is_id(a)) {
        return;
    }
    let correct_argv: Vec<String> = std::iter::once("yt-dlp")
        .chain(argv.iter().map(String::as_str).filter(|a| !is_id(a)))
        .chain(std::iter::once("--"))
        .chain(argv.iter().map(String::as_str).filter(|a| is_id(a)))
        .map(util::shell_quote)
        .collect();
    ydl.report_warning(&format!("Long argument string detected. Use -- to separate parameters and URLs, like this:\n{}", correct_argv.join(" ")));
}

impl Ydl {
    /// `_calc_headers(ChainMap(fmt, info))`, without a cookie jar to consult.
    fn calc_headers(&self, fmt: &Map, info: &Map) -> Map {
        let chained = |k: &str| fmt.get(k).or_else(|| info.get(k)).cloned().unwrap_or(Value::Null);
        let mut res = self.http_headers.clone();
        header_update(&mut res, &chained("http_headers"));
        // clean_headers
        if res.remove("Youtubedl-No-Compression").is_some() {
            res.set("Accept-Encoding", Value::str("identity"));
        }
        res.remove("Ytdl-Socks-Proxy");
        // The `Cookie` header is removed to prevent leaks and unscoped cookies.
        res.remove("Cookie");
        if !res.contains("X-Forwarded-For") {
            let ip = chained("__x_forwarded_for_ip");
            if ip.truthy() {
                header_update(&mut res, &Value::Dict(Map(vec![("X-Forwarded-For".into(), ip)])));
            }
        }
        res
    }

    /// `Cache.remove`.
    fn remove_cache(&mut self) {
        let root = match &self.o.cachedir {
            Some(None) => {
                self.to_screen("Cache is disabled (Did you combine --no-cache-dir and --rm-cache-dir?)");
                return;
            }
            Some(Some(d)) => d.clone(),
            None => format!("{}/yt-dlp", std::env::var("XDG_CACHE_HOME").unwrap_or_else(|_| "~/.cache".into())),
        };
        let cachedir = match root.strip_prefix('~') {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("{}{rest}", std::env::var("HOME").unwrap_or_default()),
            _ => root,
        };
        if !cachedir.contains("cache") && !cachedir.contains("tmp") {
            self.report_error(&format!("Not removing directory {cachedir} - this does not look like a cache dir"));
            return;
        }
        let show = !self.quiet || self.o.verbose;
        if show {
            let _ = write!(self.out, "Removing cache dir {cachedir} .");
        }
        if std::path::Path::new(&cachedir).exists() {
            if show {
                let _ = write!(self.out, ".");
            }
            let _ = std::fs::remove_dir_all(&cachedir);
        }
        self.to_screen(".");
    }
}

fn main() {
    // The reference program is `yt-dlp --ignore-config --no-cache-dir` (parity.toml [original].args),
    // so those arguments lead argv here too: they show up in argv echoes and disable the cache.
    let args: Vec<String> = ["--ignore-config", "--no-cache-dir"].iter().map(|s| s.to_string()).chain(std::env::args().skip(1)).collect();
    let mut o = parse_args(&args);
    if o.verbose {
        eprintln!("{}", debug::command_line_config(&args));
    }
    let warnings = validate_options(&mut o);
    if print_extractor_information(&o) {
        exit(0);
    }
    let deprecation_warnings: Vec<String> = if o.deprecated_options.is_empty() {
        vec![]
    } else {
        vec![format!(
            "The following options have been deprecated: {}\n\
             Please remove them from your command/configuration to avoid future errors.\n\
             See  https://github.com/yt-dlp/yt-dlp/issues/14198  for more details",
            o.deprecated_options.join(", ")
        )]
    };

    let any_getting = o.dumpjson
        || o.dump_single_json
        || o.getdescription
        || o.getduration
        || o.getfilename
        || o.getformat
        || o.getid
        || o.getthumbnail
        || o.gettitle
        || o.geturl;
    let print_only = !o.forceprint.is_empty() && o.forceprint.iter().all(|(w, _)| !POSTPROCESS_WHEN[3..].contains(&w.as_str()) || w == "video");
    let print_only = print_only && o.forceprint.iter().all(|(w, _)| !POSTPROCESS_WHEN[3..].contains(&w.as_str()));
    let quiet = o.quiet.unwrap_or(any_getting || !o.forceprint.is_empty());
    let simulate = o.simulate.unwrap_or(print_only || any_getting);
    let list_only = o.simulate.is_none() && !simulate && (o.list_thumbnails || o.listformats);
    let simulate = simulate || list_only;

    if let Some(ua) = o.user_agent.clone() {
        o.headers.setdefault("User-Agent", Value::Str(ua));
    }
    if let Some(r) = o.referer.clone() {
        o.headers.setdefault("Referer", Value::Str(r));
    }
    let mut http_headers = std_headers();
    header_update(&mut http_headers, &Value::Dict(o.headers.clone()));

    let useid = o.useid;
    let mut ydl = Ydl {
        http_headers,
        o,
        quiet,
        simulate,
        list_only,
        retcode: 0,
        num_downloads: 0,
        num_videos: 0,
        out: std::io::stdout(),
        printed_messages: Default::default(),
    };
    if ydl.o.allow_unplayable_formats {
        ydl.report_warning(
            "You have asked for UNPLAYABLE formats to be listed/downloaded. This is a developer option intended for debugging. \n         \
             If you experience any issues while using this option, DO NOT open a bug report",
        );
    }
    if ydl.o.verbose {
        for line in debug::debug_header(quiet) {
            eprintln!("{line}");
        }
    }
    if useid {
        ydl.report_warning("--id is deprecated. Use -o \"%(id)s.%(ext)s\" instead");
    }
    for w in &warnings {
        ydl.report_warning(w);
    }
    for w in &deprecation_warnings {
        ydl.deprecated_feature(w);
    }
    ydl.write_debug(&format!("Loaded {} extractors", debug::EXTRACTOR_COUNT));

    if ydl.o.rm_cachedir {
        ydl.remove_cache();
    }

    if ydl.o.list_impersonate_targets {
        // No impersonation backend (curl_cffi) is available, so every known target is listed as unavailable,
        // each inserted at the front.
        let known = [("chrome", "curl_cffi"), ("safari", "curl_cffi"), ("firefox", "curl_cffi>=0.10"), ("edge", "curl_cffi"), ("tor", "curl_cffi>=0.11")];
        let rows: Vec<Vec<String>> = known
            .iter()
            .rev()
            .map(|(client, handler)| {
                let mut c = client.chars();
                let title = c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default();
                vec![title, "-".into(), format!("{handler} (unavailable)")]
            })
            .collect();
        let header: Vec<String> = ["Client", "OS", "Source"].iter().map(|s| s.to_string()).collect();
        ydl.to_screen("[info] Available impersonate targets");
        let table = util::render_table(&header, &rows, Some("-"), 2, false);
        ydl.to_stdout(&table);
        let _ = ydl.out.flush();
        exit(0);
    }

    let Some(info_file) = ydl.o.load_info_json.clone() else {
        if ydl.o.urls.is_empty() {
            if ydl.o.rm_cachedir {
                exit(ydl.retcode);
            }
            warn_if_short_id(&mut ydl, &args);
            eprint!("\n{USAGE}\n\nyt-dlp: error: You must provide at least one URL.\nType yt-dlp --help to see a list of all options.\n");
            exit(2);
        }
        eprintln!("ERROR: extraction from URLs is not supported");
        exit(1);
    };

    let text = match std::fs::read_to_string(&info_file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("ERROR: {e}");
            exit(1);
        }
    };
    let parsed = match json::parse(&text) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("ERROR: {e}");
            exit(1);
        }
    };
    let infos = match parsed {
        Value::List(l) => l,
        v => vec![v],
    };
    for info in infos {
        let Value::Dict(mut m) = info else { continue };
        set_sanitize_defaults(&mut m);
        let Value::Dict(m) = clean_info(&Value::Dict(m)) else { unreachable!() };
        if let Err(e) = ydl.process_video_result(m) {
            ydl.report_error(&e);
        }
    }
    let _ = ydl.out.flush();
    exit(ydl.retcode);
}
