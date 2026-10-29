//! The surface map: what a program exposes and depends on, from its file (via parity-rea), its own `--help`, and
//! the source of script programs (Python, Node). It drives case generation: every flag and subcommand found is a
//! behaviour to record.

use crate::project::{CaseInput, Program, SandboxConfig};
use crate::sandbox;
use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Debug, Default, Serialize)]
pub struct Surface {
    pub program: String,
    pub path: String,
    /// `elf`, `pe`, `mach-o`, `script` (see `interpreter`) or `unknown`.
    pub kind: String,
    pub interpreter: Option<String>,
    pub binary: Option<parity_rea::Overview>,
    pub help: Option<String>,
    pub version: Option<String>,
    pub flags: Vec<Flag>,
    pub subcommands: Vec<String>,
    pub subcommand_help: BTreeMap<String, String>,
    pub env_vars: BTreeSet<String>,
    pub paths: BTreeSet<String>,
    pub urls: BTreeSet<String>,
    /// Source files read for script programs.
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Flag {
    pub long: Option<String>,
    pub short: Option<String>,
    /// The value's name when the flag takes one (`FILE`, `<n>`).
    pub value: Option<String>,
    pub help: String,
}

pub struct Options {
    /// Run `--help`, `--version` and each subcommand's `--help` (in the project's sandbox).
    pub run_help: bool,
    /// Subcommands whose help is read, at most.
    pub max_subcommands: usize,
}

pub fn discover(program: &Program, sandbox_cfg: &SandboxConfig, opts: &Options) -> Result<Surface> {
    let path = sandbox::resolve(&program.program)?;
    let data = std::fs::read(&path).with_context(|| format!("{}", path.display()))?;
    let mut s = Surface {
        program: program.program.clone(),
        path: path.display().to_string(),
        ..Default::default()
    };
    let mut texts: Vec<String> = Vec::new();

    if let Some(line) = data.strip_prefix(b"#!") {
        let first = line.split(|b| *b == b'\n').next().unwrap_or_default();
        let interpreter = String::from_utf8_lossy(first).trim().to_string();
        s.kind = "script".into();
        let script = String::from_utf8_lossy(&data).into_owned();
        if interpreter.contains("python") {
            for file in python_sources(&script, &interpreter) {
                if let Ok(text) = std::fs::read_to_string(&file) {
                    s.sources.push(file.display().to_string());
                    texts.push(text);
                }
            }
        } else if interpreter.contains("node") {
            for file in node_sources(&path) {
                if let Ok(text) = std::fs::read_to_string(&file) {
                    s.sources.push(file.display().to_string());
                    texts.push(text);
                }
            }
        }
        texts.push(script);
        s.interpreter = Some(interpreter);
        for text in &texts {
            s.flags.extend(source_flags(text));
            s.env_vars.extend(source_env(text));
        }
    } else if let Ok(o) = parity_rea::overview(&data) {
        s.kind = match o.format.as_str() {
            "Elf" => "elf",
            "Pe" => "pe",
            "MachO" => "mach-o",
            _ => "unknown",
        }
        .into();
        let reads_env = o.imports.iter().any(|i| {
            matches!(
                i.name.as_str(),
                "getenv" | "secure_getenv" | "GetEnvironmentVariableW"
            )
        }) || o.hints.iter().any(|h| h == "rust" || h == "go");
        let found: Vec<String> = parity_rea::strings(&data, 4, None, 0)
            .into_iter()
            .map(|f| f.text)
            .collect();
        if reads_env {
            s.env_vars
                .extend(found.iter().filter(|t| ENV_NAME.is_match(t)).cloned());
        }
        texts.extend(found);
        s.binary = Some(o);
    } else {
        s.kind = "unknown".into();
        texts.push(String::from_utf8_lossy(&data).into_owned());
    }

    for text in &texts {
        s.urls.extend(
            URL.find_iter(text)
                .map(|m| m.as_str().trim_end_matches(['.', ',', ')']).to_string()),
        );
        s.paths
            .extend(PATH.find_iter(text).map(|m| m.as_str().to_string()));
    }

    if opts.run_help {
        let run = |args: &[&str]| -> Option<String> {
            let case = CaseInput {
                name: "discover".into(),
                args: args.iter().map(|a| a.to_string()).collect(),
                ..Default::default()
            };
            let o = sandbox::run(program, &case, sandbox_cfg).ok()?;
            let out = if o.stdout.is_empty() {
                o.stderr
            } else {
                o.stdout
            };
            let text = String::from_utf8_lossy(&out).trim_end().to_string();
            (!text.is_empty()).then_some(text)
        };
        s.help = run(&["--help"]);
        s.version = run(&["--version"]);
        if let Some(help) = &s.help {
            let (flags, subcommands) = parse_help(help);
            s.flags.extend(flags);
            s.subcommands = subcommands;
        }
        for sub in s.subcommands.clone().iter().take(opts.max_subcommands) {
            if let Some(help) = run(&[sub, "--help"]) {
                s.subcommand_help.insert(sub.clone(), help);
            }
        }
    }
    s.flags = merge_flags(std::mem::take(&mut s.flags));
    Ok(s)
}

static ENV_NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Z][A-Z0-9]*_[A-Z0-9_]{1,40}$").unwrap());
static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s"'<>`\\]+"#).unwrap());
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:~|\$HOME|\$XDG_[A-Z_]+|/etc|/usr/share|/var/lib|/var/cache)/[A-Za-z0-9._/@+-]+|\.config/[A-Za-z0-9._/@+-]+")
        .unwrap()
});
static SOURCE_ENV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:os\.environ\.get\(|os\.getenv\(|os\.environ\[|environ\.get\(|process\.env\[|std::env::var(?:_os)?\()\s*['"]([A-Za-z_][A-Za-z0-9_]*)['"]|process\.env\.([A-Za-z_][A-Za-z0-9_]*)"#,
    )
    .unwrap()
});
static SOURCE_FLAG_CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:add_argument|add_option|click\.option|\.option|\.requiredOption)\(\s*((?:['"][^'"]*['"]\s*,\s*)*['"][^'"]*['"])"#)
        .unwrap()
});
static QUOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"['"]([^'"]*)['"]"#).unwrap());

fn source_env(text: &str) -> impl Iterator<Item = String> + '_ {
    SOURCE_ENV.captures_iter(text).filter_map(|c| {
        c.get(1)
            .or_else(|| c.get(2))
            .map(|m| m.as_str().to_string())
    })
}

/// Flags declared in source: argparse/optparse/click (`add_argument('-o', '--output')`) and commander/yargs
/// (`.option('-o, --output <file>')`).
fn source_flags(text: &str) -> Vec<Flag> {
    let mut out = Vec::new();
    for call in SOURCE_FLAG_CALL.captures_iter(text) {
        let mut flag = Flag::default();
        for q in QUOTED.captures_iter(&call[1]) {
            for token in q[1].split([',', ' ']).filter(|t| !t.is_empty()) {
                apply_token(&mut flag, token);
            }
        }
        if flag.long.is_some() || flag.short.is_some() {
            out.push(flag);
        }
    }
    out
}

fn apply_token(flag: &mut Flag, token: &str) {
    if let Some(long) = token.strip_prefix("--") {
        let (name, value) = long
            .split_once('=')
            .map_or((long, None), |(n, v)| (n, Some(v)));
        if !name.is_empty() && flag.long.is_none() {
            flag.long = Some(format!("--{name}"));
        }
        if let Some(v) = value.filter(|v| !v.is_empty()) {
            flag.value.get_or_insert_with(|| v.to_string());
        }
    } else if token.len() >= 2 && token.starts_with('-') && !token.starts_with("--") {
        let short = &token[..2];
        if flag.short.is_none()
            && short
                .chars()
                .nth(1)
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '?')
        {
            flag.short = Some(short.to_string());
        }
    } else if !token.is_empty()
        && (token.starts_with('<')
            || token.starts_with('[')
            || token
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c == '-'))
    {
        flag.value.get_or_insert_with(|| token.to_string());
    }
}

