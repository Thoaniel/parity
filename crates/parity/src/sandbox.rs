//! Runs one case of a program and observes it: stdout, stderr, how it exited and every file in its working folder
//! afterwards. The working folder is a fresh temporary folder; inside bubblewrap it is mounted at `/work`, and
//! outside the host path is rewritten to `/work`, so recordings look the same either way.

use crate::project::{CaseInput, Isolation, Program, SandboxConfig};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const WORK: &str = "/work";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Exit {
    Code(i32),
    Signal(i32),
    Timeout,
}

impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Exit::Code(c) => write!(f, "exit {c}"),
            Exit::Signal(s) => write!(f, "signal {s}"),
            Exit::Timeout => write!(f, "timeout"),
        }
    }
}

/// What one run did.
#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit: Exit,
    /// Every regular file in the working folder after the run, by relative path. Symlinks are kept as
    /// `-> target`.
    pub files: BTreeMap<String, Vec<u8>>,
    pub duration: Duration,
}

pub fn run(program: &Program, case: &CaseInput, cfg: &SandboxConfig) -> Result<Observed> {
    let exe = resolve(&program.program)?;
    let dir = tempfile::Builder::new().prefix("parity-work-").tempdir()?;
    let work = dir.path().to_path_buf();
    for (path, content) in &case.files {
        let target = work.join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
    }
    std::fs::create_dir_all(work.join(".home"))?;

    let isolation = match cfg.isolation {
        Isolation::Auto if resolve("bwrap").is_ok() => Isolation::Bwrap,
        Isolation::Auto => Isolation::None,
        other => other,
    };
    let args: Vec<&String> = program.args.iter().chain(&case.args).collect();
    let (home, mut cmd) = match isolation {
        Isolation::Bwrap => (
            format!("{WORK}/.home"),
            bwrap(&exe, &args, &work, cfg, &case.env)?,
        ),
        _ => {
            let mut cmd = Command::new(&exe);
            cmd.args(&args).current_dir(&work);
            if !cfg.inherit_env {
                cmd.env_clear();
            }
            (work.join(".home").display().to_string(), cmd)
        }
    };
    if isolation != Isolation::Bwrap {
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        cmd.env("HOME", &home);
        cmd.envs(&cfg.env).envs(&case.env);
    }

    let start = Instant::now();
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", exe.display()))?;
    let stdin = child.stdin.take();
    let input = case.stdin.clone().into_bytes();
    // A program that never reads stdin gets EPIPE here; that is its behaviour, not an error.
    let feeder = std::thread::spawn(move || {
        if let Some(mut s) = stdin {
            let _ = s.write_all(&input);
        }
    });
    let stdout = reader(child.stdout.take());
    let stderr = reader(child.stderr.take());
    let exit = wait(&mut child, cfg.timeout_secs)?;
    let duration = start.elapsed();
    let _ = feeder.join();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();

    let host = work.display().to_string();
    let rewrite = |bytes: Vec<u8>| {
        if isolation == Isolation::Bwrap {
            bytes
        } else {
            replace(&bytes, host.as_bytes(), WORK.as_bytes())
        }
    };
    let files = snapshot(&work)?
        .into_iter()
        .map(|(k, v)| (k, rewrite(v)))
        .collect();
    Ok(Observed {
        stdout: rewrite(stdout),
        stderr: rewrite(stderr),
        exit,
        files,
        duration,
    })
}

fn bwrap(
    exe: &Path,
    args: &[&String],
    work: &Path,
    cfg: &SandboxConfig,
    case_env: &BTreeMap<String, String>,
) -> Result<Command> {
    let mut cmd = Command::new(resolve("bwrap")?);
    cmd.args([
        "--die-with-parent",
        "--unshare-user",
        "--unshare-ipc",
        "--unshare-pid",
        "--unshare-uts",
    ]);
    cmd.arg("--unshare-cgroup-try");
    if !cfg.network {
        cmd.arg("--unshare-net");
    }
    cmd.args([
        "--ro-bind",
        "/",
        "/",
        "--dev",
        "/dev",
        "--proc",
        "/proc",
        "--tmpfs",
        "/tmp",
    ]);
    cmd.arg("--bind").arg(work).arg(WORK);
    cmd.args(["--chdir", WORK]);
    if !cfg.inherit_env {
        cmd.arg("--clearenv");
    }
    if let Ok(path) = std::env::var("PATH") {
        cmd.args(["--setenv", "PATH", &path]);
    }
    cmd.args(["--setenv", "HOME", &format!("{WORK}/.home")]);
    for (k, v) in cfg.env.iter().chain(case_env) {
        cmd.args(["--setenv", k, v]);
    }
    cmd.arg("--").arg(exe).args(args);
    Ok(cmd)
}

fn reader<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut p) = pipe {
            let _ = p.read_to_end(&mut out);
        }
        out
    })
}

fn wait(child: &mut Child, timeout_secs: u64) -> Result<Exit> {
    use std::os::unix::process::ExitStatusExt;
    let deadline = (timeout_secs > 0).then(|| Instant::now() + Duration::from_secs(timeout_secs));
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if deadline.is_some_and(|d| Instant::now() >= d) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(Exit::Timeout);
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    Ok(match (status.code(), status.signal()) {
        (Some(code), _) => Exit::Code(code),
        (None, Some(sig)) => Exit::Signal(sig),
        (None, None) => Exit::Code(-1),
    })
}

fn snapshot(work: &Path) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for entry in walkdir::WalkDir::new(work).min_depth(1).sort_by_file_name() {
        let entry = entry?;
        let rel = entry
            .path()
            .strip_prefix(work)?
            .to_string_lossy()
            .into_owned();
        let kind = entry.file_type();
        if kind.is_file() {
            files.insert(rel, std::fs::read(entry.path())?);
        } else if kind.is_symlink() {
            let target = std::fs::read_link(entry.path())?;
            files.insert(rel, format!("-> {}", target.display()).into_bytes());
        }
    }
    Ok(files)
}

/// A program name looked up on PATH, or a path as given.
pub fn resolve(program: &str) -> Result<PathBuf> {
    if program.contains('/') {
        let p = PathBuf::from(program);
        return std::path::absolute(&p).with_context(|| program.to_string());
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!("{program} is not on PATH")
}

fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    if from.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_rewrites_every_occurrence() {
        assert_eq!(
            replace(b"/tmp/x a /tmp/x", b"/tmp/x", b"/work"),
            b"/work a /work"
        );
        assert_eq!(replace(b"abc", b"", b"z"), b"abc");
    }

    #[test]
    fn exits_print_plainly() {
        assert_eq!(Exit::Code(2).to_string(), "exit 2");
        assert_eq!(Exit::Signal(9).to_string(), "signal 9");
        assert_eq!(Exit::Timeout.to_string(), "timeout");
    }
}
