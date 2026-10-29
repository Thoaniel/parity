//! The flight recorder: `parity recorder` samples the kernel's pressure stall information (/proc/pressure) and
//! the busiest processes several times a second into a rolling window of minute files; `parity why WHEN` finds
//! the worst stall around that moment and says in plain words what the machine was waiting on and who caused it.
//!
//! This first version reads /proc (no privileges, a few milliseconds per sample). An eBPF backend with scheduler
//! and block-I/O latency per process comes next, behind the same files and the same `why`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TICKS: f64 = 100.0;

#[derive(Debug, Default, Deserialize, Serialize)]
struct Sample {
    /// Milliseconds since the epoch.
    t: u64,
    /// Time some task stalled on CPU, I/O and memory since the previous sample, in µs.
    cpu: u64,
    io: u64,
    mem: u64,
    /// Interval since the previous sample, in ms.
    dt: u64,
    /// Busiest processes this interval: name, CPU %, bytes read, bytes written.
    top: Vec<(String, f32, u64, u64)>,
}

fn dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .context("HOME is not set")?;
    Ok(base.join("parity/recorder"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// `some total=` of a /proc/pressure file, in µs.
fn pressure(kind: &str) -> u64 {
    std::fs::read_to_string(format!("/proc/pressure/{kind}"))
        .ok()
        .and_then(|t| {
            let line = t.lines().find(|l| l.starts_with("some"))?;
            line.split_whitespace()
                .find_map(|f| f.strip_prefix("total=")?.parse().ok())
        })
        .unwrap_or(0)
}

/// Per pid: name, CPU ticks, bytes read and written (own processes only for I/O).
fn processes() -> BTreeMap<u32, (String, u64, u64, u64)> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return out;
    };
    for e in entries.filter_map(|e| e.ok()) {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            continue;
        };
        let Some((head, rest)) = stat.rsplit_once(')') else {
            continue;
        };
        let name = head.split_once('(').map_or("?", |(_, n)| n).to_string();
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
            + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        let (mut rd, mut wr) = (0, 0);
        if let Ok(io) = std::fs::read_to_string(format!("/proc/{pid}/io")) {
            for l in io.lines() {
                if let Some(v) = l.strip_prefix("read_bytes: ") {
                    rd = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = l.strip_prefix("write_bytes: ") {
                    wr = v.trim().parse().unwrap_or(0);
                }
            }
        }
        out.insert(pid, (name, ticks, rd, wr));
    }
    out
}

pub fn run(interval_ms: u64, window_secs: u64) -> Result<()> {
    if !std::path::Path::new("/proc/pressure/cpu").exists() {
        bail!("this kernel has no /proc/pressure (PSI); boot with psi=1");
    }
    let dir = dir()?;
    std::fs::create_dir_all(&dir)?;
    let mut prev = (
        pressure("cpu"),
        pressure("io"),
        pressure("memory"),
        now_ms(),
        processes(),
    );
    loop {
        std::thread::sleep(Duration::from_millis(interval_ms));
        let (cpu, io, mem, t, procs) = (
            pressure("cpu"),
            pressure("io"),
            pressure("memory"),
            now_ms(),
            processes(),
        );
        let dt = (t - prev.3).max(1);
        let mut top: Vec<(String, f32, u64, u64)> = procs
            .iter()
            .filter_map(|(pid, (name, ticks, rd, wr))| {
                let (_, pt, prd, pwr) = prev.4.get(pid)?;
                let cpu = (ticks.saturating_sub(*pt)) as f64 / TICKS / (dt as f64 / 1000.0) * 100.0;
                let (r, w) = (rd.saturating_sub(*prd), wr.saturating_sub(*pwr));
                (cpu >= 5.0 || r + w >= 1 << 20).then(|| (name.clone(), cpu as f32, r, w))
            })
            .collect();
        top.sort_by(|a, b| {
            (b.1 as f64 + (b.2 + b.3) as f64 / 1e7)
                .total_cmp(&(a.1 as f64 + (a.2 + a.3) as f64 / 1e7))
        });
        top.truncate(6);
        let s = Sample {
            t,
            cpu: cpu.saturating_sub(prev.0),
            io: io.saturating_sub(prev.1),
            mem: mem.saturating_sub(prev.2),
            dt,
            top,
        };
        let minute = t / 60_000;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{minute}.jsonl")))?;
        writeln!(f, "{}", serde_json::to_string(&s)?)?;
        if t % 60_000 < interval_ms {
            prune(&dir, minute, window_secs);
        }
        prev = (cpu, io, mem, t, procs);
    }
}

