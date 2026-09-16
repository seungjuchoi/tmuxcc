use crate::agents::MonitoredAgent;

/// The `/` search: a query that filters the agent list and highlights the
/// preview.
///
/// Matching is case-insensitive and word-wise: every whitespace-separated
/// term of the query must appear somewhere in the agent's title, session,
/// window name, path or captured pane content. Terms may hit different
/// fields ("fix tmuxcc" matches a pane whose title says "fix" and whose
/// window is called "tmuxcc").
#[derive(Debug, Clone, Default)]
pub struct Search {
    /// The text as typed
    query: String,
    /// Lower-cased terms derived from `query`
    terms: Vec<String>,
    /// True while keystrokes go into the query instead of the list
    pub editing: bool,
}

impl Search {
    /// Opens the input line; the previous query is kept so `/` can refine it
    pub fn begin(&mut self) {
        self.editing = true;
    }

    /// Closes the input line, keeping the filter in place
    pub fn accept(&mut self) {
        self.editing = false;
    }

    /// Drops the query and closes the input line
    pub fn clear(&mut self) {
        self.editing = false;
        self.set_query(String::new());
    }

    pub fn push(&mut self, c: char) {
        let mut query = self.query.clone();
        query.push(c);
        self.set_query(query);
    }

    pub fn pop(&mut self) {
        let mut query = self.query.clone();
        query.pop();
        self.set_query(query);
    }

    pub fn set_query(&mut self, query: String) {
        self.terms = query
            .split_whitespace()
            .map(|term| term.to_lowercase())
            .collect();
        self.query = query;
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Lower-cased search terms; empty when nothing is being searched for
    pub fn terms(&self) -> &[String] {
        &self.terms
    }

    /// True when a query narrows the list (editing or not)
    pub fn is_active(&self) -> bool {
        !self.terms.is_empty()
    }

    /// True when the agent satisfies every term of the query. An empty query
    /// matches everything.
    pub fn matches(&self, agent: &MonitoredAgent) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        let haystack = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            agent.title,
            agent.session,
            agent.window_name,
            agent.path,
            agent.target,
            agent.last_content
        )
        .to_lowercase();
        self.terms
            .iter()
            .all(|term| haystack.contains(term.as_str()))
    }
}

/// Byte ranges of `text` matched by any of the `terms`, case-insensitively, merged so overlapping hits form one range. Ranges
/// are on the original `text`, so they can be sliced directly.
pub fn match_ranges(text: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if terms.is_empty() || text.is_empty() {
        return Vec::new();
    }
    // Lower-casing can change byte lengths (e.g. 'İ'); mapping back through
    // char boundaries keeps the ranges valid on the original text.
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let lowered: Vec<String> = chars
        .iter()
        .map(|(_, c)| c.to_lowercase().collect())
        .collect();
    let mut lowered_offsets = Vec::with_capacity(lowered.len() + 1);
    let mut lowered_text = String::new();
    for piece in &lowered {
        lowered_offsets.push(lowered_text.len());
        lowered_text.push_str(piece);
    }
    lowered_offsets.push(lowered_text.len());

    // Maps a byte offset in the lowered text to the byte offset of the
    // original char that starts at or before it
    let to_original = |lowered_pos: usize| -> usize {
        let idx = match lowered_offsets.binary_search(&lowered_pos) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        if idx >= chars.len() {
            text.len()
        } else {
            chars[idx].0
        }
    };

    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let term = term.to_lowercase();
        if term.is_empty() {
            continue;
        }
        let mut from = 0;
        while let Some(pos) = lowered_text[from..].find(term.as_str()) {
            let start = from + pos;
            let end = start + term.len();
            ranges.push((to_original(start), to_original(end)));
            from = end;
        }
    }
    ranges.sort_unstable();

    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::AgentType;

    fn agent(title: &str, window: &str, content: &str) -> MonitoredAgent {
        let mut agent = MonitoredAgent::new(
            "1".to_string(),
            "main:0.0".to_string(),
            "main".to_string(),
            0,
            window.to_string(),
            0,
            "/home/user/project".to_string(),
            AgentType::ClaudeCode,
            1000,
        );
        agent.title = title.to_string();
        agent.last_content = content.to_string();
        agent
    }

    #[test]
    fn empty_query_matches_everything() {
        let search = Search::default();
        assert!(!search.is_active());
        assert!(search.matches(&agent("", "", "")));
    }

    #[test]
    fn matches_title_window_and_content_case_insensitively() {
        let mut search = Search::default();
        let a = agent("✳ Fix the login bug", "backend", "running cargo test\n");

        search.set_query("LOGIN".to_string());
        assert!(search.matches(&a));
        search.set_query("backend".to_string());
        assert!(search.matches(&a));
        search.set_query("cargo".to_string());
        assert!(search.matches(&a));
        search.set_query("frontend".to_string());
        assert!(!search.matches(&a));
    }

    #[test]
    fn every_term_must_match_but_fields_may_differ() {
        let mut search = Search::default();
        let a = agent("Fix the login bug", "backend", "");
        search.set_query("fix backend".to_string());
        assert!(search.matches(&a));
        search.set_query("fix frontend".to_string());
        assert!(!search.matches(&a));
    }

    #[test]
    fn editing_keeps_the_query_and_clear_drops_it() {
        let mut search = Search::default();
        search.begin();
        search.push('a');
        search.push('b');
        assert!(search.editing);
        assert_eq!(search.query(), "ab");
        search.pop();
        assert_eq!(search.query(), "a");
        search.accept();
        assert!(!search.editing);
        assert!(search.is_active());
        search.clear();
        assert!(!search.is_active());
        assert_eq!(search.query(), "");
    }

    #[test]
    fn match_ranges_are_case_insensitive_and_merged() {
        let terms = vec!["ab".to_string(), "bc".to_string()];
        assert_eq!(match_ranges("xABCx abc", &terms), vec![(1, 4), (6, 9)]);
        assert!(match_ranges("nothing", &terms).is_empty());
        assert!(match_ranges("abc", &[]).is_empty());
    }

    #[test]
    fn match_ranges_survive_multibyte_text() {
        let terms = vec!["버그".to_string()];
        let text = "로그인 버그 수정";
        let ranges = match_ranges(text, &terms);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&text[ranges[0].0..ranges[0].1], "버그");
    }
}