/// Flags and subcommands from a help text. Flag lines start (after indentation) with `-`; the part before the
/// first run of two spaces names the flag, the rest is its help. Subcommands are the indented first words of a
/// section whose header mentions commands, and argparse's `{a,b,c}`.
pub fn parse_help(help: &str) -> (Vec<Flag>, Vec<String>) {
    static GAP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s{2,}|\t").unwrap());
    static CHOICES: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\{([a-z][\w-]*(?:,[a-z][\w-]*)+)\}").unwrap());
    static SUB: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s{1,8}([a-z][\w:-]*)(?:\s{2,}|\t|$)").unwrap());
    let mut flags = Vec::new();
    let mut subs = Vec::new();
    let mut in_commands = false;
    for line in help.lines() {
        let trimmed = line.trim_start();
        if !line.starts_with([' ', '\t']) && !trimmed.is_empty() {
            let lower = trimmed.to_lowercase();
            in_commands = trimmed.ends_with(':') && lower.contains("command");
            continue;
        }
        if trimmed.starts_with('-') {
            let (head, help) = match GAP.find(trimmed) {
                Some(m) => (&trimmed[..m.start()], trimmed[m.end()..].trim()),
                None => (trimmed, ""),
            };
            let mut flag = Flag {
                help: help.to_string(),
                ..Default::default()
            };
            for token in head.split([',', ' ']).filter(|t| !t.is_empty()) {
                apply_token(&mut flag, token);
            }
            if flag.long.is_some() || flag.short.is_some() {
                flags.push(flag);
            }
        } else if in_commands && let Some(c) = SUB.captures(line) {
            subs.push(c[1].to_string());
        }
        for c in CHOICES.captures_iter(line) {
            subs.extend(c[1].split(',').map(str::to_string));
        }
    }
    let mut seen = BTreeSet::new();
    subs.retain(|s| seen.insert(s.clone()));
    (flags, subs)
}

