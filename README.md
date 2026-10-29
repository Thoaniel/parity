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

**Status:** early design. Nothing runs yet. Read the [spec](docs/spec-v1.md).

Flagship target: [yt-dlp](https://github.com/yt-dlp/yt-dlp).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT) at your option.
