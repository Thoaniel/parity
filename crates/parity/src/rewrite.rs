//! The rewrite loop: build the Rust crate, replay the corpus against it, hand the agent the next failing cases
//! (or the build errors), and repeat until everything matches, the budget runs out, or progress stalls.
//!
//! Parity owns the judge: the agent only edits the crate. Each iteration is logged to `.parity/history.jsonl`.

use crate::ledger::Ledger;
use crate::project::{Layout, Program, Project};
use crate::report::Report;
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

pub struct Options {
    pub max_iterations: Option<u32>,
    /// Write the task and print the agent command without running it.
    pub dry_run: bool,
}

#[derive(Serialize)]
struct Step {
    iteration: u32,
    unix_time: u64,
    built: bool,
    matching: usize,
    total: usize,
    agent_seconds: u64,
}

/// The crate folder and the binary path the candidate runs from.
pub fn candidate(layout: &Layout, project: &Project) -> (PathBuf, PathBuf) {
    let dir = layout.root.join(&project.rewrite.dir);
    let bin = binary_name(project);
    let path = dir.join("target/release").join(&bin);
    (dir, path)
}

fn binary_name(project: &Project) -> String {
    project.rewrite.binary.clone().unwrap_or_else(|| {
        Path::new(&project.original.program)
            .file_name()
            .map_or_else(|| "candidate".into(), |n| n.to_string_lossy().into_owned())
    })
}

/// Creates the crate when it doesn't exist: a binary named like the original that fails every case honestly.
pub fn scaffold(layout: &Layout, project: &Project) -> Result<bool> {
    let (dir, _) = candidate(layout, project);
    if dir.join("Cargo.toml").exists() {
        return Ok(false);
    }
    let bin = binary_name(project);
    let package: String = bin
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    std::fs::create_dir_all(dir.join("src"))?;
    std::fs::write(
        dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{package}-rs\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [[bin]]\nname = \"{bin}\"\npath = \"src/main.rs\"\n\n[dependencies]\n\n\
             [profile.release]\nlto = \"thin\"\ncodegen-units = 1\n\n\
             # Its own workspace, so a project inside another Cargo workspace still builds alone.\n[workspace]\n"
        ),
    )?;
    std::fs::write(
        dir.join("src/main.rs"),
        format!(
            "// The Rust rewrite of {original}, written until `parity replay` reports no divergence.\n\n\
             fn main() {{\n    eprintln!(\"{bin}: not written yet\");\n    std::process::exit(1);\n}}\n",
            original = project.original.program
        ),
    )?;
    std::fs::write(dir.join(".gitignore"), "/target\n")?;
    Ok(true)
}

pub fn run(layout: &Layout, opts: &Options) -> Result<()> {
    let project = layout.load_project()?;
    if crate::corpus::list(&layout.corpus_dir())?.is_empty() {
        bail!("nothing recorded yet: run `parity record` first");
    }
    if scaffold(layout, &project)? {
        println!("created the crate in {}", project.rewrite.dir);
    }
    let (dir, bin) = candidate(layout, &project);
    let state = layout.root.join(".parity");
    std::fs::create_dir_all(&state)?;
    let max = opts
        .max_iterations
        .unwrap_or(project.rewrite.max_iterations);
    let mut best = 0usize;
    let mut stalled = 0u32;

    for iteration in 1..=max {
        let (built, build_log) = build(&dir, &project.rewrite.post_build)?;
        let (task, matching, total) = if !built {
            (build_task(layout, &project, &build_log), 0, 0)
        } else {
            let report = crate::replay_report(layout, Some(bin.display().to_string()), &[], false)?;
            let mut ledger = Ledger::load(layout)?;
            if ledger.sync_from_report(&report) > 0 {
                ledger.save(layout)?;
            }
            let (m, t) = (report.matching(), report.cases.len());
            println!("iteration {iteration}: {m}/{t} cases match");
            if m == t {
                log(&state, iteration, true, m, t, 0)?;
                println!("all {t} cases match: the rewrite is at parity with the corpus");
                return Ok(());
            }
            (cases_task(layout, &project, &report), m, t)
        };

        if built {
            if matching > best {
                best = matching;
                stalled = 0;
            } else {
                stalled += 1;
                if stalled > project.rewrite.stall_limit {
                    log(&state, iteration, built, matching, total, 0)?;
                    bail!(
                        "no progress for {} iterations ({best}/{total} match). Read .parity/task.md and the \
                         crate, adjust cases or normalizers, then run again",
                        project.rewrite.stall_limit
                    );
                }
            }
        }

        let task_file = state.join("task.md");
        std::fs::write(&task_file, &task)?;
        let argv = agent_command(&project, &task, &task_file)?;
        if opts.dry_run {
            println!(
                "task written to {}\nagent: {}",
                task_file.display(),
                argv.join(" ")
            );
            return Ok(());
        }
        let started = Instant::now();
        let status = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(&layout.root)
            .status()
            .with_context(|| format!("starting the agent ({})", argv[0]))?;
        let seconds = started.elapsed().as_secs();
        log(&state, iteration, built, matching, total, seconds)?;
        if !status.success() {
            bail!("the agent exited with {status}");
        }
    }
    println!("stopped after {max} iterations ({best} cases match); run again to continue");
    Ok(())
}