/// One entry per flag: help-text entries win over source entries, which only fill gaps.
fn merge_flags(flags: Vec<Flag>) -> Vec<Flag> {
    let mut by_key: BTreeMap<(Option<String>, Option<String>), Flag> = BTreeMap::new();
    let mut long_to_key: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for flag in flags {
        let key = flag
            .long
            .as_ref()
            .and_then(|l| long_to_key.get(l).cloned())
            .unwrap_or_else(|| (flag.long.clone(), flag.short.clone()));
        match by_key.get_mut(&key) {
            Some(existing) => {
                if existing.help.is_empty() {
                    existing.help = flag.help;
                }
                if existing.value.is_none() {
                    existing.value = flag.value;
                }
                if existing.short.is_none() {
                    existing.short = flag.short;
                }
            }
            None => {
                if let Some(l) = &flag.long {
                    long_to_key.insert(l.clone(), key.clone());
                }
                by_key.insert(key, flag);
            }
        }
    }
    by_key.into_values().collect()
}

/// The first-party Python package a console script runs: the top-level module of its `from X import` /
/// `import X` lines, located with the script's own interpreter (`find_spec` of a top-level name imports
/// nothing), then every `.py` file under it.
fn python_sources(script: &str, interpreter: &str) -> Vec<PathBuf> {
    static IMPORT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^\s*(?:from|import)\s+([A-Za-z_][A-Za-z0-9_]*)").unwrap()
    });
    const STDLIB: &[&str] = &["sys", "os", "re", "importlib", "runpy", "__future__"];
    let python = interpreter.split_whitespace().last().unwrap_or("python3");
    let python = if python == "/usr/bin/env" {
        "python3"
    } else {
        python
    };
    let mut out = Vec::new();
    for c in IMPORT.captures_iter(script) {
        let module = &c[1];
        if STDLIB.contains(&module) {
            continue;
        }
        let Ok(o) = std::process::Command::new(python)
            .args(["-I", "-c", "import importlib.util,sys;s=importlib.util.find_spec(sys.argv[1]);print(s.origin if s else '')", module])
            .output()
        else {
            continue;
        };
        let origin = String::from_utf8_lossy(&o.stdout).trim().to_string();
        let origin = Path::new(&origin);
        let root = if origin.file_name().is_some_and(|n| n == "__init__.py") {
            origin.parent()
        } else {
            Some(origin)
        };
        if let Some(root) = root.filter(|r| r.exists()) {
            out.extend(walk(root, "py"));
        }
    }
    out
}

/// A Node script's package: the folder with the nearest package.json above the real script, without
/// node_modules.
fn node_sources(script: &Path) -> Vec<PathBuf> {
    let real = std::fs::canonicalize(script).unwrap_or_else(|_| script.to_path_buf());
    let root = real
        .ancestors()
        .skip(1)
        .find(|d| d.join("package.json").is_file());
    root.map(|r| {
        walk(r, "js")
            .into_iter()
            .chain(walk(r, "mjs"))
            .chain(walk(r, "cjs"))
            .collect()
    })
    .unwrap_or_default()
}

fn walk(root: &Path, ext: &str) -> Vec<PathBuf> {
    if root.is_file() {
        return vec![root.to_path_buf()];
    }
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.file_name() != "node_modules" && e.file_name() != "__pycache__")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == ext))
        .map(|e| e.into_path())
        .collect()
}

