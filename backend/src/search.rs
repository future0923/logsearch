use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_insensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub cursor: Option<String>,
    #[serde(default)]
    pub file_ids: Vec<String>,
    #[serde(default = "default_context_before")]
    pub context_before: usize,
    #[serde(default = "default_context_after")]
    pub context_after: usize,
}

impl SearchRequest {
    pub fn validate(&self) -> Result<(), SearchValidationError> {
        if self.limit > 1_000 {
            return Err(SearchValidationError::LimitTooLarge(self.limit));
        }

        Ok(())
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum SearchValidationError {
    #[error("limit cannot exceed 1000, got {0}")]
    LimitTooLarge(usize),
    #[error("path cannot be empty")]
    EmptyPath,
    #[error("context window cannot exceed 500 lines before or after")]
    ContextTooLarge,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextLine {
    pub line_no: u64,
    pub offset: u64,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub file_id: String,
    pub path: String,
    pub line_no: u64,
    pub offset: u64,
    pub score: f32,
    pub kind: String,
    pub content: String,
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub context: Vec<ContextLine>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub total: usize,
    pub truncated: bool,
    pub has_next: bool,
    pub next_cursor: Option<String>,
    pub elapsed_ms: u128,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AroundRequest {
    pub path: String,
    pub line_no: u64,
    pub offset: u64,
    #[serde(default)]
    pub compressed: bool,
    #[serde(default = "default_around_before")]
    pub before: usize,
    #[serde(default = "default_around_after")]
    pub after: usize,
}

impl AroundRequest {
    pub fn validate(&self) -> Result<(), SearchValidationError> {
        if self.path.trim().is_empty() {
            return Err(SearchValidationError::EmptyPath);
        }

        if self.before > 500 || self.after > 500 {
            return Err(SearchValidationError::ContextTooLarge);
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AroundResponse {
    pub path: String,
    pub center_line_no: u64,
    pub center_offset: u64,
    pub lines: Vec<ContextLine>,
    pub has_before: bool,
    pub has_after: bool,
}

pub fn default_limit() -> usize {
    200
}

fn default_context_before() -> usize {
    2
}

fn default_context_after() -> usize {
    2
}

fn default_around_before() -> usize {
    120
}

fn default_around_after() -> usize {
    120
}

pub fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

pub fn contains_whole_word(haystack: &str, needle: &str, case_insensitive: bool) -> bool {
    if needle.is_empty() {
        return false;
    }

    let owned_haystack;
    let owned_needle;
    let (haystack, needle) = if case_insensitive {
        owned_haystack = haystack.to_lowercase();
        owned_needle = needle.to_lowercase();
        (owned_haystack.as_str(), owned_needle.as_str())
    } else {
        (haystack, needle)
    };

    let mut start = 0;
    while let Some(relative) = haystack[start..].find(needle) {
        let found_start = start + relative;
        let found_end = found_start + needle.len();
        let left_ok = found_start == 0 || !is_word_byte(haystack.as_bytes()[found_start - 1]);
        let right_ok = found_end == haystack.len() || !is_word_byte(haystack.as_bytes()[found_end]);

        if left_ok && right_ok {
            return true;
        }

        start = found_end;
    }

    false
}

pub fn build_regex(req: &SearchRequest) -> anyhow::Result<Regex> {
    let source = if req.regex {
        req.query.clone()
    } else {
        regex::escape(&req.query)
    };
    let source = if req.whole_word {
        format!(r"\b(?:{})\b", source)
    } else {
        source
    };

    Ok(RegexBuilder::new(&source)
        .case_insensitive(req.case_insensitive)
        .build()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_word_does_not_match_inside_longer_identifier() {
        assert!(contains_whole_word("disk error happened", "error", false));
        assert!(!contains_whole_word(
            "disk myerror happened",
            "error",
            false
        ));
        assert!(!contains_whole_word(
            "disk error_code happened",
            "error",
            false
        ));
    }

    #[test]
    fn whole_word_can_ignore_case() {
        assert!(contains_whole_word("Disk ERROR happened", "error", true));
        assert!(!contains_whole_word("Disk MYERROR happened", "error", true));
    }

    #[test]
    fn regex_builder_combines_case_and_word_options() {
        let req = SearchRequest {
            query: "error".to_string(),
            regex: false,
            case_insensitive: true,
            whole_word: true,
            limit: 10,
            cursor: None,
            file_ids: Vec::new(),
            context_before: 0,
            context_after: 0,
        };

        let regex = build_regex(&req).unwrap();

        assert!(regex.is_match("ERROR"));
        assert!(!regex.is_match("MYERROR"));
    }
}
