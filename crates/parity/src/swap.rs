//! Swapping a rewrite in, shadowing it during real use, and rolling it back.
//!
//! `parity swap NAME` links `~/.local/bin/NAME` to a copy of Parity. Started under that name, Parity is a shim
//! (`shim_main`) that, by the swap's mode:
//! - `swapped`: execs the rewrite;
//! - `original`: execs the original (after a rollback, or while paused);
//! - `shadow`: runs the original for real (the user gets its output and exit status), then, in the background,
//!   runs the rewrite on the same arguments and stdin with the working folder as a throwaway overlay, compares, and
//!   logs. A divergence becomes a new case in the project, and one in exit status rolls the swap back to
//!   `original` at once.
//!
//! systemd units are swapped with a drop-in that points ExecStart at the rewrite. `parity revert` undoes either.

use crate::project::{CaseInput, Layout};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Shadow,
    Swapped,
    Original,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Swap {
    pub name: String,
    pub original: PathBuf,
    pub candidate: PathBuf,
    pub project: Option<PathBuf>,
    pub mode: Mode,
    pub since: u64,
    #[serde(default)]
    pub runs: u64,
    #[serde(default)]
    pub divergences: u64,
    /// The original was moved out of ~/.local/bin to make room for the shim; revert moves it back.
    #[serde(default)]
    pub moved_from: Option<PathBuf>,
    /// A systemd unit swapped with a drop-in, and whether it is a user unit.
    #[serde(default)]
    pub unit: Option<String>,
    #[serde(default)]
    pub user_unit: bool,
    /// Divergences found in shadow mode become cases (their arguments and stdin are saved, and the agent sees
    /// them). Off by default: real use can carry secrets.
    #[serde(default)]
    pub learn: bool,
    /// The shadow run of the rewrite may use the network. Off by default, so nothing is sent twice.
    #[serde(default)]
    pub shadow_network: bool,
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}
fn data_dir() -> Result<PathBuf> {
    Ok(std::env::var_os("XDG_DATA_HOME")
        .map_or(home()?.join(".local/share"), PathBuf::from)
        .join("parity"))
}
fn state_dir() -> Result<PathBuf> {
    Ok(std::env::var_os("XDG_STATE_HOME")
        .map_or(home()?.join(".local/state"), PathBuf::from)
        .join("parity"))
}
fn bin_dir() -> Result<PathBuf> {
    Ok(home()?.join(".local/bin"))
}
fn registry_path() -> Result<PathBuf> {
    Ok(data_dir()?.join("swaps.json"))
}

pub fn load() -> Result<Vec<Swap>> {
    let p = registry_path()?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    serde_json::from_slice(&std::fs::read(&p)?).with_context(|| format!("{}", p.display()))
}