/// Cases worth recording for a surface: help, version, no arguments, each subcommand's help, and each flag
/// that takes no value on its own. Names are prefixed `gen-` so they never collide with hand-written ones.
pub fn suggested_cases(s: &Surface) -> Vec<CaseInput> {
    let case = |name: String, args: Vec<String>| CaseInput {
        name: format!("gen-{}", sanitize(&name)),
        args,
        ..Default::default()
    };
    let mut out = vec![
        case("no-args".into(), vec![]),
        case("help".into(), vec!["--help".into()]),
        case("version".into(), vec!["--version".into()]),
    ];
    for sub in &s.subcommands {
        out.push(case(
            format!("{sub}-help"),
            vec![sub.clone(), "--help".into()],
        ));
    }
    for flag in s.flags.iter().filter(|f| f.value.is_none()) {
        let Some(arg) = flag.long.as_ref().or(flag.short.as_ref()) else {
            continue;
        };
        if matches!(arg.as_str(), "--help" | "-h" | "--version" | "-V") {
            continue;
        }
        out.push(case(format!("flag{arg}"), vec![arg.clone()]));
    }
    let mut seen = BTreeSet::new();
    out.retain(|c| seen.insert(c.name.clone()));
    out
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELP: &str = "\
Usage: tool [OPTIONS] <COMMAND>

Commands:
  fetch   Download things
  show    Print things
  help    Print help

Options:
  -o, --output <FILE>  Where to write
      --dry-run        Do nothing
  -v                   Verbose
  -h, --help           Print help
";

    #[test]
    fn parses_clap_style_help() {
        let (flags, subs) = parse_help(HELP);
        assert_eq!(subs, ["fetch", "show", "help"]);
        let output = flags
            .iter()
            .find(|f| f.long.as_deref() == Some("--output"))
            .unwrap();
        assert_eq!(output.short.as_deref(), Some("-o"));
        assert_eq!(output.value.as_deref(), Some("<FILE>"));
        assert_eq!(output.help, "Where to write");
        assert!(
            flags
                .iter()
                .any(|f| f.long.as_deref() == Some("--dry-run") && f.value.is_none())
        );
        assert!(
            flags
                .iter()
                .any(|f| f.short.as_deref() == Some("-v") && f.long.is_none())
        );
    }

    #[test]
    fn parses_argparse_choices() {
        let (_, subs) = parse_help(
            "usage: x [-h] {build,clean,test} ...\n\npositional arguments:\n  {build,clean,test}\n",
        );
        assert_eq!(subs, ["build", "clean", "test"]);
    }

    #[test]
    fn reads_flags_and_env_from_source() {
        let py = r#"parser.add_option('-f', '--format', dest='fmt', metavar='FORMAT')
x = os.environ.get("YTDLP_HOME")"#;
        let flags = source_flags(py);
        assert_eq!(flags[0].long.as_deref(), Some("--format"));
        assert_eq!(flags[0].short.as_deref(), Some("-f"));
        assert_eq!(source_env(py).collect::<Vec<_>>(), ["YTDLP_HOME"]);
        let js = "program.option('-p, --port <n>', 'port'); const t = process.env.API_TOKEN;";
        let flags = source_flags(js);
        assert_eq!(flags[0].value.as_deref(), Some("<n>"));
        assert_eq!(source_env(js).collect::<Vec<_>>(), ["API_TOKEN"]);
    }

    #[test]
    fn help_text_wins_and_source_fills_gaps() {
        let merged = merge_flags(vec![
            Flag {
                long: Some("--out".into()),
                help: "from help".into(),
                ..Default::default()
            },
            Flag {
                long: Some("--out".into()),
                short: Some("-o".into()),
                help: "from source".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].help, "from help");
        assert_eq!(merged[0].short.as_deref(), Some("-o"));
    }

    #[test]
    fn suggests_cases_for_subcommands_and_plain_flags() {
        let (flags, subcommands) = parse_help(HELP);
        let s = Surface {
            flags: merge_flags(flags),
            subcommands,
            ..Default::default()
        };
        let names: Vec<String> = suggested_cases(&s).into_iter().map(|c| c.name).collect();
        assert!(names.contains(&"gen-fetch-help".to_string()));
        assert!(names.contains(&"gen-flag--dry-run".to_string()));
        assert!(!names.iter().any(|n| n.contains("output")));
        assert!(
            names
                .iter()
                .all(|n| crate::project::check_case_name(n).is_ok())
        );
    }
}
