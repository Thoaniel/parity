//! Parity's reverse-engineering core: what an executable is and what it contains, without running it.
//!
//! Every function takes the file's bytes, so callers decide how files are read (and agents can analyse any file,
//! not only installed programs). Results are plain serde types for the CLI's JSON and the MCP server.

pub mod binary;
pub mod disasm;
pub mod strings;

pub use binary::{Overview, Section, Symbol, overview, sections, symbols};
pub use disasm::{Instruction, disassemble};
pub use strings::{Encoding, FoundString, strings};

/// A symbol name made readable: Rust (legacy and v0) and Itanium C++ manglings; anything else as given.
pub fn demangle(name: &str) -> String {
    if let Ok(d) = rustc_demangle::try_demangle(name) {
        return format!("{d:#}");
    }
    if name.starts_with("_Z")
        && let Ok(sym) = cpp_demangle::Symbol::new(name)
        && let Ok(text) = sym.demangle(&cpp_demangle::DemangleOptions::default())
    {
        return text;
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demangles_rust_and_cpp() {
        assert_eq!(
            demangle("_ZN4core3fmt5write17h0123456789abcdefE"),
            "core::fmt::write"
        );
        assert_eq!(demangle("_ZN3foo3barEv"), "foo::bar()");
        assert_eq!(demangle("main"), "main");
    }
}
