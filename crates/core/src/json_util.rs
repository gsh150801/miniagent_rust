//! Shared JSON extraction and repair utilities.
//!
//! LLM outputs frequently wrap JSON in markdown fences or get truncated
//! mid-generation. These helpers normalise and repair such output so that
//! `serde_json::from_str` has the best chance of succeeding.

/// Strip ```` ```json ```` / ```` ``` ```` markdown fences surrounding JSON.
pub fn strip_markdown_fences(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let without_start = trimmed
            .trim_start_matches("```json")
            .trim_start_matches("```JSON")
            .trim_start_matches("```");
        without_start.trim_end_matches("```").trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Attempt to fix truncated JSON by closing open strings, braces, and brackets.
///
/// This prevents "EOF while parsing a string" errors from token-limit truncation.
pub fn fix_truncated_json(s: &str) -> String {
    let mut fixed = s.to_string();
    let mut in_string = false;
    let mut escape_next = false;
    let mut open_curly = 0i32;
    let mut open_square = 0i32;

    for ch in fixed.chars() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => open_curly += 1,
            '}' => open_curly -= 1,
            '[' => open_square += 1,
            ']' => open_square -= 1,
            _ => {}
        }
    }

    if in_string {
        fixed.push('"');
    }

    for _ in 0..open_square.max(0) {
        fixed.push(']');
    }
    for _ in 0..open_curly.max(0) {
        fixed.push('}');
    }

    fixed
}

/// Extract the last balanced top-level JSON object `{ ... }` from a string.
///
/// Tracks brace depth and string state to correctly handle nested objects.
/// Returns the last complete top-level object found, or `None`.
pub fn extract_json_object(text: &str) -> Option<String> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;
    let mut last_start: Option<usize> = None;
    let mut last_range: Option<std::ops::Range<usize>> = None;

    for (i, ch) in text.char_indices() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if ch == '\\' {
            escape_next = true;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '{' => {
                if depth == 0 {
                    last_start = Some(i);
                }
                depth += 1;
            }
            '}'
                if depth > 0 => {
                    depth -= 1;
                    if depth == 0
                        && let Some(start) = last_start {
                            last_range = Some(start..i + 1);
                        }
                }
            _ => {}
        }
    }

    last_range.map(|r| text[r].to_string())
}

/// Full pipeline: strip fences → fix truncation → extract JSON object.
///
/// Convenience for the common case where an LLM response contains
/// fenced, possibly-truncated JSON among other text.
pub fn extract_and_repair(text: &str) -> String {
    let cleaned = strip_markdown_fences(text);
    let repaired = fix_truncated_json(&cleaned);
    extract_json_object(&repaired).unwrap_or_else(|| repaired.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_json_fence() {
        assert_eq!(
            strip_markdown_fences("```json\n{\"a\": 1}\n```"),
            "{\"a\": 1}"
        );
    }

    #[test]
    fn test_strip_uppercase_fence() {
        assert_eq!(
            strip_markdown_fences("```JSON\n{\"a\": 1}\n```"),
            "{\"a\": 1}"
        );
    }

    #[test]
    fn test_strip_plain_fence() {
        assert_eq!(
            strip_markdown_fences("```\n{\"a\": 1}\n```"),
            "{\"a\": 1}"
        );
    }

    #[test]
    fn test_strip_no_fence() {
        assert_eq!(strip_markdown_fences("  {\"a\": 1}  "), "{\"a\": 1}");
    }

    #[test]
    fn test_fix_truncated_string() {
        let truncated = r#"{"key": "val"#;
        let fixed = fix_truncated_json(truncated);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok());
    }

    #[test]
    fn test_fix_truncated_array() {
        let truncated = r#"{"list": [1, 2, 3"#;
        let fixed = fix_truncated_json(truncated);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok());
    }

    #[test]
    fn test_fix_truncated_nested() {
        let truncated = r#"{"outer": {"inner": "test"#;
        let fixed = fix_truncated_json(truncated);
        assert!(serde_json::from_str::<serde_json::Value>(&fixed).is_ok());
    }

    #[test]
    fn test_fix_complete_json_unchanged() {
        let complete = r#"{"key": "value"}"#;
        assert_eq!(fix_truncated_json(complete), complete);
    }

    #[test]
    fn test_extract_json_object_simple() {
        let text = r#"Some text {"a": 1} more text"#;
        let result = extract_json_object(text);
        assert_eq!(result.as_deref(), Some(r#"{"a": 1}"#));
    }

    #[test]
    fn test_extract_json_object_last() {
        let text = r#"First {"a": 1} and second {"b": 2}"#;
        let result = extract_json_object(text);
        assert_eq!(result.as_deref(), Some(r#"{"b": 2}"#));
    }

    #[test]
    fn test_extract_json_object_nested() {
        let text = r#"Result: {"outer": {"inner": 42}}"#;
        let result = extract_json_object(text);
        assert_eq!(result.as_deref(), Some(r#"{"outer": {"inner": 42}}"#));
    }

    #[test]
    fn test_extract_json_object_none() {
        assert!(extract_json_object("no json here").is_none());
    }

    #[test]
    fn test_extract_and_repair_fenced_truncated() {
        // Simpler truncation: string cut mid-value in a flat object.
        // fix_truncated_json closes the string and braces.
        let raw = "```json\n{\"goal\": \"research CRISPR";
        let result = extract_and_repair(raw);
        assert!(
            serde_json::from_str::<serde_json::Value>(&result).is_ok(),
            "Expected valid JSON, got: {result}"
        );
    }
}
