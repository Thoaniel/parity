mod compare;
mod corpus;
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
    },
    /// Print what the original did in one recorded case.
    Show { case: String },
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
        } => replay(&layout, candidate, &cases, json, diff_lines),
        Command::Show { case } => show(&layout, &case),
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
) -> Result<ExitCode> {
    let project = layout.load_project()?;
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
        let actual = sandbox::run(&candidate, &record.input, &project.sandbox)
            .with_context(|| format!("case {name}"))?;
        results.push(report::CaseResult {
            name: name.clone(),
            divergences: compare::compare(&expected, &actual, &norm),
            original_ms: record.duration_ms,
            candidate_ms: actual.duration.as_millis() as u64,
        });
    }
    let report = report::Report {
        original: project.original.program.clone(),
        candidate: candidate.program.clone(),
        cases: results,
    };
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
