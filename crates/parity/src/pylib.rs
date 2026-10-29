//! Python libraries rewritten as PyO3 extension modules with the same import name.
//!
//! `parity pylib MODULE` sets up a project where the original is `python3` importing the installed module, and
//! the candidate is the same `python3` with PYTHONPATH pointing at the rewrite's built extension. Cases are
//! Python snippets (`python3 -c ...`), so record, replay, the rewrite loop and swap all work unchanged. One
//! signature case per public function is generated; behaviour cases are written by hand or by the agent.

use crate::project::{CaseInput, Layout};
use anyhow::{Context, Result, bail};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// The module's source: a package folder or a single file, found without importing anything.
fn find_module(module: &str) -> Result<PathBuf> {
    let top = module.split('.').next().unwrap_or(module);
    for root in crate::discover::python_roots("/usr/bin/python3", Path::new("/nonexistent/x")) {
        let package = root.join(top);
        if package.join("__init__.py").is_file() {
            return Ok(package);
        }
        let single = root.join(format!("{top}.py"));
        if single.is_file() {
            return Ok(single);
        }
    }
    bail!("module {top} isn't installed for python3 (looked in site-packages and dist-packages)")
}

/// Public module-level functions: `__all__` when the module declares it, else every `def` not starting with `_`.
pub fn public_functions(source: &str) -> Vec<String> {
    static ALL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?s)__all__\s*=\s*[\[(](.*?)[\])]").unwrap());
    static QUOTED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"['"]([A-Za-z_]\w*)['"]"#).unwrap());
    static DEF: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?m)^(?:async\s+)?def\s+([A-Za-z]\w*)\s*\(").unwrap());
    let defs: Vec<String> = DEF
        .captures_iter(source)
        .map(|c| c[1].to_string())
        .collect();
    if let Some(all) = ALL.captures(source) {
        let named: Vec<String> = QUOTED
            .captures_iter(&all[1])
            .map(|c| c[1].to_string())
            .collect();
        return named.into_iter().filter(|n| defs.contains(n)).collect();
    }
    defs
}

pub fn init(layout: &Layout, module: &str) -> Result<()> {
    if layout.project_file().exists() {
        bail!("{} already exists", layout.project_file().display());
    }
    if !module
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
        || module.is_empty()
    {
        bail!("{module:?} is not a module name");
    }
    let found = find_module(module)?;
    let init_file = if found.is_dir() {
        found.join("__init__.py")
    } else {
        found.clone()
    };
    let source =
        std::fs::read_to_string(&init_file).with_context(|| format!("{}", init_file.display()))?;
    let functions = public_functions(&source);
    let top = module.split('.').next().unwrap();
    std::fs::create_dir_all(&layout.root)?;
    let root = std::path::absolute(&layout.root)?;
    let ext_dir = root.join("rewrite/python");

    std::fs::write(
        layout.project_file(),
        format!(
            r#"# A Python library rewritten as a PyO3 module with the same import name.
# The original is python3 importing the installed {module} ({found}); the candidate is the same python3 with
# PYTHONPATH pointing at the rewrite's extension.
[original]
program = "python3"
args = ["-I"]

[candidate]
program = "python3"
env = {{ PYTHONPATH = "{ext}" }}

[sandbox]
env = {{ LANG = "C.UTF-8", LC_ALL = "C.UTF-8", TZ = "UTC", PYTHONHASHSEED = "0" }}

[rewrite]
dir = "rewrite"
# Copies the built library to rewrite/python/{top}.so so python3 imports it first.
post_build = ["sh", "-c", "mkdir -p python && cp target/release/lib{top}.so python/{top}.so"]
"#,
            found = found.display(),
            ext = ext_dir.display(),
        ),
    )?;
    // The candidate runs with -I too, but -I ignores PYTHONPATH, so it is left out there on purpose.

    let snippet = |code: String| vec!["-c".to_string(), code];
    let mut cases = vec![CaseInput {
        name: "public-names".into(),
        args: snippet(format!(
            "import {module} as m; print(sorted(n for n in dir(m) if not n.startswith('_') and callable(getattr(m, n))))"
        )),
        ..Default::default()
    }];
    for f in &functions {
        cases.push(CaseInput {
            name: format!("sig-{f}"),
            args: snippet(format!(
                "import inspect, {module} as m\ntry:\n    print(inspect.signature(m.{f}))\nexcept (ValueError, TypeError) as e:\n    print('no signature:', type(e).__name__)"
            )),
            ..Default::default()
        });
    }
    std::fs::write(
        layout.cases_file(),
        format!(
            "# Each case is a Python snippet run as `python3 -c`. Add behaviour cases like:\n\
             # [[case]]\n# name = \"example\"\n# args = [\"-c\", \"import {module} as m; print(m.some_function(1, 2))\"]\n"
        ),
    )?;
    crate::project::append_cases(
        layout,
        &cases,
        &format!("generated by `parity pylib {module}`"),
    )?;

    let crate_dir = root.join("rewrite");
    std::fs::create_dir_all(crate_dir.join("src"))?;
    std::fs::write(
        crate_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"{top}-rs\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [lib]\nname = \"{top}\"\ncrate-type = [\"cdylib\"]\n\n\
             [dependencies]\npyo3 = {{ version = \"0.25\", features = [\"extension-module\", \"abi3-py39\"] }}\n\n\
             [profile.release]\nlto = \"thin\"\ncodegen-units = 1\n\n[workspace]\n"
        ),
    )?;
    let todo: String = functions.iter().map(|f| format!("//   {f}\n")).collect();
    std::fs::write(
        crate_dir.join("src/lib.rs"),
        format!(
            "// The Rust rewrite of the Python module `{module}`, importable under the same name.\n\
             // Public functions of the original to port:\n{todo}\n\
             use pyo3::prelude::*;\n\n\
             #[pymodule]\nfn {top}(_m: &Bound<'_, PyModule>) -> PyResult<()> {{\n    Ok(())\n}}\n"
        ),
    )?;
    std::fs::write(crate_dir.join(".gitignore"), "/target\n/python\n")?;
    println!(
        "project for {module}: {} public functions, {} cases; next: `parity record`, then `parity rewrite`",
        functions.len(),
        cases.len()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_functions_follow_all_or_names() {
        let src = "def a(): pass\ndef _b(): pass\nclass C: pass\nasync def d(x): pass\n";
        assert_eq!(public_functions(src), ["a", "d"]);
        let src = "__all__ = ['d', 'missing']\ndef a(): pass\ndef d(): pass\n";
        assert_eq!(public_functions(src), ["d"]);
    }
}
