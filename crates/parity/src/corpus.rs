//! The behaviour corpus: one folder per case under `corpus/`, holding what the original did, raw (normalizers run
//! at comparison time, so changing one never needs a re-recording).
//!
//! ```text
//! corpus/<case>/case.json   input, exit, duration, file hashes, provenance
//! corpus/<case>/stdout
//! corpus/<case>/stderr
//! corpus/<case>/files/...   the working folder after the run
//! ```

use crate::project::{CaseInput, check_case_name, check_relative};
use crate::sandbox::{Exit, Observed};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

pub const FORMAT: u32 = 1;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CaseRecord {
    pub format: u32,
    pub input: CaseInput,
    pub exit: Exit,
    pub duration_ms: u64,
    /// Relative path → SHA-256 of the file after the run.
    pub files: BTreeMap<String, String>,
    /// The program that made the recording, as resolved then.
    pub recorded_with: String,
    pub parity_version: String,
}

pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn write(
    dir: &Path,
    input: &CaseInput,
    observed: &Observed,
    recorded_with: &str,
) -> Result<()> {
    if dir.exists() {
        std::fs::remove_dir_all(dir).with_context(|| format!("replacing {}", dir.display()))?;
    }
    std::fs::create_dir_all(dir.join("files"))?;
    std::fs::write(dir.join("stdout"), &observed.stdout)?;
    std::fs::write(dir.join("stderr"), &observed.stderr)?;
    for (path, content) in &observed.files {
        let target = dir.join("files").join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, content)?;
    }
    let record = CaseRecord {
        format: FORMAT,
        input: input.clone(),
        exit: observed.exit,
        duration_ms: observed.duration.as_millis() as u64,
        files: observed
            .files
            .iter()
            .map(|(k, v)| (k.clone(), sha256(v)))
            .collect(),
        recorded_with: recorded_with.to_string(),
        parity_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    let mut json = serde_json::to_vec_pretty(&record)?;
    json.push(b'\n');
    std::fs::write(dir.join("case.json"), json)?;
    Ok(())
}

/// Reads a recorded case back, checking every file against its recorded hash.
pub fn read(dir: &Path) -> Result<(CaseRecord, Observed)> {
    let json =
        std::fs::read(dir.join("case.json")).with_context(|| format!("{}", dir.display()))?;
    let record: CaseRecord =
        serde_json::from_slice(&json).with_context(|| format!("{}/case.json", dir.display()))?;
    if record.format != FORMAT {
        bail!(
            "{}: corpus format {} (this Parity reads {FORMAT})",
            dir.display(),
            record.format
        );
    }
    // A corpus can come from anyone: its input is checked like cases.toml before anything runs from it.
    check_case_name(&record.input.name)
        .and_then(|()| {
            record
                .input
                .files
                .keys()
                .try_for_each(|f| check_relative(f))
        })
        .with_context(|| format!("{}/case.json", dir.display()))?;
    let mut files = BTreeMap::new();
    for (path, hash) in &record.files {
        check_relative(path)?;
        let bytes = std::fs::read(dir.join("files").join(path))
            .with_context(|| format!("{}: recorded file {path}", dir.display()))?;
        if &sha256(&bytes) != hash {
            bail!(
                "{}: recorded file {path} was changed after recording",
                dir.display()
            );
        }
        files.insert(path.clone(), bytes);
    }
    let observed = Observed {
        stdout: std::fs::read(dir.join("stdout"))?,
        stderr: std::fs::read(dir.join("stderr"))?,
        exit: record.exit,
        files,
        duration: Duration::from_millis(record.duration_ms),
    };
    Ok((record, observed))
}

/// The recorded case names, sorted.
pub fn list(corpus: &Path) -> Result<Vec<String>> {
    if !corpus.exists() {
        return Ok(Vec::new());
    }
    let mut names: Vec<String> = std::fs::read_dir(corpus)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join("case.json").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_case_survives_the_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let case = dir.path().join("hello");
        let input = CaseInput {
            name: "hello".into(),
            args: vec!["-n".into()],
            ..Default::default()
        };
        let observed = Observed {
            stdout: b"hi\n".to_vec(),
            stderr: Vec::new(),
            exit: Exit::Code(0),
            files: [("out/a.txt".to_string(), b"\xff\x00".to_vec())].into(),
            duration: Duration::from_millis(12),
        };
        write(&case, &input, &observed, "/usr/bin/echo").unwrap();
        let (record, back) = read(&case).unwrap();
        assert_eq!(record.input, input);
        assert_eq!(back, observed);
        assert_eq!(list(dir.path()).unwrap(), ["hello"]);
    }

    #[test]
    fn tampered_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let observed = Observed {
            stdout: Vec::new(),
            stderr: Vec::new(),
            exit: Exit::Code(0),
            files: [("a".to_string(), b"1".to_vec())].into(),
            duration: Duration::ZERO,
        };
        let input = CaseInput {
            name: "a".into(),
            ..Default::default()
        };
        write(dir.path(), &input, &observed, "x").unwrap();
        std::fs::write(dir.path().join("files/a"), "2").unwrap();
        assert!(read(dir.path()).is_err());
    }
}
