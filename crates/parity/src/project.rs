//! A Parity project: `parity.toml` (the programs, the sandbox, the normalizers) and `cases.toml` (the inputs to
//! record), both in the project folder. Recordings go to `corpus/`.

use crate::normalize::Rule;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

pub const PROJECT_FILE: &str = "parity.toml";
pub const CASES_FILE: &str = "cases.toml";
pub const CORPUS_DIR: &str = "corpus";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    /// The program being rewritten: the oracle.
    pub original: Program,
    /// The rewrite under test; `parity replay --candidate` overrides it.
    pub candidate: Option<Program>,
    #[serde(default)]
    pub sandbox: SandboxConfig,
    #[serde(default)]
    pub normalize: Vec<Rule>,
    #[serde(default)]
    pub rewrite: RewriteConfig,
    /// Where new releases of the original come from (`parity track`).
    pub upstream: Option<Upstream>,
    /// For servers: how replayed HTTP exchanges reach the candidate (`parity http`).
    pub http: Option<HttpConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Program {
    /// A name looked up on PATH, or a path.
    pub program: String,
    /// Arguments put before every case's own.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment for this program only (PYTHONPATH pointing at a rewritten module, for instance).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RewriteConfig {
    /// The Rust crate the agent writes, relative to the project.
    pub dir: String,
    /// The binary the crate builds (default: the original's file name).
    pub binary: Option<String>,
    /// `claude`, `codex`, `opencode`, or `custom` with `command`.
    pub agent: String,
    /// The agent's command line; `{prompt}` is replaced by the task, `{prompt_file}` by its path.
    pub command: Vec<String>,
    pub max_iterations: u32,
    /// Failing cases handed to the agent per iteration.
    pub batch: usize,
    /// Stop after this many iterations without more matching cases.
    pub stall_limit: u32,
    /// Run in the crate folder after each successful build (copying a built extension into place, say).
    pub post_build: Vec<String>,
}

impl Default for RewriteConfig {
    fn default() -> Self {
        Self {
            dir: "rewrite".into(),
            binary: None,
            agent: "claude".into(),
            command: Vec::new(),
            max_iterations: 50,
            batch: 5,
            stall_limit: 3,
            post_build: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpstreamSource {
    Pypi,
    Github,
    Npm,
    Crates,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Upstream {
    pub source: UpstreamSource,
    /// The package name, or `owner/repo` on GitHub.
    pub name: String,
    /// Run to install a new release before re-recording (`{version}` is replaced). Without it, `parity track`
    /// only reports.
    #[serde(default)]
    pub install: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HttpConfig {
    /// The original server's base URL, which `parity http record` proxies to.
    pub original_url: String,
    /// The candidate server's base URL that `parity http replay` sends recorded requests to.
    pub candidate_url: String,
    /// Response headers not compared (lower case). Date and Server are always skipped.
    pub ignore_headers: Vec<String>,
}

/// How a run is isolated. The defaults favour reproducible recordings; none of them is a restriction.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Isolation {
    /// bubblewrap when it is installed, else `none`.
    #[default]
    Auto,
    /// bubblewrap: read-only system, writable work folder and /tmp, own PID/IPC namespaces.
    Bwrap,
    /// The real system: for programs that must touch real files, devices or services.
    None,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SandboxConfig {
    pub isolation: Isolation,
    /// Let the program reach the network (always reachable with `isolation = "none"`).
    pub network: bool,
    /// Keep the caller's whole environment instead of clearing it.
    pub inherit_env: bool,
    /// The environment every run gets, on top of PATH and HOME.
    pub env: BTreeMap<String, String>,
    /// A run longer than this is killed and recorded as a timeout; 0 waits forever.
    pub timeout_secs: u64,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        let env = [("LANG", "C.UTF-8"), ("LC_ALL", "C.UTF-8"), ("TZ", "UTC")]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        Self {
            isolation: Isolation::Auto,
            network: false,
            inherit_env: false,
            env,
            timeout_secs: 0,
        }
    }
}

/// One input to record: arguments, stdin, files in the working folder and extra environment.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CaseInput {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub stdin: String,
    /// Relative path → content, written before the run.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub files: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// Appends cases to cases.toml as `[[case]]` blocks, skipping names it already has. Returns how many were added.
pub fn append_cases(layout: &Layout, cases: &[CaseInput], comment: &str) -> Result<usize> {
    #[derive(Serialize)]
    struct Out<'a> {
        case: Vec<&'a CaseInput>,
    }
    let existing: BTreeSet<String> = if layout.cases_file().exists() {
        layout.load_cases()?.into_iter().map(|c| c.name).collect()
    } else {
        BTreeSet::new()
    };
    let new: Vec<&CaseInput> = cases
        .iter()
        .filter(|c| !existing.contains(&c.name))
        .collect();
    if new.is_empty() {
        return Ok(0);
    }
    let mut text = std::fs::read_to_string(layout.cases_file()).unwrap_or_default();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str(&format!("\n# {comment}\n"));
    text.push_str(&toml::to_string(&Out { case: new.clone() })?);
    std::fs::write(layout.cases_file(), text)?;
    Ok(new.len())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CasesFile {
    #[serde(rename = "case", default)]
    cases: Vec<CaseInput>,
}

pub struct Layout {
    pub root: PathBuf,
}

impl Layout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn project_file(&self) -> PathBuf {
        self.root.join(PROJECT_FILE)
    }
    pub fn cases_file(&self) -> PathBuf {
        self.root.join(CASES_FILE)
    }
    pub fn corpus_dir(&self) -> PathBuf {
        self.root.join(CORPUS_DIR)
    }
    pub fn case_dir(&self, name: &str) -> PathBuf {
        self.corpus_dir().join(name)
    }

    pub fn load_project(&self) -> Result<Project> {
        let path = self.project_file();
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("{} not found: run `parity init` first", path.display()))?;
        toml::from_str(&text).with_context(|| format!("{}", path.display()))
    }

    pub fn load_cases(&self) -> Result<Vec<CaseInput>> {
        let path = self.cases_file();
        let text = std::fs::read_to_string(&path).with_context(|| format!("{}", path.display()))?;
        let file: CasesFile =
            toml::from_str(&text).with_context(|| format!("{}", path.display()))?;
        let mut seen = BTreeSet::new();
        for case in &file.cases {
            check_case_name(&case.name)?;
            if !seen.insert(case.name.as_str()) {
                bail!("{}: case {:?} is defined twice", path.display(), case.name);
            }
            for file in case.files.keys() {
                check_relative(file).with_context(|| format!("case {:?}", case.name))?;
            }
        }
        Ok(file.cases)
    }
}

/// Case names become folder names: letters, digits, `.`, `_` and `-`, not starting with `.`.
pub fn check_case_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if !ok {
        bail!("case name {name:?} must be letters, digits, '.', '_' or '-'");
    }
    Ok(())
}

/// A path inside the working folder: relative, without `..`.
pub fn check_relative(path: &str) -> Result<()> {
    let p = Path::new(path);
    let ok = !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)));
    if !ok {
        bail!("file {path:?} must be a relative path without '..'");
    }
    Ok(())
}

