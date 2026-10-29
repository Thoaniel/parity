//! The parity report: how many recorded cases the candidate matches, every divergence with a diff, and how fast
//! each side was. Printed as text and optionally written as JSON.

use crate::compare::Divergence;
use serde::Serialize;
use similar::TextDiff;
use std::fmt::Write;

#[derive(Clone, Debug, Serialize)]
pub struct CaseResult {
    pub name: String,
    pub divergences: Vec<Divergence>,
    pub original_ms: u64,
    pub candidate_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Report {
    pub original: String,
    pub candidate: String,
    pub cases: Vec<CaseResult>,
}

impl Report {
    pub fn matching(&self) -> usize {
        self.cases
            .iter()
            .filter(|c| c.divergences.is_empty())
            .count()
    }

    pub fn render(&self, max_diff_lines: usize) -> String {
        let mut s = String::new();
        let total = self.cases.len();
        let ok = self.matching();
        let pct = if total == 0 {
            100.0
        } else {
            ok as f64 * 100.0 / total as f64
        };
        let _ = writeln!(s, "Parity: {} against {}", self.candidate, self.original);
        let _ = writeln!(s, "{ok}/{total} cases match ({pct:.1} %)\n");
        for case in self.cases.iter().filter(|c| !c.divergences.is_empty()) {
            let _ = writeln!(s, "✗ {}", case.name);
            for d in &case.divergences {
                let _ = writeln!(s, "  {}:", d.field);
                let diff = TextDiff::from_lines(&d.expected, &d.actual)
                    .unified_diff()
                    .context_radius(2)
                    .header("original", "candidate")
                    .to_string();
                let lines: Vec<&str> = diff.lines().collect();
                for line in lines.iter().take(max_diff_lines) {
                    let _ = writeln!(s, "    {line}");
                }
                if lines.len() > max_diff_lines {
                    let _ = writeln!(s, "    … {} more lines", lines.len() - max_diff_lines);
                }
            }
        }
        let original: u64 = self.cases.iter().map(|c| c.original_ms).sum();
        let candidate: u64 = self.cases.iter().map(|c| c.candidate_ms).sum();
        let _ = writeln!(
            s,
            "\nTime: original {original} ms, candidate {candidate} ms (wall clock, all cases)"
        );
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compare::Field;

    #[test]
    fn renders_counts_diffs_and_times() {
        let report = Report {
            original: "jq".into(),
            candidate: "jaq".into(),
            cases: vec![
                CaseResult {
                    name: "ok".into(),
                    divergences: vec![],
                    original_ms: 10,
                    candidate_ms: 2,
                },
                CaseResult {
                    name: "bad".into(),
                    divergences: vec![Divergence {
                        field: Field::Stdout,
                        expected: "1\n".into(),
                        actual: "1.0\n".into(),
                    }],
                    original_ms: 5,
                    candidate_ms: 1,
                },
            ],
        };
        let text = report.render(20);
        assert!(text.contains("1/2 cases match (50.0 %)"));
        assert!(text.contains("✗ bad"));
        assert!(text.contains("-1") && text.contains("+1.0"));
        assert!(text.contains("original 15 ms, candidate 3 ms"));
    }
}
