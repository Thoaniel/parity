# Parity

Rewrite any program in Rust, prove it behaves the same as the original, and swap it in without anyone noticing.

```
discover → record → rewrite → prove → swap → shadow → keep in sync
```

Parity records what a program really does (outputs, files, exit status, HTTP exchanges), turns that into a
differential test suite with the original as the oracle, and drives a coding agent until the Rust version matches.
It then installs the rewrite as a drop-in, runs it side by side with the original during real use, rolls back on
divergence, and keeps following upstream releases.

The same tools make any AI agent more capable: eyes to see what code really does (built-in reverse engineering, a
flight recorder for your machine, recordings of real behaviour) and a judge that says exactly what is still wrong.

**Status:** every stage exists in a first version; none has been battle-tested yet. Read the
[spec](docs/spec-v1.md). Flagship target: [yt-dlp](https://github.com/yt-dlp/yt-dlp) ([project](examples/yt-dlp)).

## The whole loop

```sh
cargo install --path crates/parity

parity init jq -C jq                 # parity.toml + cases.toml
parity discover -C jq --write-cases  # surface map: flags, subcommands, env vars, paths; a case for each
parity record -C jq                  # run the original on every case, keep what it did in corpus/
parity ledger init -C jq             # ledger.toml: every part of the original and its verdict
parity rewrite -C jq                 # drive an agent (claude, codex, opencode, custom) until every case matches
parity replay -C jq                  # the parity report: share matching, a diff per divergence, timings
parity swap -C jq                    # ~/.local/bin/jq → shadow mode: the original answers, the rewrite is compared
parity swap-mode jq swapped          # switch for real once shadowing found nothing
parity revert jq                     # everything back as it was
parity track -C jq --install-timer   # daily: new upstream release → re-record → replay → notify
```

## Stages

**Record and replay.** Each run gets a fresh working folder (mounted at `/tmp/work`), a cleared environment and, with
bubblewrap, a read-only system and no network. These are defaults for reproducible recordings, not limits:
`isolation = "none"`, `network = true`, `inherit_env = true` run against the real system. Nondeterminism is hidden
only by normalizers you declare. Recordings that differ from your own cases.toml need `--trust-corpus`.

**Discover.** The surface map comes from the program's file (ELF, PE, Mach-O via `parity-rea`), its source when it
is a Python or Node script (argparse, optparse, click, commander), and its own `--help` run in the sandbox.

**Rewrite.** Parity scaffolds a crate named like the original, builds it, replays, and hands the agent the next
failing cases with their diffs (or the build errors) in `.parity/task.md`. It stops when everything matches, the
budget is spent, or progress stalls. Agents: `claude`, `codex`, `opencode`, or any command (`agent = "custom"`).

**Swap, shadow, revert.** `parity swap` links `~/.local/bin/NAME` to Parity, which then acts as that program. In
shadow mode the original runs for real and the rewrite runs afterwards in the background on the same input, with
the working folder as a throwaway overlay; divergences become new cases, and a different exit status rolls the swap
back at once. `--unit NAME.service` swaps a systemd service with a drop-in.

**Servers.** `parity http record` is a proxy in front of the original that keeps every exchange;
`parity http replay` sends them to the rewrite and compares status, headers and bodies.

**Python libraries.** `parity pylib MODULE` sets up a PyO3 crate with the same import name; cases are Python
snippets, so the same loop applies.

**Keep in sync.** `parity track` follows PyPI, GitHub, npm or crates.io releases, re-records against the new
original, replays the rewrite and reports what the release changed.

## Tools for any machine

```sh
parity suggest                     # what here is worth rewriting first, ranked by expected gain
parity recorder --install          # flight recorder: pressure stalls and busy processes, rolling window
parity why "2 minutes ago"         # what the machine was waiting on, and who caused it
parity inspect /usr/bin/foo --symbols main   # overview, sections, symbols, strings, disassembly
```

## For agents: `parity mcp`

`binary_overview`, `list_sections`, `list_symbols` (demangled Rust and C++), `list_strings`, `disassemble` (x86,
ARM, AArch64, RISC-V, MIPS, PowerPC, SPARC), `surface_map`, and `replay`, the judge that tells an agent exactly
where its rewrite still differs from the original.

```sh
claude mcp add parity -- parity mcp
```

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