pub const PROJECT_TEMPLATE: &str = r#"# The program being rewritten. Its recorded behaviour is the reference.
[original]
program = "{original}"

# The rewrite to check against the recordings (or pass --candidate to `parity replay`).
# [candidate]
# program = "./target/release/{original}"

# Defaults keep recordings reproducible. Every one can be turned off.
[sandbox]
isolation = "auto"      # "bwrap", or "none" to run against the real system
network = false
inherit_env = false
timeout_secs = 0        # 0 = no limit
env = { LANG = "C.UTF-8", LC_ALL = "C.UTF-8", TZ = "UTC" }

# Normalizers hide honest nondeterminism before comparing. Declare each one; nothing is ignored silently.
# [[normalize]]
# name = "iso-timestamp"
# pattern = '\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d+)?(Z|[+-]\d{2}:?\d{2})?'
# replace = "<TIMESTAMP>"
# applies_to = ["stdout", "stderr", "files"]
"#;

pub const CASES_TEMPLATE: &str = r#"# Each case is one run: arguments, stdin, files in the working folder and extra environment.
[[case]]
name = "version"
args = ["--version"]

# [[case]]
# name = "example"
# args = ["input.txt"]
# stdin = ""
# files = { "input.txt" = "hello\n" }
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn case_names_are_safe_folder_names() {
        assert!(check_case_name("format-select_1.2").is_ok());
        assert!(check_case_name("").is_err());
        assert!(check_case_name(".hidden").is_err());
        assert!(check_case_name("a/b").is_err());
    }

    #[test]
    fn fixture_paths_stay_inside_the_work_folder() {
        assert!(check_relative("dir/file.txt").is_ok());
        assert!(check_relative("../escape").is_err());
        assert!(check_relative("/etc/passwd").is_err());
        assert!(check_relative("a/../../b").is_err());
    }

    #[test]
    fn templates_parse() {
        let project: Project =
            toml::from_str(&PROJECT_TEMPLATE.replace("{original}", "jq")).unwrap();
        assert_eq!(project.original.program, "jq");
        assert!(!project.sandbox.network);
        let cases: CasesFile = toml::from_str(CASES_TEMPLATE).unwrap();
        assert_eq!(cases.cases[0].args, ["--version"]);
    }
}