fn save(swaps: &[Swap]) -> Result<()> {
    let p = registry_path()?;
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(swaps)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

fn update(name: &str, f: impl FnOnce(&mut Swap)) -> Result<()> {
    let mut swaps = load()?;
    if let Some(s) = swaps.iter_mut().find(|s| s.name == name) {
        f(s);
        save(&swaps)?;
    }
    Ok(())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The shim's program: a copy of this Parity, so rebuilding or cleaning the source tree never breaks a swap.
fn installed_parity() -> Result<PathBuf> {
    let target = data_dir()?.join("parity");
    let me = std::env::current_exe()?;
    if std::fs::canonicalize(&me).ok() != std::fs::canonicalize(&target).ok() {
        std::fs::create_dir_all(target.parent().unwrap())?;
        let tmp = target.with_extension("new");
        std::fs::copy(&me, &tmp)?;
        std::fs::rename(&tmp, &target)?;
    }
    Ok(target)
}

pub struct SwapRequest {
    pub name: String,
    pub original: PathBuf,
    pub candidate: PathBuf,
    pub project: Option<PathBuf>,
    pub mode: Mode,
    pub unit: Option<String>,
    pub system_unit: bool,
    pub learn: bool,
    pub shadow_network: bool,
}

pub fn swap(req: SwapRequest) -> Result<()> {
    let candidate = std::path::absolute(&req.candidate)?;
    if !candidate.is_file() {
        bail!(
            "{} doesn't exist: build the rewrite first",
            candidate.display()
        );
    }
    let mut swaps = load()?;
    if swaps.iter().any(|s| s.name == req.name) {
        bail!(
            "{} is already swapped: `parity swap-mode` changes its mode, `parity revert` undoes it",
            req.name
        );
    }
    let mut entry = Swap {
        name: req.name.clone(),
        original: std::path::absolute(&req.original)?,
        candidate: candidate.clone(),
        project: req.project.map(std::path::absolute).transpose()?,
        mode: req.mode,
        since: now(),
        runs: 0,
        divergences: 0,
        moved_from: None,
        unit: None,
        user_unit: !req.system_unit,
        learn: req.learn,
        shadow_network: req.shadow_network,
    };

    if let Some(unit) = req.unit {
        swap_unit(&unit, !req.system_unit, &entry.original, &candidate)?;
        entry.unit = Some(unit.clone());
        entry.mode = Mode::Swapped;
        println!(
            "{unit}: ExecStart now runs {} (drop-in parity.conf); restarted",
            candidate.display()
        );
    } else {
        let link = bin_dir()?.join(&req.name);
        let parity = installed_parity()?;
        if link.symlink_metadata().is_ok() {
            if std::fs::canonicalize(&link).ok() == std::fs::canonicalize(&entry.original).ok() {
                // The original lives where the shim goes (pipx, cargo install): keep it beside Parity.
                let kept = data_dir()?.join("originals").join(&req.name);
                std::fs::create_dir_all(kept.parent().unwrap())?;
                std::fs::rename(&link, &kept)?;
                entry.moved_from = Some(link.clone());
                entry.original = kept;
            } else {
                bail!(
                    "{} exists and is not {}: move it first",
                    link.display(),
                    entry.original.display()
                );
            }
        }
        std::fs::create_dir_all(bin_dir()?)?;
        std::os::unix::fs::symlink(&parity, &link)?;
        warn_path_order(&link, &entry.original);
        println!("{}: {} → shim ({:?})", req.name, link.display(), entry.mode);
    }
    swaps.push(entry);
    save(&swaps)
}

/// The shim only takes over when its folder comes before the original's on PATH.
fn warn_path_order(link: &Path, original: &Path) {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    let pos = |d: Option<&Path>| d.and_then(|d| dirs.iter().position(|x| x == d));
    match (pos(link.parent()), pos(original.parent())) {
        (Some(a), Some(b)) if a > b => eprintln!(
            "warning: {} comes after {} on PATH, so the original still runs; put ~/.local/bin first",
            link.parent().unwrap().display(),
            original.parent().unwrap().display()
        ),
        (None, _) => eprintln!(
            "warning: {} is not on PATH",
            link.parent().unwrap().display()
        ),
        _ => {}
    }
}

pub fn set_mode(name: &str, mode: Mode) -> Result<()> {
    if !load()?.iter().any(|s| s.name == name) {
        bail!("{name} is not swapped");
    }
    update(name, |s| s.mode = mode)
}

pub fn revert(name: &str) -> Result<()> {
    let mut swaps = load()?;
    let i = swaps
        .iter()
        .position(|s| s.name == name)
        .with_context(|| format!("{name} is not swapped"))?;
    let s = swaps.remove(i);
    if let Some(unit) = &s.unit {
        let dropin = unit_dropin(unit, s.user_unit)?;
        if dropin.exists() {
            std::fs::remove_file(&dropin)?;
        }
        systemctl(s.user_unit, &["daemon-reload"])?;
        systemctl(s.user_unit, &["restart", unit])?;
        println!("{unit}: back on the original");
    } else {
        let link = bin_dir()?.join(name);
        if link
            .symlink_metadata()
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            std::fs::remove_file(&link)?;
        }
        if let Some(from) = &s.moved_from {
            std::fs::rename(&s.original, from)?;
        }
        println!(
            "{name}: back on {}",
            s.moved_from.as_ref().unwrap_or(&s.original).display()
        );
    }
    save(&swaps)
}

pub fn list() -> Result<()> {
    let swaps = load()?;
    if swaps.is_empty() {
        println!("nothing swapped");
    }
    for s in swaps {
        println!(
            "{:<20} {:<9} {} runs, {} divergences{}",
            s.name,
            format!("{:?}", s.mode).to_lowercase(),
            s.runs,
            s.divergences,
            s.unit.map(|u| format!(", unit {u}")).unwrap_or_default()
        );
    }
    Ok(())
}

fn unit_dropin(unit: &str, user: bool) -> Result<PathBuf> {
    let base = if user {
        home()?.join(".config/systemd/user")
    } else {
        PathBuf::from("/etc/systemd/system")
    };
    Ok(base.join(format!("{unit}.d")).join("parity.conf"))
}

fn systemctl(user: bool, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("systemctl");
    if user {
        cmd.arg("--user");
    }
    let out = cmd.args(args).output().context("running systemctl")?;
    if !out.status.success() {
        bail!(
            "systemctl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Rewrites the unit's ExecStart lines with the original's path replaced by the rewrite's, as a drop-in.
fn swap_unit(unit: &str, user: bool, original: &Path, candidate: &Path) -> Result<()> {
    let fragment = systemctl(user, &["show", "-p", "FragmentPath", "--value", unit])?;
    let text =
        std::fs::read_to_string(fragment.trim()).with_context(|| format!("reading {unit}"))?;
    let original = original.display().to_string();
    let lines: Vec<String> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("ExecStart="))
        .map(|cmd| cmd.replace(&original, &candidate.display().to_string()))
        .collect();
    if lines.is_empty() || !text.contains(&original) {
        bail!("{unit}'s ExecStart doesn't run {original}");
    }
    let dropin = unit_dropin(unit, user)?;
    std::fs::create_dir_all(dropin.parent().unwrap())?;
    let body: String = lines.iter().map(|l| format!("ExecStart={l}\n")).collect();
    std::fs::write(
        &dropin,
        format!(
            "# Written by `parity swap`; `parity revert` removes it.\n[Service]\nExecStart=\n{body}"
        ),
    )?;
    systemctl(user, &["daemon-reload"])?;
    systemctl(user, &["restart", unit])?;
    Ok(())
}

// ---- the shim ---------------------------------------------------------------------------------------------

/// Stdin larger than this is passed through unshadowed (it can't be replayed without holding it).
const MAX_SHADOW_STDIN: usize = 64 << 20;

#[derive(Serialize, Deserialize)]
struct ShadowJob {
    name: String,
    args: Vec<String>,
    cwd: PathBuf,
    stdin: Vec<u8>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    exit: crate::sandbox::Exit,
}

/// Runs as `NAME args…` through a swap's link. Never returns on exec; returns the exit code otherwise.
pub fn shim_main(name: &str, args: Vec<String>) -> Result<i32> {
    use std::os::unix::process::CommandExt;
    let swap = load()?
        .into_iter()
        .find(|s| s.name == name)
        .with_context(|| {
            format!("parity: {name} is linked to Parity but not swapped; `parity revert {name}`")
        })?;
    let _ = update(name, |s| s.runs += 1);
    let exec = |program: &Path| -> Result<i32> {
        let err = Command::new(program).arg0(name).args(&args).exec();
        Err(err).with_context(|| format!("starting {}", program.display()))
    };
    match swap.mode {
        Mode::Swapped => exec(&swap.candidate),
        Mode::Original => exec(&swap.original),
        Mode::Shadow => {
            let interactive = std::io::stdout().is_terminal() || std::io::stderr().is_terminal();
            if interactive {
                // A terminal changes what programs print (colour, progress), so there is nothing fair to compare.
                return exec(&swap.original);
            }
            let stdin = if std::io::stdin().is_terminal() {
                Vec::new()
            } else {
                let mut buf = Vec::new();
                std::io::stdin()
                    .take(MAX_SHADOW_STDIN as u64 + 1)
                    .read_to_end(&mut buf)?;
                if buf.len() > MAX_SHADOW_STDIN {
                    // Too big to hold: run the original on what was read plus the rest, unshadowed.
                    let mut child = Command::new(&swap.original)
                        .arg0(name)
                        .args(&args)
                        .stdin(Stdio::piped())
                        .spawn()?;
                    let mut pipe = child.stdin.take().unwrap();
                    pipe.write_all(&buf)?;
                    std::io::copy(&mut std::io::stdin(), &mut pipe)?;
                    drop(pipe);
                    return Ok(child.wait()?.code().unwrap_or(1));
                }
                buf
            };
            let (stdout, stderr, exit) = tee_run(&swap.original, name, &args, &stdin)?;
            let job = ShadowJob {
                name: name.to_string(),
                args,
                cwd: std::env::current_dir()?,
                stdin,
                stdout,
                stderr,
                exit,
            };
            if let Err(e) = spawn_check(&job) {
                log_line(
                    name,
                    &serde_json::json!({ "time": now(), "error": format!("{e:#}") }),
                );
            }
            Ok(match exit {
                crate::sandbox::Exit::Code(c) => c,
                crate::sandbox::Exit::Signal(s) => 128 + s,
                crate::sandbox::Exit::Timeout => 124,
            })
        }
    }
}

/// Runs the original with the caller's stdout/stderr live, keeping copies.
fn tee_run(
    program: &Path,
    name: &str,
    args: &[String],
    stdin: &[u8],
) -> Result<(Vec<u8>, Vec<u8>, crate::sandbox::Exit)> {
    use std::os::unix::process::{CommandExt, ExitStatusExt};
    let mut child = Command::new(program)
        .arg0(name)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("starting {}", program.display()))?;
    let mut pipe = child.stdin.take().unwrap();
    let input = stdin.to_vec();
    let feeder = std::thread::spawn(move || {
        let _ = pipe.write_all(&input);
    });
    fn tee<R: Read + Send + 'static, W: Write + Send + 'static>(
        mut from: R,
        mut to: W,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut kept = Vec::new();
            let mut buf = [0u8; 16384];
            while let Ok(n) = from.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let _ = to.write_all(&buf[..n]);
                let _ = to.flush();
                kept.extend_from_slice(&buf[..n]);
            }
            kept
        })
    }
    let out = tee(child.stdout.take().unwrap(), std::io::stdout());
    let err = tee(child.stderr.take().unwrap(), std::io::stderr());
    let status = child.wait()?;
    let _ = feeder.join();
    let exit = match (status.code(), status.signal()) {
        (Some(c), _) => crate::sandbox::Exit::Code(c),
        (None, Some(s)) => crate::sandbox::Exit::Signal(s),
        _ => crate::sandbox::Exit::Code(-1),
    };
    Ok((
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default(),
        exit,
    ))
}

