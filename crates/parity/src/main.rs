mod compare;
mod corpus;
mod discover;
mod http;
mod ledger;
mod mcp;
mod normalize;
mod project;
mod pylib;
mod recorder;
mod report;
mod rewrite;
mod sandbox;
mod suggest;
mod swap;
mod track;

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
    /// Look inside any executable: overview, sections, symbols, strings, disassembly.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        sections: bool,
        /// Symbols whose name contains this ("" for all).
        #[arg(long)]
        symbols: Option<String>,
        /// Strings matching this regex ("" for all).
        #[arg(long)]
        strings: Option<String>,
        #[arg(long, default_value_t = 6)]
        min_len: usize,
        /// Disassemble from this symbol or address (0x...).
        #[arg(long)]
        disasm: Option<String>,
        #[arg(long, default_value_t = 64)]
        count: usize,
        /// At most this many symbols or strings (0 = all).
        #[arg(long, default_value_t = 200)]
        limit: usize,
    },
    /// The port ledger: every part of the original and its verdict. Without an action, a summary.
    Ledger {
        #[command(subcommand)]
        action: Option<LedgerAction>,
    },
    /// Drive a coding agent until the rewrite matches every recorded case.
    Rewrite {
        #[arg(long)]
        max_iterations: Option<u32>,
        /// Write the next task and print the agent command without running it.
        #[arg(long)]
        dry_run: bool,
    },
    /// Put the rewrite in place of the original: shadow mode by default (the original still answers; the rewrite
    /// runs beside it and is compared), --now to switch at once, --unit for a systemd service.
    Swap {
        /// The command name (default: the original's file name).
        name: Option<String>,
        #[arg(long)]
        now: bool,
        #[arg(long)]
        unit: Option<String>,
        /// A system unit instead of a user unit (needs root).
        #[arg(long)]
        system: bool,
    },
    /// Change a swap's mode: shadow, swapped or original.
    SwapMode { name: String, mode: String },
    /// Undo a swap: the original is back exactly where it was.
    Revert { name: String },
    /// The swaps on this machine, their modes, runs and divergences.
    Swaps,
    #[command(hide = true)]
    ShadowCheck { job: PathBuf },
    /// Servers: record HTTP exchanges through a proxy in front of the original, replay them against the rewrite.
    Http {
        #[command(subcommand)]
        action: HttpAction,
    },
    /// Start a project for a Python library, rewritten as a PyO3 module with the same import name.
    Pylib { module: String },
    /// Follow the original's releases: re-record and replay when a new one is out.
    Track {
        /// Re-record and replay now, whatever the version.
        #[arg(long)]
        rerecord: bool,
        /// Run daily with a systemd user timer.
        #[arg(long)]
        install_timer: bool,
    },
    /// What on this machine is worth rewriting first.
    Suggest {
        #[arg(long, default_value_t = 15)]
        limit: usize,
    },
    /// The flight recorder: sample stalls and busy processes into a rolling window.
    Recorder {
        #[arg(long, default_value_t = 250)]
        interval_ms: u64,
        #[arg(long, default_value_t = 1800)]
        window_secs: u64,
        /// Install and start it as a systemd user service.
        #[arg(long)]
        install: bool,
    },
    /// Why was the machine slow then? `now`, `90s`, `2 minutes ago`, `14:05`.
    Why {
        when: String,
        /// Seconds before and after to look at.
        #[arg(long, default_value_t = 15)]
        span: u64,
    },
}

#[derive(Subcommand)]
enum LedgerAction {
    /// Add everything in surface.json that the ledger doesn't list yet.
    Init,
    /// Set an item's status: missing, partial, ported, folded, not_needed.
    Set {
        id: String,
        status: ledger::Status,
        #[arg(long)]
        note: Option<String>,
    },
}

#[derive(Subcommand)]
enum HttpAction {
    /// Listen here and forward to [http] original_url, keeping every exchange.
    Record {
        #[arg(long, default_value = "127.0.0.1:8099")]
        listen: String,
    },
    /// Send the recorded requests to the candidate and report divergences.
    Replay {
        #[arg(long)]
        candidate_url: Option<String>,
        #[arg(long, default_value_t = 40)]
        diff_lines: usize,
    },
}

