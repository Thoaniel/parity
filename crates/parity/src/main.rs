mod compare;
mod corpus;
mod discover;
mod mcp;
mod normalize;
mod project;
mod report;
mod sandbox;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use project::{Layout, Program};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    version,
    about = "Rewrite any program in Rust, prove it behaves the same, and swap it in seamlessly."
)]
struct Cli {
    /// The project folder (with parity.toml and cases.toml).
    #[arg(long, short = 'C', global = true, default_value = ".")]
    project: PathBuf,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start a project for PROGRAM: writes parity.toml and cases.toml.
    Init { program: String },
    /// Run the original on every case (or the named ones) and store what it did in corpus/.
    Record {
        #[arg(long = "case")]
        cases: Vec<String>,
    },
    /// Run the candidate on every recorded case and report where it differs from the original.
    Replay {
        /// The program to check (default: [candidate] in parity.toml).
        #[arg(long)]
        candidate: Option<String>,
        #[arg(long = "case")]
        cases: Vec<String>,
        /// Also write the report as JSON here.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Diff lines shown per divergence.
        #[arg(long, default_value_t = 40)]
        diff_lines: usize,
        /// Replay recorded cases that cases.toml doesn't define identically (a corpus from someone else). Their
        /// arguments, stdin, files and environment run as recorded, LD_PRELOAD and the like included.
        #[arg(long)]
        trust_corpus: bool,
    },
    /// Print what the original did in one recorded case.
    Show { case: String },
    /// Map what a program exposes (flags, subcommands, env vars, paths, URLs) from its file, its source and its
    /// own --help. Default program: the project's original.
    Discover {
        program: Option<String>,
        /// Only read files; don't run the program's --help.
        #[arg(long)]
        no_run: bool,
        /// Add a case for each subcommand and plain flag found to cases.toml.
        #[arg(long)]
        write_cases: bool,
        /// Where to write the surface map (default: surface.json in the project).
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Serve Parity's tools over the Model Context Protocol on stdin/stdout, for any agent.
    Mcp,
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("parity: {e:#}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> Result<ExitCode> {
    let layout = Layout::new(&cli.project);
    match cli.command {
        Command::Init { program } => init(&layout, &program),
        Command::Record { cases } => record(&layout, &cases),
        Command::Replay {
            candidate,
            cases,
            json,
            diff_lines,
            trust_corpus,
        } => replay(&layout, candidate, &cases, json, diff_lines, trust_corpus),
        Command::Show { case } => show(&layout, &case),
        Command::Discover {
            program,
            no_run,
            write_cases,
            json,
        } => discover(&layout, program, no_run, write_cases, json),
        Command::Mcp => mcp::serve().map(|()| ExitCode::SUCCESS),
    }
}

fn init(layout: &Layout, program: &str) -> Result<ExitCode> {
    std::fs::create_dir_all(&layout.root)?;
    for (path, text) in [
        (
            layout.project_file(),
            project::PROJECT_TEMPLATE.replace("{original}", program),
        ),
        (layout.cases_file(), project::CASES_TEMPLATE.to_string()),
    ] {
        if path.exists() {
            bail!("{} already exists", path.display());
        }
        std::fs::write(&path, text)?;
        println!("wrote {}", path.display());
    }
    println!("Add cases to cases.toml, then run `parity record`.");
    Ok(ExitCode::SUCCESS)
}

fn record(layout: &Layout, only: &[String]) -> Result<ExitCode> {
    let project = layout.load_project()?;
    let cases = selected(layout.load_cases()?, only, |c| &c.name)?;
    let exe = sandbox::resolve(&project.original.program)?;
    for case in &cases {
        let observed = sandbox::run(&project.original, case, &project.sandbox)
            .with_context(|| format!("case {}", case.name))?;
        corpus::write(
            &layout.case_dir(&case.name),
            case,
            &observed,
            &exe.display().to_string(),
        )?;
        println!(
            "recorded {:<32} {} ({} ms)",
            case.name,
            observed.exit,
            observed.duration.as_millis()
        );
    }
    println!("{} cases in {}", cases.len(), layout.corpus_dir().display());
    Ok(ExitCode::SUCCESS)
}

fn replay(
    layout: &Layout,
    candidate: Option<String>,
    only: &[String],
    json: Option<PathBuf>,
    diff_lines: usize,
    trust_corpus: bool,
) -> Result<ExitCode> {
    let report = replay_report(layout, candidate, only, trust_corpus)?;
    print!("{}", report.render(diff_lines));
    if let Some(path) = json {
        std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    }
    Ok(if report.matching() == report.cases.len() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// Runs the candidate on the recorded cases and compares; shared by `parity replay` and the MCP `replay` tool.
fn replay_report(
    layout: &Layout,
    candidate: Option<String>,
    only: &[String],
    trust_corpus: bool,
) -> Result<report::Report> {
    let project = layout.load_project()?;
    // The project's own cases are trusted; a recording that differs from them came from elsewhere.
    let own = if layout.cases_file().exists() {
        layout.load_cases()?
    } else {
        Vec::new()
    };
    let candidate = match candidate {
        Some(program) => Program {
            program,
            args: Vec::new(),
        },
        None => project
            .candidate
            .clone()
            .context("no candidate: set [candidate] in parity.toml or pass --candidate")?,
    };
    let norm = normalize::Normalizer::new(&project.normalize)?;
    let names = selected(corpus::list(&layout.corpus_dir())?, only, |n| n)?;
    if names.is_empty() {
        bail!("nothing recorded yet: run `parity record` first");
    }
    let mut results = Vec::new();
    for name in &names {
        let (record, expected) = corpus::read(&layout.case_dir(name))?;
        if !trust_corpus && !own.contains(&record.input) {
            bail!(
                "case {name}: the recording's input isn't the one in cases.toml. Check it with \
                 `parity show {name}` and replay with --trust-corpus if it is yours"
            );
        }
        let actual = sandbox::run(&candidate, &record.input, &project.sandbox)
            .with_context(|| format!("case {name}"))?;
        results.push(report::CaseResult {
            name: name.clone(),
            divergences: compare::compare(&expected, &actual, &norm),
            original_ms: record.duration_ms,
            candidate_ms: actual.duration.as_millis() as u64,
        });
    }
    Ok(report::Report {
        original: project.original.program.clone(),
        candidate: candidate.program.clone(),
        cases: results,
    })
}

fn discover(
    layout: &Layout,
    program: Option<String>,
    no_run: bool,
    write_cases: bool,
    json: Option<PathBuf>,
) -> Result<ExitCode> {
    let project = layout
        .project_file()
        .exists()
        .then(|| layout.load_project())
        .transpose()?;
    let program = match (program, &project) {
        (Some(p), _) => Program {
            program: p,
            args: Vec::new(),
        },
        (None, Some(p)) => p.original.clone(),
        (None, None) => bail!("name a program, or run in a project folder"),
    };
    let sandbox_cfg = project.map(|p| p.sandbox).unwrap_or_default();
    let opts = discover::Options {
        run_help: !no_run,
        max_subcommands: 50,
    };
    let s = discover::discover(&program, &sandbox_cfg, &opts)?;
    println!(
        "{} ({}{})",
        s.path,
        s.kind,
        s.interpreter
            .as_ref()
            .map(|i| format!(", {i}"))
            .unwrap_or_default()
    );
    if let Some(b) = &s.binary {
        println!(
            "  {} {}-bit, {} libraries, hints: {}",
            b.architecture,
            b.bits,
            b.libraries.len(),
            b.hints.join(", ")
        );
    }
    println!(
        "  {} flags, {} subcommands",
        s.flags.len(),
        s.subcommands.len()
    );
    println!(
        "  {} env vars, {} paths, {} urls",
        s.env_vars.len(),
        s.paths.len(),
        s.urls.len()
    );
    if !s.sources.is_empty() {
        println!("  {} source files read", s.sources.len());
    }
    let out = json.or_else(|| {
        layout
            .project_file()
            .exists()
            .then(|| layout.root.join("surface.json"))
    });
    if let Some(path) = out {
        std::fs::write(&path, serde_json::to_vec_pretty(&s)?)?;
        println!("wrote {}", path.display());
    }
    if write_cases {
        let added = project::append_cases(
            layout,
            &discover::suggested_cases(&s),
            "generated by `parity discover`",
        )?;
        println!("added {added} cases to {}", layout.cases_file().display());
    }
    Ok(ExitCode::SUCCESS)
}

fn show(layout: &Layout, case: &str) -> Result<ExitCode> {
    project::check_case_name(case)?;
    let (record, observed) = corpus::read(&layout.case_dir(case))?;
    println!("case:     {}", record.input.name);
    println!("args:     {:?}", record.input.args);
    println!("program:  {}", record.recorded_with);
    println!("result:   {} in {} ms", observed.exit, record.duration_ms);
    println!("--- stdout\n{}", String::from_utf8_lossy(&observed.stdout));
    println!("--- stderr\n{}", String::from_utf8_lossy(&observed.stderr));
    for (path, bytes) in &observed.files {
        println!("--- file {path} ({} bytes)", bytes.len());
    }
    Ok(ExitCode::SUCCESS)
}

/// All items, or only the named ones (each must exist).
fn selected<T>(items: Vec<T>, only: &[String], name: impl Fn(&T) -> &str) -> Result<Vec<T>> {
    if only.is_empty() {
        return Ok(items);
    }
    for wanted in only {
        if !items.iter().any(|i| name(i) == wanted) {
            bail!("no case named {wanted:?}");
        }
    }
    Ok(items
        .into_iter()
        .filter(|i| only.iter().any(|w| w == name(i)))
        .collect())
}
