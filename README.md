# Parity

Rewrite any program in Rust, prove it behaves the same as the original, and swap it in without anyone noticing.

```
parity <program>      # discover → record → rewrite → prove → swap → shadow → keep in sync
```

Parity records what a program really does (outputs, files, network, syscalls), turns that into a differential test
suite with the original as the oracle, and drives a coding agent until the Rust version matches. It then installs the
rewrite as a drop-in, runs it side by side with the original during real use, rolls back on any divergence, and
keeps following upstream releases.

The same tools make any AI agent more capable: eyes to see what code really does (built-in reverse engineering, a
flight recorder for your machine, recordings of real behaviour) and a judge that says exactly what is still wrong.

**Status:** M1, record and replay for command-line programs, surface maps and an MCP server. Read the [spec](docs/spec-v1.md) for where it is going.

## Try it (M0)

```sh
cargo install --path crates/parity
parity init jq -C jq-project          # writes parity.toml and cases.toml
$EDITOR jq-project/cases.toml         # describe runs: args, stdin, files, env
parity record -C jq-project           # runs jq on every case, stores what it did in corpus/
parity replay -C jq-project --candidate jaq   # runs another implementation, reports every difference
```

Each run gets a fresh working folder (mounted at `/work`), a cleared environment and, when bubblewrap is
installed, a read-only system and no network. All of that is a default for reproducible recordings, not a limit:
`isolation = "none"`, `network = true` and `inherit_env = true` in `parity.toml` run against the real system.
Honest nondeterminism (timestamps, PIDs) is hidden by normalizers you declare in `parity.toml`; nothing is ignored
silently.

## Discover (M1)

```sh
parity discover yt-dlp                       # flags, subcommands, env vars, paths, URLs
parity discover -C jq-project --write-cases  # the same for the project's original, plus a case per flag/subcommand
```

The surface map comes from the program's file (ELF, PE and Mach-O via `parity-rea`: libraries, imports, toolchain
hints, strings), from its source when it is a Python or Node script (argparse, optparse, click, commander), and from
its own `--help` run in the sandbox.

## For agents: `parity mcp`

Parity's tools over the Model Context Protocol: `binary_overview`, `list_sections`, `list_symbols` (demangled
Rust and C++), `list_strings`, `disassemble` (x86, ARM, AArch64, RISC-V, MIPS, PowerPC, SPARC), `surface_map`, and
`replay`, the judge that tells an agent exactly where its rewrite still differs from the original.

```sh
claude mcp add parity -- parity mcp
```

Flagship target: [yt-dlp](https://github.com/yt-dlp/yt-dlp).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