fn main() -> ExitCode {
    // Started through a swap's link under another name: be that program.
    let mut argv = std::env::args();
    let argv0 = argv.next().unwrap_or_default();
    let called = std::path::Path::new(&argv0)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !called.is_empty()
        && called != "parity"
        && !called.starts_with("parity-")
        && !called.starts_with("parity.")
    {
        return match swap::shim_main(&called, argv.collect()) {
            Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
            Err(e) => {
                eprintln!("{e:#}");
                ExitCode::from(127)
            }
        };
    }
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
        Command::Inspect {
            path,
            sections,
            symbols,
            strings,
            min_len,
            disasm,
            count,
            limit,
        } => inspect(
            &path, sections, symbols, strings, min_len, disasm, count, limit,
        ),
        Command::Ledger { action } => {
            let mut l = ledger::Ledger::load(&layout)?;
            match action {
                None => print!("{}", l.summary()),
                Some(LedgerAction::Init) => {
                    let path = layout.root.join("surface.json");
                    let text = std::fs::read_to_string(&path).with_context(|| {
                        format!("{}: run `parity discover` first", path.display())
                    })?;
                    let surface: discover::Surface = serde_json::from_str(&text)?;
                    let added = l.merge_surface(&surface);
                    l.save(&layout)?;
                    println!("{added} items added\n{}", l.summary());
                }
                Some(LedgerAction::Set { id, status, note }) => {
                    l.set(&id, status, note)?;
                    l.save(&layout)?;
                }
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Rewrite {
            max_iterations,
            dry_run,
        } => rewrite::run(
            &layout,
            &rewrite::Options {
                max_iterations,
                dry_run,
            },
        )
        .map(|()| ExitCode::SUCCESS),
        Command::Swap {
            name,
            now,
            unit,
            system,
        } => {
            let project = layout.load_project()?;
            let original = sandbox::resolve(&project.original.program)?;
            let candidate = rewrite::default_candidate(&layout, &project)
                .context("no candidate to swap in: build the rewrite or set [candidate]")?;
            let name = name.unwrap_or_else(|| {
                original
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            });
            swap::swap(swap::SwapRequest {
                name,
                original,
                candidate: sandbox::resolve(&candidate.program)?,
                project: Some(layout.root.clone()),
                mode: if now {
                    swap::Mode::Swapped
                } else {
                    swap::Mode::Shadow
                },
                unit,
                system_unit: system,
            })
            .map(|()| ExitCode::SUCCESS)
        }
        Command::SwapMode { name, mode } => {
            let mode = match mode.as_str() {
                "shadow" => swap::Mode::Shadow,
                "swapped" => swap::Mode::Swapped,
                "original" => swap::Mode::Original,
                other => bail!("mode {other:?}: shadow, swapped or original"),
            };
            swap::set_mode(&name, mode).map(|()| ExitCode::SUCCESS)
        }
        Command::Revert { name } => swap::revert(&name).map(|()| ExitCode::SUCCESS),
        Command::Swaps => swap::list().map(|()| ExitCode::SUCCESS),
        Command::ShadowCheck { job } => swap::shadow_check(&job).map(|()| ExitCode::SUCCESS),
        Command::Http { action } => match action {
            HttpAction::Record { listen } => {
                http::record(&layout, &listen).map(|()| ExitCode::SUCCESS)
            }
            HttpAction::Replay {
                candidate_url,
                diff_lines,
            } => {
                let report = http::replay(&layout, candidate_url)?;
                print!("{}", report.render(diff_lines));
                Ok(if report.matching() == report.cases.len() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(1)
                })
            }
        },
        Command::Pylib { module } => pylib::init(&layout, &module).map(|()| ExitCode::SUCCESS),
        Command::Track {
            rerecord,
            install_timer,
        } => {
            if install_timer {
                track::install_timer(&layout)?;
            } else {
                track::track(&layout, rerecord)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Suggest { limit } => suggest::suggest(limit).map(|()| ExitCode::SUCCESS),
        Command::Recorder {
            interval_ms,
            window_secs,
            install,
        } => {
            if install {
                recorder::install()?;
            } else {
                recorder::run(interval_ms.max(50), window_secs)?;
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Why { when, span } => recorder::why(&when, span).map(|()| ExitCode::SUCCESS),
    }
}

#[allow(clippy::too_many_arguments)]
fn inspect(
    path: &std::path::Path,
    sections: bool,
    symbols: Option<String>,
    strings: Option<String>,
    min_len: usize,
    disasm: Option<String>,
    count: usize,
    limit: usize,
) -> Result<ExitCode> {
    let data = std::fs::read(path).with_context(|| format!("{}", path.display()))?;
    let cap = |n: usize| if limit == 0 { n } else { n.min(limit) };
    if sections {
        for s in parity_rea::sections(&data)? {
            println!(
                "{:<24} {:#014x} {:>10}  {}",
                s.name, s.address, s.size, s.kind
            );
        }
    } else if let Some(filter) = symbols {
        let syms = parity_rea::symbols(&data, (!filter.is_empty()).then_some(filter.as_str()))?;
        for s in syms.iter().take(cap(syms.len())) {
            println!(
                "{:#014x} {:>8} {:<8} {}",
                s.address, s.size, s.kind, s.demangled
            );
        }
        println!("{} symbols", syms.len());
    } else if let Some(pattern) = strings {
        let re = (!pattern.is_empty())
            .then(|| regex::Regex::new(&pattern))
            .transpose()?;
        let found = parity_rea::strings(&data, min_len, re.as_ref(), 0);
        for s in found.iter().take(cap(found.len())) {
            println!("{:#010x} {}", s.offset, s.text);
        }
        println!("{} strings", found.len());
    } else if let Some(start) = disasm {
        let start = match start.strip_prefix("0x") {
            Some(hex) => parity_rea::disasm::Start::Address(u64::from_str_radix(hex, 16)?),
            None => parity_rea::disasm::Start::Symbol(&start),
        };
        for i in parity_rea::disassemble(&data, start, count)? {
            println!(
                "{:#014x}  {:<24} {} {}",
                i.address, i.bytes, i.mnemonic, i.operands
            );
        }
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(&parity_rea::overview(&data)?)?
        );
    }
    Ok(ExitCode::SUCCESS)
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
            ..Default::default()
        },
        None => rewrite::default_candidate(layout, &project).context(
            "no candidate: set [candidate] in parity.toml, build the rewrite, or pass --candidate",
        )?,
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
            ..Default::default()
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
