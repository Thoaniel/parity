//! Normalizers: declared regex replacements that hide honest nondeterminism (timestamps, PIDs, temp paths) before
//! outputs are compared. They work on bytes, so binary output passes through unchanged where nothing matches.

use anyhow::{Context, Result};
use regex::bytes::Regex;
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Stdout,
    Stderr,
    Files,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    pub pattern: String,
    pub replace: String,
    /// Where it applies; empty means everywhere.
    #[serde(default)]
    pub applies_to: Vec<Target>,
}

pub struct Normalizer {
    rules: Vec<(Regex, Vec<u8>, Vec<Target>)>,
}

impl Normalizer {
    pub fn new(rules: &[Rule]) -> Result<Self> {
        let rules = rules
            .iter()
            .map(|r| {
                let re =
                    Regex::new(&r.pattern).with_context(|| format!("normalizer {:?}", r.name))?;
                Ok((re, r.replace.clone().into_bytes(), r.applies_to.clone()))
            })
            .collect::<Result<_>>()?;
        Ok(Self { rules })
    }

    pub fn apply(&self, target: Target, bytes: &[u8]) -> Vec<u8> {
        let mut out = bytes.to_vec();
        for (re, replace, targets) in &self.rules {
            if targets.is_empty() || targets.contains(&target) {
                out = re.replace_all(&out, replace.as_slice()).into_owned();
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, replace: &str, applies_to: Vec<Target>) -> Rule {
        Rule {
            name: "test".into(),
            pattern: pattern.into(),
            replace: replace.into(),
            applies_to,
        }
    }

    #[test]
    fn replaces_only_where_it_applies() {
        let n = Normalizer::new(&[rule(r"pid \d+", "pid <PID>", vec![Target::Stderr])]).unwrap();
        assert_eq!(n.apply(Target::Stderr, b"pid 4242 done"), b"pid <PID> done");
        assert_eq!(n.apply(Target::Stdout, b"pid 4242 done"), b"pid 4242 done");
    }

    #[test]
    fn rules_compose_in_order_and_keep_binary_bytes() {
        let n = Normalizer::new(&[rule("a", "b", vec![]), rule("b", "c", vec![])]).unwrap();
        assert_eq!(n.apply(Target::Files, b"a\xff"), b"c\xff");
    }

    #[test]
    fn bad_patterns_name_the_rule() {
        let err = Normalizer::new(&[rule("(", "", vec![])]).err().unwrap();
        assert!(format!("{err:#}").contains("\"test\""));
    }
}