/// Hands the comparison to a detached `parity shadow-check JOB`, so the caller's command returns at once.
fn spawn_check(job: &ShadowJob) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let dir = state_dir()?.join("shadow/jobs");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!(
        "{}-{}-{}.json",
        job.name,
        now(),
        std::process::id()
    ));
    private_file(&file)?.write_all(&serde_json::to_vec(job)?)?;
    Command::new(data_dir()?.join("parity"))
        .arg0("parity")
        .arg("shadow-check")
        .arg(&file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    Ok(())
}

fn log_line(name: &str, value: &serde_json::Value) {
    if let Ok(dir) = state_dir().map(|d| d.join("shadow")) {
        let _ = std::fs::create_dir_all(&dir);
        if let Ok(mut f) = private_append(&dir.join(format!("{name}.jsonl"))) {
            let _ = writeln!(f, "{value}");
        }
    }
}

/// Shadow jobs and logs hold real arguments and stdin: readable by the user only.
fn private_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

fn private_append(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
}

/// `parity shadow-check JOB`: runs the rewrite like the original ran, compares, records divergences.
pub fn shadow_check(job_file: &Path) -> Result<()> {
    let job: ShadowJob = serde_json::from_slice(&std::fs::read(job_file)?)?;
    let _ = std::fs::remove_file(job_file);
    let swap = load()?
        .into_iter()
        .find(|s| s.name == job.name)
        .context("swap gone")?;
    let project = swap.project.as_ref().map(Layout::new);
    let loaded = project.as_ref().map(|l| l.load_project()).transpose()?;
    let rules = loaded
        .as_ref()
        .map(|p| p.normalize.clone())
        .unwrap_or_default();
    let norm = crate::normalize::Normalizer::new(&rules)?;

    let (stdout, stderr, exit) = overlay_run(&swap.candidate, &job, swap.shadow_network)?;
    let expected = crate::sandbox::Observed {
        stdout: job.stdout.clone(),
        stderr: job.stderr.clone(),
        exit: job.exit,
        files: Default::default(),
        duration: Default::default(),
    };
    let actual = crate::sandbox::Observed {
        stdout,
        stderr,
        exit,
        files: Default::default(),
        duration: Default::default(),
    };
    let divergences = crate::compare::compare(&expected, &actual, &norm);
    let fields: Vec<String> = divergences.iter().map(|d| d.field.to_string()).collect();
    log_line(
        &job.name,
        &serde_json::json!({ "time": now(), "args": job.args, "match": divergences.is_empty(), "fields": fields }),
    );
    if divergences.is_empty() {
        return Ok(());
    }
    let exit_differs = divergences
        .iter()
        .any(|d| d.field == crate::compare::Field::Exit);
    update(&job.name, |s| {
        s.divergences += 1;
        if exit_differs {
            s.mode = Mode::Original;
        }
    })?;
    if let Some(layout) = project.as_ref().filter(|_| swap.learn) {
        let case = CaseInput {
            name: format!("shadow-{}-{}", now(), std::process::id()),
            args: job.args.clone(),
            stdin: String::from_utf8_lossy(&job.stdin).into_owned(),
            ..Default::default()
        };
        crate::project::append_cases(layout, std::slice::from_ref(&case), "found by shadow mode")?;
        crate::corpus::write(
            &layout.case_dir(&case.name),
            &case,
            &expected,
            &swap.original.display().to_string(),
        )?;
    }
    let what = if exit_differs {
        "exit status differs: rolled back to the original"
    } else {
        "output differs"
    };
    let _ = Command::new("notify-send")
        .args([
            "-a",
            "Parity",
            &format!("{}: {what}", job.name),
            &format!("{} {}", job.name, job.args.join(" ")),
        ])
        .status();
    Ok(())
}

