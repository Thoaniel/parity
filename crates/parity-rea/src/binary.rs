//! Executables and object files: ELF, PE, Mach-O (and what else `object` reads), with their sections, symbols,
//! imports, exports and the libraries they load.

use anyhow::{Context, Result};
use object::{Object, ObjectSection, ObjectSymbol};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct Overview {
    pub format: String,
    pub architecture: String,
    pub bits: u8,
    pub endian: String,
    pub entry: u64,
    pub kind: String,
    pub stripped: bool,
    pub sections: usize,
    pub symbols: usize,
    pub dynamic_symbols: usize,
    /// Shared libraries it loads (DT_NEEDED, PE imports, Mach-O load commands).
    pub libraries: Vec<String>,
    pub imports: Vec<Import>,
    pub exports: Vec<String>,
    /// Toolchain hints found in the file: Rust, Go, Python embedding, Electron and others.
    pub hints: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Import {
    pub library: String,
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Section {
    pub name: String,
    pub address: u64,
    pub size: u64,
    pub file_offset: Option<u64>,
    pub kind: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Symbol {
    pub name: String,
    pub demangled: String,
    pub address: u64,
    pub size: u64,
    pub kind: String,
    pub global: bool,
    pub defined: bool,
    pub dynamic: bool,
}

pub fn overview(data: &[u8]) -> Result<Overview> {
    let file =
        object::File::parse(data).context("not an executable or object file this build reads")?;
    let imports: Vec<Import> = file
        .imports()
        .unwrap_or_default()
        .iter()
        .map(|i| Import {
            library: String::from_utf8_lossy(i.library()).into_owned(),
            name: String::from_utf8_lossy(i.name()).into_owned(),
        })
        .collect();
    let exports = file
        .exports()
        .unwrap_or_default()
        .iter()
        .map(|e| String::from_utf8_lossy(e.name()).into_owned())
        .collect();
    let symbols = file.symbols().count();
    Ok(Overview {
        format: format!("{:?}", file.format()),
        architecture: format!("{:?}", file.architecture()),
        bits: if file.is_64() { 64 } else { 32 },
        endian: if file.is_little_endian() {
            "little"
        } else {
            "big"
        }
        .into(),
        entry: file.entry(),
        kind: format!("{:?}", file.kind()),
        stripped: symbols == 0,
        sections: file.sections().count(),
        symbols,
        dynamic_symbols: file.dynamic_symbols().count(),
        libraries: libraries(data),
        imports,
        exports,
        hints: hints(&file, data),
    })
}

pub fn sections(data: &[u8]) -> Result<Vec<Section>> {
    let file = object::File::parse(data)?;
    Ok(file
        .sections()
        .map(|s| Section {
            name: s.name().unwrap_or("?").to_string(),
            address: s.address(),
            size: s.size(),
            file_offset: s.file_range().map(|(offset, _)| offset),
            kind: format!("{:?}", s.kind()),
        })
        .collect())
}

/// Static and dynamic symbols, optionally only those whose name or demangled name contains `filter`.
pub fn symbols(data: &[u8], filter: Option<&str>) -> Result<Vec<Symbol>> {
    let file = object::File::parse(data)?;
    let mut out = Vec::new();
    for (dynamic, iter) in [(false, file.symbols()), (true, file.dynamic_symbols())] {
        for s in iter {
            let Ok(name) = s.name() else { continue };
            if name.is_empty() {
                continue;
            }
            let demangled = crate::demangle(name);
            if filter.is_some_and(|f| !name.contains(f) && !demangled.contains(f)) {
                continue;
            }
            out.push(Symbol {
                name: name.to_string(),
                demangled,
                address: s.address(),
                size: s.size(),
                kind: format!("{:?}", s.kind()),
                global: s.is_global(),
                defined: s.is_definition(),
                dynamic,
            });
        }
    }
    Ok(out)
}

fn libraries(data: &[u8]) -> Vec<String> {
    match goblin::Object::parse(data) {
        Ok(goblin::Object::Elf(elf)) => elf.libraries.iter().map(|s| s.to_string()).collect(),
        Ok(goblin::Object::PE(pe)) => pe.libraries.iter().map(|s| s.to_string()).collect(),
        Ok(goblin::Object::Mach(goblin::mach::Mach::Binary(m))) => m
            .libs
            .iter()
            .filter(|l| *l != &"self")
            .map(|s| s.to_string())
            .collect(),
        _ => Vec::new(),
    }
}

fn hints(file: &object::File, data: &[u8]) -> Vec<String> {
    let section = |name: &str| file.section_by_name(name).is_some();
    let contains = |needle: &[u8]| data.windows(needle.len()).any(|w| w == needle);
    let mut out = Vec::new();
    if section(".go.buildinfo") || section("__go_buildinfo") || section(".gopclntab") {
        out.push("go".into());
    }
    if contains(b"/rustc/") || contains(b"rust_panic") {
        out.push("rust".into());
    }
    if contains(b"Py_Initialize") || contains(b"_PyRuntime") {
        out.push("python-embedded".into());
    }
    if contains(b"PyInstaller") || contains(b"_MEIPASS") {
        out.push("pyinstaller".into());
    }
    if contains(b"electron.asar") || contains(b"ELECTRON_RUN_AS_NODE") {
        out.push("electron".into());
    }
    if contains(b"node::Start") || contains(b"v8::Isolate") {
        out.push("node-or-v8".into());
    }
    if contains(b"JNI_CreateJavaVM") {
        out.push("jvm".into());
    }
    if contains(b"mono_jit_init") || contains(b"coreclr_initialize") {
        out.push("dotnet".into());
    }
    if contains(b"GLIBCXX_") || contains(b"libstdc++") {
        out.push("c++".into());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_own_test_binary() {
        let data = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let o = overview(&data).unwrap();
        assert!(o.sections > 0);
        assert!(o.hints.contains(&"rust".to_string()));
        assert!(!sections(&data).unwrap().is_empty());
        let syms = symbols(&data, Some("reads_its_own_test_binary")).unwrap();
        assert!(
            syms.iter()
                .any(|s| s.demangled.contains("reads_its_own_test_binary"))
        );
    }

    #[test]
    fn refuses_non_executables() {
        assert!(overview(b"#!/bin/sh\necho hi\n").is_err());
    }
}
