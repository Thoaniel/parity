//! Following the original's releases. `parity track` asks the upstream (PyPI, GitHub, npm, crates.io) for the
//! newest version; when it changed it installs it (when `[upstream] install` says how), keeps the old corpus in
//! `.parity/corpus-<old version>`, re-records every case against the new original, replays the rewrite and
//! reports what the release changed. `--install-timer` runs that daily with a systemd user timer.

use crate::project::{Layout, Upstream, UpstreamSource};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Default, Deserialize, Serialize)]
struct State {
    version: Option<String>,
    checked: u64,
}

fn state_file(layout: &Layout) -> PathBuf {
    layout.root.join(".parity/upstream.json")
}

pub fn latest(up: &Upstream) -> Result<String> {
    let agent = ureq::AgentBuilder::new()
        .user_agent(concat!(
            "parity/",
            env!("CARGO_PKG_VERSION"),
            " (https://github.com/Thoaniel/parity)"
        ))
        .timeout(std::time::Duration::from_secs(30))
        .build();
    let get = |url: String| -> Result<serde_json::Value> {
        agent
            .get(&url)
            .call()
            .with_context(|| url.clone())?
            .into_json()
            .with_context(|| url)
    };
    let v =
        match up.source {
            UpstreamSource::Pypi => {
                get(format!("https://pypi.org/pypi/{}/json", up.name))?["info"]["version"].clone()
            }
            UpstreamSource::Github => get(format!(
                "https://api.github.com/repos/{}/releases/latest",
                up.name
            ))?["tag_name"]
                .clone(),
            UpstreamSource::Npm => {
                get(format!("https://registry.npmjs.org/{}/latest", up.name))?["version"].clone()
            }
            UpstreamSource::Crates => get(format!("https://crates.io/api/v1/crates/{}", up.name))?
                ["crate"]["max_stable_version"]
                .clone(),
        };
    v.as_str()
        .map(str::to_string)
        .context("the upstream answered without a version")
}

pub fn track(layout: &Layout, force_rerecord: bool) -> Result<()> {
    let project = layout.load_project()?;
    let up = project
        .upstream
        .as_ref()
        .context("parity.toml has no [upstream] section")?;
    let latest = latest(up)?;
    let path = state_file(layout);
    let mut state: State = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let changed = state.version.as_deref() != Some(latest.as_str());
    let first = state.version.is_none();

    if !changed && !force_rerecord {
        println!("{} {latest}: up to date", up.name);
    } else if first && !force_rerecord {
        println!("{} {latest}: tracking from now on", up.name);
    } else {
        println!(
            "{} {} → {latest}",
            up.name,
            state.version.as_deref().unwrap_or("?")
        );
        if !up.install.is_empty() {
            let argv: Vec<String> = up
                .install
                .iter()
                .map(|a| a.replace("{version}", &latest))
                .collect();
            let status = Command::new(&argv[0])
                .args(&argv[1..])
                .current_dir(&layout.root)
                .status()?;
            if !status.success() {
                bail!("installing {latest} failed ({status})");
            }
        } else if !force_rerecord {
            println!(
                "no [upstream] install command: install {latest}, then run `parity track --rerecord`"
            );
            notify(
                &format!("{} {latest} is out", up.name),
                "Install it and run parity track --rerecord",
            );
            state.checked = now;
            save(&path, &state)?;
            return Ok(());
        }
        let old = state.version.clone().unwrap_or_else(|| "previous".into());
        let corpus = layout.corpus_dir();
        if corpus.exists() {
            let keep = layout.root.join(format!(".parity/corpus-{old}"));
            if keep.exists() {
                std::fs::remove_dir_all(&keep)?;
            }
            copy_dir(&corpus, &keep)?;
        }
        let status = Command::new(std::env::current_exe()?)
            .args(["record", "-C"])
            .arg(&layout.root)
            .status()?;
        if !status.success() {
            bail!("re-recording against {latest} failed");
        }
        match crate::replay_report(layout, None, &[], false) {
            Ok(report) => {
                let text = report.render(40);
                std::fs::write(layout.root.join(".parity/upstream-report.txt"), &text)?;
                let summary = format!(
                    "{}/{} cases still match after {latest}",
                    report.matching(),
                    report.cases.len()
                );
                println!("{summary}\nreport: .parity/upstream-report.txt");
                notify(&format!("{} {latest}", up.name), &summary);
            }
            Err(e) => println!("re-recorded; no candidate to replay yet ({e:#})"),
        }
    }
    state.version = Some(latest);
    state.checked = now;
    save(&path, &state)
}

fn save(path: &std::path::Path, state: &State) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, serde_json::to_vec_pretty(state)?)?;
    Ok(())
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> Result<()> {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry?;
        let target = to.join(entry.path().strip_prefix(from)?);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "Parity", title, body])
        .status();
}

/// A daily systemd user timer running `parity track` for this project.
pub fn install_timer(layout: &Layout) -> Result<()> {
    let root = std::fs::canonicalize(&layout.root)?;
    let name: String = format!(
        "parity-track-{}",
        crate::corpus::sha256(root.display().to_string().as_bytes())
            .get(..10)
            .unwrap_or("x")
    );
    let units = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME")?
        .join(".config/systemd/user");
    std::fs::create_dir_all(&units)?;
    let exe = std::env::current_exe()?;
    std::fs::write(
        units.join(format!("{name}.service")),
        format!(
            "[Unit]\nDescription=Parity: follow upstream releases for {root}\n\n\
             [Service]\nType=oneshot\nExecStart={exe} track -C {root}\n",
            root = root.display(),
            exe = exe.display()
        ),
    )?;
    std::fs::write(
        units.join(format!("{name}.timer")),
        "[Unit]\nDescription=Parity: daily upstream check\n\n\
         [Timer]\nOnCalendar=daily\nRandomizedDelaySec=1h\nPersistent=true\n\n\
         [Install]\nWantedBy=timers.target\n",
    )?;
    let ok = Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?
        .success()
        && Command::new("systemctl")
            .args(["--user", "enable", "--now", &format!("{name}.timer")])
            .status()?
            .success();
    if !ok {
        bail!("systemctl couldn't enable {name}.timer");
    }
    println!("{name}.timer: daily `parity track -C {}`", root.display());
    Ok(())
}