fn build(dir: &Path, post_build: &[String]) -> Result<(bool, String)> {
    let out = Command::new("cargo")
        .args(["build", "--release", "--quiet", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .output()
        .context("running cargo")?;
    let mut log = String::from_utf8_lossy(&out.stderr).into_owned();
    if !out.status.success() {
        return Ok((false, log));
    }
    if let Some((program, args)) = post_build.split_first() {
        let post = Command::new(program)
            .args(args)
            .current_dir(dir)
            .output()
            .context("running post_build")?;
        log.push_str(&String::from_utf8_lossy(&post.stderr));
        if !post.status.success() {
            return Ok((false, format!("post_build failed:\n{log}")));
        }
    }
    Ok((true, log))
}

const RULES: &str = "\
Rules:
- Edit only files inside the crate folder. Never edit cases.toml, parity.toml, corpus/ or .parity/.
- The rewrite must behave exactly like the original: same output bytes, same exit status, same files written.
  Normalizers in parity.toml already hide declared noise (timestamps and the like); everything else counts.
- Build with `cargo build --release` in the crate. Check single cases with
  `parity replay -C <project> --candidate <binary> --case <name>`; `parity show <name>` prints what the original did.
- To understand the original: `parity discover`, surface.json, `parity inspect <file>` (sections, symbols,
  strings, disassembly), and the original's source when it is a script.
- When a part of the original is done, mark it: `parity ledger set <id> ported` (see ledger.toml).
- Prefer general, faithful implementations over special-casing the recorded inputs.
";

fn header(layout: &Layout, project: &Project) -> String {
    let (dir, bin) = candidate(layout, project);
    format!(
        "# Parity task: rewrite `{}` in Rust\n\nProject: {}\nCrate: {}\nBinary: {}\n\n{RULES}\n",
        project.original.program,
        layout.root.display(),
        dir.display(),
        bin.display()
    )
}

fn build_task(layout: &Layout, project: &Project, log: &str) -> String {
    let tail: Vec<&str> = log.lines().rev().take(150).collect();
    let tail: Vec<&str> = tail.into_iter().rev().collect();
    format!(
        "{}## The crate doesn't build\n\nFix the build first. Last lines of `cargo build --release`:\n\n```\n{}\n```\n",
        header(layout, project),
        tail.join("\n")
    )
}

fn cases_task(layout: &Layout, project: &Project, report: &Report) -> String {
    let failing: Vec<_> = report
        .cases
        .iter()
        .filter(|c| !c.divergences.is_empty())
        .collect();
    let batch: Vec<_> = failing.iter().take(project.rewrite.batch.max(1)).collect();
    let mut s = header(layout, project);
    s.push_str(&format!(
        "## Status\n\n{}/{} cases match. {} still differ.\n\n",
        report.matching(),
        report.cases.len(),
        failing.len()
    ));
    if let Ok(ledger) = Ledger::load(layout)
        && !ledger.items.is_empty()
    {
        s.push_str(&format!("Ledger: {}\n", ledger.summary()));
    }
    s.push_str("## Make these cases match\n\n");
    let sub = Report {
        original: report.original.clone(),
        candidate: report.candidate.clone(),
        cases: Vec::new(),
    };
    for case in batch {
        let one = Report {
            cases: vec![crate::report::CaseResult {
                name: case.name.clone(),
                divergences: case.divergences.clone(),
                original_ms: case.original_ms,
                candidate_ms: case.candidate_ms,
            }],
            ..sub.clone()
        };
        let inputs = crate::corpus::read(&layout.case_dir(&case.name))
            .map(|(r, _)| {
                format!(
                    "args {:?}, stdin {} bytes, files {:?}",
                    r.input.args,
                    r.input.stdin.len(),
                    r.input.files.keys().collect::<Vec<_>>()
                )
            })
            .unwrap_or_default();
        s.push_str(&format!(
            "### {}\n\nInput: {inputs}\n\n```diff\n{}```\n\n",
            case.name,
            one.render(60)
        ));
    }
    s
}

fn agent_command(project: &Project, task: &str, task_file: &Path) -> Result<Vec<String>> {
    let template: Vec<String> = match project.rewrite.agent.as_str() {
        "claude" => [
            "claude",
            "-p",
            "{prompt}",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Bash",
        ]
        .map(String::from)
        .to_vec(),
        "codex" => ["codex", "exec", "--full-auto", "{prompt}"]
            .map(String::from)
            .to_vec(),
        "opencode" => ["opencode", "run", "{prompt}"].map(String::from).to_vec(),
        "custom" => project.rewrite.command.clone(),
        other => bail!("agent {other:?}: claude, codex, opencode or custom"),
    };
    if template.is_empty() {
        bail!("agent = \"custom\" needs a command in [rewrite]");
    }
    let file = task_file.display().to_string();
    Ok(template
        .into_iter()
        .map(|a| a.replace("{prompt_file}", &file).replace("{prompt}", task))
        .collect())
}

fn log(
    state: &Path,
    iteration: u32,
    built: bool,
    matching: usize,
    total: usize,
    agent_seconds: u64,
) -> Result<()> {
    let step = Step {
        iteration,
        unix_time: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        built,
        matching,
        total,
        agent_seconds,
    };
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("history.jsonl"))?;
    writeln!(f, "{}", serde_json::to_string(&step)?)?;
    Ok(())
}

/// The candidate the project checks against by default: `[candidate]`, else the rewrite crate's binary.
pub fn default_candidate(layout: &Layout, project: &Project) -> Option<Program> {
    project.candidate.clone().or_else(|| {
        let (_, bin) = candidate(layout, project);
        bin.exists().then(|| Program {
            program: bin.display().to_string(),
            ..Default::default()
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(agent: &str) -> Project {
        let mut p: Project = toml::from_str("[original]\nprogram = \"/usr/bin/yt-dlp\"\n").unwrap();
        p.rewrite.agent = agent.into();
        p
    }

    #[test]
    fn agent_templates_take_the_prompt() {
        let argv = agent_command(&project("codex"), "do it", Path::new("/t.md")).unwrap();
        assert_eq!(argv, ["codex", "exec", "--full-auto", "do it"]);
        let mut p = project("custom");
        p.rewrite.command = vec!["my-agent".into(), "--file".into(), "{prompt_file}".into()];
        assert_eq!(
            agent_command(&p, "x", Path::new("/t.md")).unwrap(),
            ["my-agent", "--file", "/t.md"]
        );
        assert!(agent_command(&project("nope"), "x", Path::new("/t.md")).is_err());
    }

    #[test]
    fn scaffolds_a_crate_named_like_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::new(dir.path());
        let p = project("claude");
        assert!(scaffold(&layout, &p).unwrap());
        assert!(!scaffold(&layout, &p).unwrap());
        let toml = std::fs::read_to_string(dir.path().join("rewrite/Cargo.toml")).unwrap();
        assert!(toml.contains("name = \"yt-dlp\"") && toml.contains("[workspace]"));
        assert!(
            candidate(&layout, &p)
                .1
                .ends_with("rewrite/target/release/yt-dlp")
        );
    }
}