fn prune(dir: &std::path::Path, minute: u64, window_secs: u64) {
    let keep = (window_secs / 60).max(1) + 1;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.filter_map(|e| e.ok()) {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(m) = name
                .strip_suffix(".jsonl")
                .and_then(|m| m.parse::<u64>().ok())
                && m + keep < minute
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// `now`, `90s`, `2m`, `2 minutes ago`, `1h ago`, or a clock time `HH:MM[:SS]` today (local time via `date`).
pub fn parse_when(text: &str) -> Result<u64> {
    let t = text.trim().to_lowercase();
    if t == "now" {
        return Ok(now_ms());
    }
    let t = t.trim_end_matches(" ago").trim();
    let (num, unit): (String, String) = (
        t.chars().take_while(|c| c.is_ascii_digit()).collect(),
        t.chars()
            .skip_while(|c| c.is_ascii_digit())
            .collect::<String>()
            .trim()
            .to_string(),
    );
    if !num.is_empty() && !unit.contains(':') {
        let n: u64 = num.parse()?;
        let secs = match unit.as_str() {
            "" | "s" | "sec" | "secs" | "second" | "seconds" => n,
            "m" | "min" | "mins" | "minute" | "minutes" => n * 60,
            "h" | "hour" | "hours" => n * 3600,
            other => bail!("unit {other:?}: seconds, minutes or hours"),
        };
        return Ok(now_ms().saturating_sub(secs * 1000));
    }
    if t.contains(':') {
        let out = std::process::Command::new("date")
            .args(["-d", t, "+%s"])
            .output()?;
        let secs: u64 = String::from_utf8_lossy(&out.stdout)
            .trim()
            .parse()
            .context("not a time `date` understands")?;
        return Ok(secs * 1000);
    }
    bail!("when: now, 90s, 2 minutes ago, or 14:05")
}

pub fn why(when: &str, span_secs: u64) -> Result<()> {
    let at = parse_when(when)?;
    let (from, to) = (at.saturating_sub(span_secs * 1000), at + span_secs * 1000);
    let dir = dir()?;
    let mut samples = Vec::new();
    for minute in from / 60_000..=to / 60_000 {
        let Ok(f) = std::fs::File::open(dir.join(format!("{minute}.jsonl"))) else {
            continue;
        };
        for line in std::io::BufReader::new(f).lines().map_while(Result::ok) {
            if let Ok(s) = serde_json::from_str::<Sample>(&line)
                && (from..=to).contains(&s.t)
            {
                samples.push(s);
            }
        }
    }
    if samples.is_empty() {
        bail!(
            "no recording around then: is `parity recorder` running? (`parity recorder --install`)"
        );
    }
    let share = |stall: u64, dt: u64| stall as f64 / (dt as f64 * 1000.0) * 100.0;
    let worst = samples
        .iter()
        .max_by(|a, b| {
            let w = |s: &Sample| share(s.cpu.max(s.io).max(s.mem), s.dt);
            w(a).total_cmp(&w(b))
        })
        .unwrap();
    let (cpu, io, mem) = (
        share(worst.cpu, worst.dt),
        share(worst.io, worst.dt),
        share(worst.mem, worst.dt),
    );
    let clock = std::process::Command::new("date")
        .args(["-d", &format!("@{}", worst.t / 1000), "+%H:%M:%S"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    if cpu.max(io).max(mem) < 5.0 {
        println!(
            "Nothing stalled between {}s before and after {when}: worst moment {clock}, under 5 % waiting.",
            span_secs
        );
        return Ok(());
    }
    let (what, pct) = [
        ("I/O (disk)", io),
        ("CPU time", cpu),
        ("memory (reclaim or swap)", mem),
    ]
    .into_iter()
    .max_by(|a, b| a.1.total_cmp(&b.1))
    .unwrap();
    println!(
        "At {clock}, tasks spent {pct:.0} % of {} ms waiting on {what}.",
        worst.dt
    );
    if io >= 5.0 && what != "I/O (disk)" {
        println!("I/O waits were also {io:.0} %.");
    }
    let mb = |b: u64| b as f64 / 1_048_576.0 / (worst.dt as f64 / 1000.0);
    for (name, cpu, rd, wr) in &worst.top {
        let mut parts = Vec::new();
        if *cpu >= 5.0 {
            parts.push(format!("{cpu:.0} % CPU"));
        }
        if *rd > 0 {
            parts.push(format!("reading {:.1} MB/s", mb(*rd)));
        }
        if *wr > 0 {
            parts.push(format!("writing {:.1} MB/s", mb(*wr)));
        }
        println!("  {name}: {}", parts.join(", "));
    }
    Ok(())
}

/// A systemd user service that keeps the recorder running.
pub fn install() -> Result<()> {
    let units = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME")?
        .join(".config/systemd/user");
    std::fs::create_dir_all(&units)?;
    let exe = std::env::current_exe()?;
    std::fs::write(
        units.join("parity-recorder.service"),
        format!(
            "[Unit]\nDescription=Parity flight recorder\n\n[Service]\nExecStart={} recorder\nNice=10\n\
             Restart=on-failure\n\n[Install]\nWantedBy=default.target\n",
            exe.display()
        ),
    )?;
    let ok = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()?
        .success()
        && std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "parity-recorder.service"])
            .status()?
            .success();
    if !ok {
        bail!("systemctl couldn't enable parity-recorder.service");
    }
    println!("parity-recorder.service is running; ask with `parity why \"2 minutes ago\"`");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_relative_times() {
        let now = now_ms();
        let two_min = parse_when("2 minutes ago").unwrap();
        assert!((now - two_min).abs_diff(120_000) < 2_000);
        assert!((now - parse_when("90s").unwrap()).abs_diff(90_000) < 2_000);
        assert!(parse_when("5 fortnights").is_err());
    }
}
