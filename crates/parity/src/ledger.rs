//! The port ledger (`ledger.toml`): every flag, subcommand and source function of the original, each with a
//! verdict, like Horizon's port ledger. Built from the surface map, kept by the agent loop and the agent itself
//! (`parity ledger set`), and synced from replay reports: an item whose generated case matches is ported.

use crate::discover::Surface;
use crate::project::Layout;
use crate::report::Report;
use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::LazyLock;

pub const LEDGER_FILE: &str = "ledger.toml";

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Missing,
    Partial,
    Ported,
    /// Done as part of another item.
    Folded,
    /// The rewrite doesn't need it (an implementation detail of the original).
    NotNeeded,
}

impl std::str::FromStr for Status {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "missing" => Status::Missing,
            "partial" => Status::Partial,
            "ported" => Status::Ported,
            "folded" => Status::Folded,
            "not_needed" | "not-needed" => Status::NotNeeded,
            other => bail!("status {other:?}: missing, partial, ported, folded or not_needed"),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Item {
    /// `flag:--format`, `subcommand:fetch`, `fn:yt_dlp/utils.py:sanitize_filename`.
    pub id: String,
    pub kind: String,
    #[serde(default)]
    pub status: Status,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Ledger {
    #[serde(rename = "item", default)]
    pub items: Vec<Item>,
}

fn path(layout: &Layout) -> PathBuf {
    layout.root.join(LEDGER_FILE)
}

impl Ledger {
    pub fn load(layout: &Layout) -> Result<Self> {
        let p = path(layout);
        if !p.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&p)?;
        toml::from_str(&text).with_context(|| format!("{}", p.display()))
    }

    pub fn save(&self, layout: &Layout) -> Result<()> {
        let header = "# The port ledger: every part of the original and its verdict.\n\
                      # Statuses: missing, partial, ported, folded, not_needed. Set with `parity ledger set`.\n\n";
        std::fs::write(path(layout), format!("{header}{}", toml::to_string(self)?))?;
        Ok(())
    }

    /// Adds the surface's items that aren't listed yet; existing verdicts stay.
    pub fn merge_surface(&mut self, s: &Surface) -> usize {
        let mut new = Vec::new();
        for f in &s.flags {
            if let Some(name) = f.long.as_ref().or(f.short.as_ref()) {
                new.push((format!("flag:{name}"), "flag"));
            }
        }
        for sub in &s.subcommands {
            new.push((format!("subcommand:{sub}"), "subcommand"));
        }
        for source in &s.sources {
            let Ok(text) = std::fs::read_to_string(source) else {
                continue;
            };
            let short = short_source(source);
            for name in top_level_functions(&text) {
                new.push((format!("fn:{short}:{name}"), "function"));
            }
        }
        let mut added = 0;
        for (id, kind) in new {
            if !self.items.iter().any(|i| i.id == id) {
                self.items.push(Item {
                    id,
                    kind: kind.into(),
                    status: Status::Missing,
                    note: String::new(),
                });
                added += 1;
            }
        }
        added
    }

    pub fn set(&mut self, id: &str, status: Status, note: Option<String>) -> Result<()> {
        let item = self
            .items
            .iter_mut()
            .find(|i| i.id == id)
            .with_context(|| format!("no ledger item {id:?}"))?;
        item.status = status;
        if let Some(n) = note {
            item.note = n;
        }
        Ok(())
    }

    /// Marks flags and subcommands whose generated cases all match as ported (never downgrades a verdict).
    pub fn sync_from_report(&mut self, report: &Report) -> usize {
        let ok: BTreeMap<&str, bool> = report
            .cases
            .iter()
            .map(|c| (c.name.as_str(), c.divergences.is_empty()))
            .collect();
        let mut changed = 0;
        for item in self
            .items
            .iter_mut()
            .filter(|i| i.status == Status::Missing)
        {
            let case = match item.id.split_once(':') {
                Some(("flag", name)) => format!("gen-flag{name}"),
                Some(("subcommand", name)) => format!("gen-{name}-help"),
                _ => continue,
            };
            if ok.get(case.as_str()) == Some(&true) {
                item.status = Status::Ported;
                changed += 1;
            }
        }
        changed
    }

    pub fn summary(&self) -> String {
        let mut by: BTreeMap<(&str, Status), usize> = BTreeMap::new();
        for i in &self.items {
            *by.entry((i.kind.as_str(), i.status)).or_default() += 1;
        }
        let mut kinds: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for ((kind, status), n) in by {
            kinds
                .entry(kind)
                .or_default()
                .push(format!("{n} {status:?}").to_lowercase());
        }
        let done = self
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i.status,
                    Status::Ported | Status::Folded | Status::NotNeeded
                )
            })
            .count();
        let mut s = format!("{done}/{} items done\n", self.items.len());
        for (kind, parts) in kinds {
            s.push_str(&format!("  {kind}: {}\n", parts.join(", ")));
        }
        s
    }
}

/// Module-level `def`/`async def` and `class` names (not indented), and module-level JS `function` names.
fn top_level_functions(text: &str) -> Vec<String> {
    static DEF: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?m)^(?:async\s+def|def|class|(?:export\s+)?(?:async\s+)?function)\s+([A-Za-z_$][\w$]*)").unwrap()
    });
    DEF.captures_iter(text).map(|c| c[1].to_string()).collect()
}

/// A source path from its package folder on (`yt_dlp/utils/_utils.py`), for readable ids.
fn short_source(source: &str) -> String {
    let parts: Vec<&str> = source.split('/').collect();
    let start = parts
        .iter()
        .rposition(|p| p.ends_with("-packages") || *p == "node_modules" || *p == "lib")
        .map_or(parts.len().saturating_sub(2), |i| i + 1);
    parts[start..].join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::Flag;

    #[test]
    fn merges_without_losing_verdicts() {
        let s = Surface {
            flags: vec![Flag {
                long: Some("--x".into()),
                ..Default::default()
            }],
            subcommands: vec!["run".into()],
            ..Default::default()
        };
        let mut l = Ledger::default();
        assert_eq!(l.merge_surface(&s), 2);
        l.set("flag:--x", Status::Ported, Some("done".into()))
            .unwrap();
        assert_eq!(l.merge_surface(&s), 0);
        assert_eq!(l.items[0].status, Status::Ported);
        assert!(l.set("flag:--nope", Status::Ported, None).is_err());
    }

    #[test]
    fn finds_top_level_definitions_only() {
        let py = "def a():\n    def inner(): pass\nclass B:\n    def m(self): pass\nasync def c(): pass\n";
        assert_eq!(top_level_functions(py), ["a", "B", "c"]);
        assert_eq!(top_level_functions("export async function go() {}"), ["go"]);
    }

    #[test]
    fn short_sources_start_at_the_package() {
        assert_eq!(
            short_source("/usr/lib/python3.13/site-packages/yt_dlp/utils.py"),
            "yt_dlp/utils.py"
        );
        assert_eq!(short_source("/x/y/z.py"), "y/z.py");
    }
}
