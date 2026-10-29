//! `parity suggest`: what on this machine is worth rewriting first. Reads /proc for the user's processes, groups
//! them by program (the script for interpreters), classifies the runtime, and ranks by expected gain: resident
//! memory and CPU, weighted by how much a Rust rewrite typically saves for that runtime. Programs already in Rust
//! are left out.

use anyhow::Result;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
struct Group {
    rss_kb: u64,
    cpu_percent: f64,
    instances: usize,
    runtime: &'static str,
    kind: &'static str,
}

const TICKS: f64 = 100.0;

fn read(p: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(p).ok()
}

/// The runtime and how much a rewrite typically gains for it.
fn runtime(
    exe: &Path,
    cmdline: &[String],
    cache: &mut BTreeMap<PathBuf, &'static str>,
) -> &'static str {
    let base = exe
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if base.starts_with("python") {
        return "python";
    }
    if matches!(base.as_str(), "node" | "nodejs" | "bun" | "deno") {
        return "node";
    }
    if base == "java" {
        return "jvm";
    }
    if base.starts_with("php") {
        return "php";
    }
    if base == "dotnet" || base.starts_with("mono") {
        return "dotnet";
    }
    if base.starts_with("ruby") {
        return "ruby";
    }
    if cmdline.iter().any(|a| {
        a.starts_with("--type=renderer") || a.starts_with("--type=zygote") || a.contains("app.asar")
    }) {
        return "electron";
    }
    cache.entry(exe.to_path_buf()).or_insert_with(|| {
        let Ok(data) = std::fs::read(exe) else {
            return "native";
        };
        let hints = parity_rea::overview(&data)
            .map(|o| o.hints)
            .unwrap_or_default();
        let has = |h: &str| hints.iter().any(|x| x == h);
        if has("electron") {
            "electron"
        } else if has("rust") {
            "rust"
        } else if has("go") {
            "go"
        } else if has("python-embedded") || has("pyinstaller") {
            "python"
        } else if has("node-or-v8") {
            "node"
        } else if has("jvm") {
            "jvm"
        } else if has("dotnet") {
            "dotnet"
        } else {
            "native"
        }
    })
}

fn weight(runtime: &str) -> f64 {
    match runtime {
        "electron" => 4.0,
        "python" | "node" | "ruby" | "php" => 3.0,
        "jvm" => 2.5,
        "dotnet" => 2.0,
        "go" => 1.2,
        "native" => 0.8,
        _ => 0.0,
    }
}

fn kind(pid: &str) -> &'static str {
    let cgroup = read(format!("/proc/{pid}/cgroup")).unwrap_or_default();
    if cgroup.contains("/app-") || cgroup.contains(".scope") && cgroup.contains("app") {
        "app"
    } else if cgroup.contains(".service") {
        "service"
    } else {
        "cli"
    }
}

pub fn suggest(limit: usize) -> Result<()> {
    let uptime: f64 = read("/proc/uptime")
        .and_then(|u| u.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0);
    let me = read("/proc/self/status")
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))?
                .split_whitespace()
                .nth(1)
                .map(str::to_string)
        })
        .unwrap_or_default();
    let mut cache = BTreeMap::new();
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    for entry in std::fs::read_dir("/proc")?.filter_map(|e| e.ok()) {
        let pid = entry.file_name().to_string_lossy().into_owned();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Some(status) = read(format!("/proc/{pid}/status")) else {
            continue;
        };
        let field = |name: &str| {
            status
                .lines()
                .find(|l| l.starts_with(name))
                .map(|l| l[name.len()..].trim().to_string())
        };
        if field("Uid:").and_then(|u| u.split_whitespace().next().map(str::to_string))
            != Some(me.clone())
        {
            continue;
        }
        let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        let cmdline: Vec<String> = read(format!("/proc/{pid}/cmdline"))
            .unwrap_or_default()
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let rt = runtime(&exe, &cmdline, &mut cache);
        if rt == "rust" {
            continue;
        }
        // Interpreters: the program is the script they run, not the interpreter.
        let name = match rt {
            "python" | "node" | "ruby" | "php"
                if exe
                    .file_name()
                    .is_some_and(|n| !n.to_string_lossy().contains("electron")) =>
            {
                cmdline
                    .iter()
                    .skip(1)
                    .find(|a| !a.starts_with('-'))
                    .map(|a| {
                        Path::new(a)
                            .file_name()
                            .map_or(a.clone(), |n| n.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| exe.display().to_string())
            }
            _ => exe.file_name().map_or(exe.display().to_string(), |n| {
                n.to_string_lossy().into_owned()
            }),
        };
        let rss_kb: u64 = field("VmRSS:")
            .and_then(|v| v.split_whitespace().next()?.parse().ok())
            .unwrap_or(0);
        let stat = read(format!("/proc/{pid}/stat")).unwrap_or_default();
        // Fields after the command name, which may itself contain spaces and parentheses.
        let rest: Vec<&str> = stat
            .rsplit_once(')')
            .map(|(_, r)| r.split_whitespace().collect())
            .unwrap_or_default();
        let num = |i: usize| {
            rest.get(i)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let (utime, stime, start) = (num(11), num(12), num(19));
        let alive = (uptime - start / TICKS).max(1.0);
        let g = groups.entry(name).or_default();
        g.rss_kb += rss_kb;
        g.cpu_percent += (utime + stime) / TICKS / alive * 100.0;
        g.instances += 1;
        g.runtime = rt;
        if g.kind.is_empty() || kind(&pid) == "app" {
            g.kind = kind(&pid);
        }
    }
    let mut ranked: Vec<(f64, String, Group)> = groups
        .into_iter()
        .map(|(name, g)| {
            let score = (g.rss_kb as f64 / 1024.0 + g.cpu_percent * 20.0) * weight(g.runtime);
            (score, name, g)
        })
        .filter(|(score, _, _)| *score > 0.0)
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    println!(
        "{:>7}  {:<28} {:<9} {:<8} {:>8} {:>6} {:>4}",
        "score", "program", "runtime", "kind", "memory", "cpu", "n"
    );
    for (score, name, g) in ranked.into_iter().take(limit) {
        println!(
            "{score:>7.0}  {:<28} {:<9} {:<8} {:>6} MB {:>5.1}% {:>4}",
            name.chars().take(28).collect::<String>(),
            g.runtime,
            g.kind,
            g.rss_kb / 1024,
            g.cpu_percent,
            g.instances
        );
    }
    println!(
        "\nscore = (memory MB + 20 × average CPU %) × runtime weight (electron 4, python/node/ruby/php 3, jvm 2.5, \
              dotnet 2, go 1.2, native 0.8). CLIs are the easiest to prove; services next; apps need GUI parity."
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpreters_and_chromium_children_are_classified_without_reading_files() {
        let mut cache = BTreeMap::new();
        assert_eq!(
            runtime(Path::new("/usr/bin/python3.13"), &[], &mut cache),
            "python"
        );
        assert_eq!(runtime(Path::new("/usr/bin/node"), &[], &mut cache), "node");
        let renderer = vec!["/opt/x/x".to_string(), "--type=renderer".to_string()];
        assert_eq!(
            runtime(Path::new("/opt/x/x"), &renderer, &mut cache),
            "electron"
        );
        assert!(weight("electron") > weight("python") && weight("rust") == 0.0);
    }

    #[test]
    fn suggests_on_this_machine() {
        suggest(3).unwrap();
    }
}
