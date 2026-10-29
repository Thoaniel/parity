// Python `re.match` / `re.search` on top of the regex crate.

pub struct Regex {
    any: regex::Regex,
    anchored: regex::Regex,
}

impl Regex {
    fn build(pat: &str, ci: bool) -> Option<Regex> {
        let b = |p: &str| regex::RegexBuilder::new(p).case_insensitive(ci).build().ok();
        Some(Regex { any: b(pat)?, anchored: b(&format!("^(?:{pat})"))? })
    }
    pub fn new(pat: &str) -> Option<Regex> {
        Self::build(pat, false)
    }
    pub fn new_ci(pat: &str) -> Option<Regex> {
        Self::build(pat, true)
    }
    /// `re.match`: a match anchored at the start.
    pub fn match_start(&self, s: &str) -> bool {
        self.anchored.is_match(s)
    }
    /// `re.search`.
    pub fn search(&self, s: &str) -> bool {
        self.any.is_match(s)
    }
}