/// The rewrite in bubblewrap, the caller's working folder visible through a throwaway overlay (its writes
/// vanish), without network or session sockets unless the swap allows network.
fn overlay_run(
    candidate: &Path,
    job: &ShadowJob,
    shadow_network: bool,
) -> Result<(Vec<u8>, Vec<u8>, crate::sandbox::Exit)> {
    use std::os::unix::process::ExitStatusExt;
    let cwd = job.cwd.display().to_string();
    let mut cmd = Command::new(crate::sandbox::resolve("bwrap")?);
    cmd.args([
        "--die-with-parent",
        "--unshare-user",
        "--unshare-ipc",
        "--unshare-pid",
    ]);
    if !shadow_network {
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
    // No D-Bus, Wayland, PipeWire or agent sockets: the shadow run must not reach the session.
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        cmd.arg("--tmpfs").arg(runtime);
    }
    cmd.args([
        "--overlay-src",
        &cwd,
        "--tmp-overlay",
        &cwd,
        "--chdir",
        &cwd,
    ]);
    cmd.arg("--").arg(candidate).args(&job.args);
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut pipe = child.stdin.take().unwrap();
    let input = job.stdin.clone();
    let feeder = std::thread::spawn(move || {
        let _ = pipe.write_all(&input);
    });
    let out = child.wait_with_output()?;
    let _ = feeder.join();
    let exit = match (out.status.code(), out.status.signal()) {
        (Some(c), _) => crate::sandbox::Exit::Code(c),
        (None, Some(s)) => crate::sandbox::Exit::Signal(s),
        _ => crate::sandbox::Exit::Code(-1),
    };
    Ok((out.stdout, out.stderr, exit))
}
