//! Compares what a candidate did with what the original did, after normalizing both the same way.

use crate::normalize::{Normalizer, Target};
use crate::sandbox::Observed;
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Field {
    Exit,
    Stdout,
    Stderr,
    File(String),
    /// HTTP: the response status, a response header, the response body.
    Status,
    Header(String),
    Body,
}

impl std::fmt::Display for Field {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Field::Exit => f.write_str("exit status"),
            Field::Stdout => f.write_str("stdout"),
            Field::Stderr => f.write_str("stderr"),
            Field::File(p) => write!(f, "file {p}"),
            Field::Status => f.write_str("status"),
            Field::Header(h) => write!(f, "header {h}"),
            Field::Body => f.write_str("body"),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Divergence {
    pub field: Field,
    /// Normalized, as text (invalid UTF-8 shown lossily).
    pub expected: String,
    pub actual: String,
}

pub const ABSENT: &str = "<absent>";

pub fn compare(expected: &Observed, actual: &Observed, norm: &Normalizer) -> Vec<Divergence> {
    let mut out = Vec::new();
    if expected.exit != actual.exit {
        out.push(Divergence {
            field: Field::Exit,
            expected: expected.exit.to_string(),
            actual: actual.exit.to_string(),
        });
    }
    let streams = [
        (
            Field::Stdout,
            Target::Stdout,
            &expected.stdout,
            &actual.stdout,
        ),
        (
            Field::Stderr,
            Target::Stderr,
            &expected.stderr,
            &actual.stderr,
        ),
    ];
    for (field, target, e, a) in streams {
        let (e, a) = (norm.apply(target, e), norm.apply(target, a));
        if e != a {
            out.push(Divergence {
                field,
                expected: text(&e),
                actual: text(&a),
            });
        }
    }
    let paths: std::collections::BTreeSet<&String> =
        expected.files.keys().chain(actual.files.keys()).collect();
    for path in paths {
        let e = expected
            .files
            .get(path)
            .map(|b| norm.apply(Target::Files, b));
        let a = actual.files.get(path).map(|b| norm.apply(Target::Files, b));
        if e != a {
            let show = |v: Option<Vec<u8>>| v.map_or_else(|| ABSENT.to_string(), |b| text(&b));
            out.push(Divergence {
                field: Field::File(path.clone()),
                expected: show(e),
                actual: show(a),
            });
        }
    }
    out
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalize::Rule;
    use crate::sandbox::Exit;
    use std::time::Duration;

    fn observed(stdout: &str, exit: i32, files: &[(&str, &str)]) -> Observed {
        Observed {
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
            exit: Exit::Code(exit),
            files: files
                .iter()
                .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
                .collect(),
            duration: Duration::ZERO,
        }
    }

    #[test]
    fn identical_runs_match() {
        let n = Normalizer::new(&[]).unwrap();
        let a = observed("x\n", 0, &[("f", "1")]);
        assert!(compare(&a, &a.clone(), &n).is_empty());
    }

    #[test]
    fn every_kind_of_difference_is_reported() {
        let n = Normalizer::new(&[]).unwrap();
        let e = observed("x\n", 0, &[("kept", "1"), ("gone", "2")]);
        let a = observed("y\n", 1, &[("kept", "9"), ("new", "3")]);
        let fields: Vec<Field> = compare(&e, &a, &n).into_iter().map(|d| d.field).collect();
        assert_eq!(
            fields,
            [
                Field::Exit,
                Field::Stdout,
                Field::File("gone".into()),
                Field::File("kept".into()),
                Field::File("new".into())
            ]
        );
    }

    #[test]
    fn normalizers_hide_declared_noise_only() {
        let n = Normalizer::new(&[Rule {
            name: "pid".into(),
            pattern: r"\d+".into(),
            replace: "N".into(),
            applies_to: vec![],
        }])
        .unwrap();
        assert!(compare(&observed("pid 1", 0, &[]), &observed("pid 2", 0, &[]), &n).is_empty());
        assert_eq!(
            compare(&observed("pid 1", 0, &[]), &observed("tid 2", 0, &[]), &n).len(),
            1
        );
    }
}
