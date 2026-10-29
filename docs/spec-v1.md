# Parity: spec v1 (draft)

Draft 2026-10-29. Rewrite any program in Rust, prove it behaves the same as the original, and swap it in without the
user noticing. Horizon (Minecraft in Rust, 300+ differential harnesses) showed the method works for one program;
Parity makes it a tool for every program. One command, from "this is slow" to "it is now Rust and nothing broke".

```
parity <program>      # discover → record → rewrite → prove → swap → shadow → keep in sync
```

Parity is also a toolkit that makes AI agents more capable. Most people fail with coding agents because the agent
cannot see what a program really does and cannot check its own work. Parity gives any agent those two things:
eyes (REA, the flight recorder, recordings of real behaviour) and a judge (differential suites that say exactly
what is still wrong). Rewriting into Rust is the flagship use; the same tools serve debugging, porting between
languages, upgrading dependencies and understanding unknown code.

## Principles
- The original is the oracle. Nothing counts as done until the differential suite says it matches.
- Seamless: the user keeps their commands, configs, data, units and apps. Swapping in and rolling back are one step
  each, and rollback is automatic on divergence.
- Agent-neutral: the rewrite loop drives any coding agent (Claude Code, Codex, OpenCode, a local model) through
  adapter files; Parity owns the tests, the agent only writes code.
- Rust only, one static binary (`parity`), no Python/Node at runtime. Code and APIs in English.
- Local first: recordings, corpora and reports stay on the machine; nothing is uploaded.

## Pipeline

### 1. Discover (built-in REA)
A Rust port of REA's analysis core, as a library (`parity-rea`) and as an MCP server (`parity mcp`), replacing the
Node REA (371 MB resident → target < 40 MB).
- Binaries: ELF/PE/Mach-O sections, symbols, imports, strings, disassembly (`goblin`, `object`, `capstone`).
- Managed code: Python packages (entry points, CLI parsers, public API), JS bundles and Electron apps (`oxc`),
  JVM jars, .NET assemblies, APK/DEX.
- Output: a surface map. Commands, flags, config files read, env vars, sockets, HTTP routes, D-Bus names, files
  written, libraries exposed. The surface map drives recording and input generation.

### 2. Record
Run the original under observation in a sandbox (overlay filesystem, private network namespace, own D-Bus).
- CLIs: argv, env, stdin, stdout/stderr, exit code, filesystem diff.
- Daemons and servers: HTTP/WebSocket/gRPC through a recording proxy, D-Bus, Unix sockets.
- Everything: syscalls and timing via eBPF (ptrace fallback), CPU, RSS and startup time as the baseline.
- Inputs: real usage captured in shadow mode (step 6), the surface map's examples, and generated cases (grammar
  fuzzing from flags, schemas and routes).
- Normalizers for honest nondeterminism: timestamps, PIDs, temp paths, map ordering, random IDs. Declared per
  project, never guessed silently.
- Result: a behaviour corpus, versioned and replayable.

### 3. Rewrite
- Parity scaffolds the Rust crate (same CLI surface, same config formats) and a port ledger: every entry point,
  command and function of the original, with a verdict (ported, folded, missing, not needed) like Horizon's.
- The agent loop picks the next failing behaviours, hands the agent the failing cases plus REA's view of the
  original, and reruns the suite after every change. Budgets cap tokens and wall time per step.
- Humans can take over any time: the ledger and suite are plain files in the repo.

### 4. Prove
A parity report per build: share of corpus cases identical, every divergence with a minimal reproduction, and
performance against the baseline (startup, RSS, CPU, throughput). The report is what gets published with a rewrite.

### 5. Swap
Drop-in installation that keeps everything the user had.
- CLI: shadow install earlier on PATH, original kept under `~/.local/share/parity/originals/`.
- systemd units: drop-in override for ExecStart; same sockets, same state directory.
- Python packages: a PyO3 wheel with the same import name and API.
- Servers: same ports and API; clients do not change.
- `parity revert <program>` restores the original in one step.

### 6. Shadow
The rewrite runs for real while the original keeps running beside it on the same inputs (mirrored requests,
duplicated CLI calls in a sandbox). Outputs are compared live.
- On divergence: the user gets the original's answer, the case goes into the corpus, and the rewrite is rolled
  back automatically when the divergence touches user data or exit status.
- After N days with no divergence, shadowing turns off and the original is only kept for revert.

### 7. Keep in sync
Watch the original's releases. On a new version: re-record, run the suite, open a task for the agent with the new
divergences, report what changed. A rewrite never silently falls behind.

## Flight recorder
An always-on eBPF daemon (`parity record --always`, also a hadesd module) with near-zero overhead. It keeps a rolling
window of scheduling, I/O, memory pressure, GPU and process events.
- Ask after the fact: `parity why "2 minutes ago"` explains a stutter in plain language, for example "Steam wrote
  2 GB to disk while localsearch indexed; Firefox waited 340 ms on I/O".
- Shares the recording layer with step 2, so a recorded stall can become a test case.

## Suggestions
Parity measures what actually runs (PC, and later the server, the Pi and Android) and ranks rewrite candidates by
expected gain: resident memory, CPU, startup, stalls blamed by the flight recorder, runtime (Python, Node,
Electron, JVM, PHP score higher than C/C++), and how testable the program is. `parity suggest` prints the ranking with
the reasons; the bar panel shows the top few.

## Agent toolkit
Everything above is also exposed as tools for any agent over MCP (`parity mcp`) and as a CLI agents can call:
inspect (REA), observe (record a run, flight-recorder queries), check (run the differential suite, minimise a
failing case), and sandbox (run anything in an overlay with a diff of what it changed). Skills/adapter files tell
each agent when to use which tool. Goal: an agent with Parity can verify its own work instead of guessing.

## Integration
- hadesd module and bar panel: rewrites, their state (recording, porting x %, shadowing, swapped) and divergences.
- Notifications on divergence and rollback.
- `parity mcp` exposes the REA tools plus Parity's own (corpus, ledger, report) to any agent.

## Milestones
- **M0:** CLI record/replay. Corpus format, sandbox, normalizers, report. Prove it on one small CLI.
- **M1:** `parity-rea` core: binary, Python and JS surface maps; MCP server with the most used REA tools.
- **M2:** Agent loop and port ledger with agent adapters. First full CLI rewrite end to end.
- **M3:** Swap, revert and shadow mode for CLIs and systemd daemons.
- **M4:** Servers and D-Bus: recording proxy, mirrored traffic. First daemon rewrite.
- **M5:** Python libraries → PyO3 wheels.
- **M6:** Release tracking. Launch announcement with the flagship rewrite and its parity report.

## Flagship: yt-dlp
yt-dlp (Python, public domain) is the launch proof: huge user base, slow Python startup, and upstream changes
weekly, which shows off release tracking. Scope for launch: the core (option parsing, format selection, output
templates, downloader, post-processing via ffmpeg) and the YouTube extractor; more extractors follow, each with its
own corpus. The corpus is recorded against public test URLs and yt-dlp's own extractor test definitions; recordings
of third-party media are never committed.
- Alongside: flight recorder (after M0, it shares the recorder), suggestions (after M1), agent toolkit (grows with
  every milestone).
- Open source and public from day one (MIT OR Apache-2.0), developed in the open.
- Later: GUI apps (screenshot diff), Electron apps.

## Not included (v1)
Kernel modules, drivers and anything that writes to hardware; proprietary software where the licence forbids
reverse engineering; uploading recordings anywhere.
