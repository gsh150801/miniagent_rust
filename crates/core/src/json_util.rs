//! Shared JSON extraction and repair utilities.
//!
//! LLM outputs frequently wrap JSON in markdown fences or get truncated
//! mid-generation. These helpers normalise and repair such output so that
//! `serde_json::from_str` has the best chance of succeeding.

/// Remove `<think>...</think>` reasoning blocks from model output.
///
/// Reasoning-style models (DeepSeek reasoner, MiniMax M3, …) may emit chain-of-
/// thought inline. When raw output is used *as data* (e.g. a search query, a
/// file path, a script), the reasoning must be stripped first or downstream
/// systems receive truncated thinking text. Handles both closed blocks and an
/// unterminated `<think>` prefix (truncation mid-reasoning).
pub fn strip_reasoning_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(start) = rest.find("<think>") {
        out.push_str(&rest[..start]);
        let after = &rest[start + "<think>".len()..];
        match after.find("</think>") {
            Some(end) => rest = &after[end + "</think>".len()..],
            None => {
                // Unterminated reasoning block: drop everything after it.
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Strip ```` ```json ```` / ```` ``` ```` markdown fences surrounding JSON.
pub fn strip_markdown_fences(s: &str) -> String {    let trimmed = s.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed.to_string();
    };
    // Drop the fence's language tag (```python, ```json, …). The tag is the
    // remainder of the first line when it looks like an identifier; code that
    // starts on the same line as the fence (rare, e.g. "```{") is kept.
    let content = match rest.find('\n') {
        Some(nl) => {
            let tag = &rest[..nl];
            let is_tag = !tag.is_empty()
                && tag.len() <= 16
                && tag
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '_'));
            if is_tag { &rest[nl + 1..] } else { rest }
        }
        // single-line fence (```code```): nothing to strip beyond the fences
        None => rest,
    };
    content.trim_end_matches("```").trim().to_string()
}

/// Attempt to fix truncated JSON by closing open strings, braces, and brackets.
///
/// This prevents "EOF while parsing a string" errors from token-limit truncation.
/// Closers are appended in reverse opening order (stack order): appending all
/// `]` before all `}` produced `…]"hyp"]}}`-style output where an array closed
/// inside an element object, which is exactly the `expected ',' or '}'`
/// parse failure seen on long debate-refinement payloads.
pub fn fix_truncated_json(s: &str) -> String {
    let mut fixed = s.to_string();
    let mut in_string = false;
    let mut escape_next = false;
    let mut open: Vec<char> = Vec::new();

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
            '{' => open.push('}'),
            '[' => open.push(']'),
            '}' | ']' => {
                open.pop();
            }
            _ => {}
        }
    }

    if escape_next {
        // Truncated right after a backslash: neutralise it so the closing
        // quote below isn't eaten as an escape.
        fixed.push(' ');
    }
    if in_string {
        fixed.push('"');
    }

    while let Some(closer) = open.pop() {
        fixed.push(closer);
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

/// Remove trailing commas from JSON (`,}` / `,]` with whitespace between).
///
/// LLMs frequently emit `{"a": [1, 2,]}`-style output which strict parsers
/// reject; observed live as a whole-paper loss in KG extraction. String-aware:
/// commas inside string literals are left untouched.
pub fn strip_trailing_commas(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape_next = false;
    let mut i = 0;
    while i < bytes.len() {
        let ch = s[i..].chars().next().unwrap();
        if escape_next {
            escape_next = false;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }
        if ch == ',' && !in_string {
            // Look ahead: skip whitespace; a closer means this comma trails.
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                i += 1; // drop the comma
                continue;
            }
        }
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Replace bare `NaN` / `Infinity` / `-Infinity` literals (illegal in JSON,
/// but emitted by LLMs) with `null`, outside string literals.
pub fn strip_invalid_json_numbers(s: &str) -> String {
    const TOKENS: [&str; 3] = ["NaN", "Infinity", "-Infinity"];
    let mut out = String::with_capacity(s.len());
    let mut in_string = false;
    let mut escape_next = false;
    let mut rest = s;
    'outer: while !rest.is_empty() {
        let ch = rest.chars().next().unwrap();
        if escape_next {
            escape_next = false;
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        if ch == '\\' && in_string {
            escape_next = true;
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            out.push(ch);
            rest = &rest[ch.len_utf8()..];
            continue;
        }
        if !in_string {
            for tok in TOKENS {
                // Only replace token boundaries: preceded by ':'/','/'[' or
                // whitespace, followed by ','/'}'/']/whitespace.
                if rest.starts_with(tok) {
                    let before_ok = out
                        .chars()
                        .last()
                        .is_none_or(|c| c == ':' || c == ',' || c == '[' || c.is_whitespace());
                    let after = &rest[tok.len()..];
                    let after_ok = after
                        .chars()
                        .next()
                        .is_none_or(|c| c == ',' || c == '}' || c == ']' || c.is_whitespace());
                    if before_ok && after_ok {
                        out.push_str("null");
                        rest = after;
                        continue 'outer;
                    }
                }
            }
        }
        out.push(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Full pipeline: strip fences → fix truncation → extract JSON object.
///
/// Convenience for the common case where an LLM response contains fenced,
/// possibly-truncated JSON among other text.
pub fn extract_and_repair(text: &str) -> String {
    let cleaned = strip_markdown_fences(&strip_reasoning_tags(text));
    let repaired = strip_trailing_commas(&fix_truncated_json(&cleaned));
    let repaired = strip_invalid_json_numbers(&repaired);
    extract_json_object(&repaired).unwrap_or_else(|| repaired.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_reasoning_closed() {
        assert_eq!(
            strip_reasoning_tags("<think>reasoning here</think>actual query"),
            "actual query"
        );
    }

    #[test]
    fn test_strip_reasoning_unterminated() {
        // truncated mid-think: everything after <think> is dropped
        assert_eq!(strip_reasoning_tags("prefix <think>half of a thou"), "prefix");
    }

    #[test]
    fn test_strip_reasoning_multiple() {
        assert_eq!(
            strip_reasoning_tags("<think>a</think>x<think>b</think>y"),
            "xy"
        );
    }

    #[test]
    fn test_strip_reasoning_none() {
        assert_eq!(strip_reasoning_tags("  plain text  "), "plain text");
    }

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
    fn test_strip_python_fence_keeps_code_without_tag() {
        // regression: the language tag used to leak into the code body,
        // producing a leading `python` line that broke generated scripts
        assert_eq!(
            strip_markdown_fences("```python\nimport numpy as np\nprint(1)\n```"),
            "import numpy as np\nprint(1)"
        );
        assert_eq!(strip_markdown_fences("```Python\nimport os\n```"), "import os");
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

    #[test]
    fn test_strip_trailing_commas_objects_and_arrays() {
        let raw = "{\n  \"a\": [1, 2, 3,],\n  \"b\": {\"c\": 1,},\n}";
        let fixed = strip_trailing_commas(raw);
        assert!(
            serde_json::from_str::<serde_json::Value>(&fixed).is_ok(),
            "got: {fixed}"
        );
    }

    #[test]
    fn test_strip_trailing_commas_preserves_in_string_commas() {
        let raw = r#"{"a": "hello,}", "b": "x, ]", "c": [1,]}"#;
        let fixed = strip_trailing_commas(raw);
        let v: serde_json::Value = serde_json::from_str(&fixed).expect(&fixed);
        assert_eq!(v["a"], "hello,}");
        assert_eq!(v["b"], "x, ]");
        assert_eq!(v["c"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn test_strip_trailing_commas_after_truncation_fix() {
        // `{"a": [1, 2,` → fix_truncated_json closes to `{"a": [1, 2,]` →
        // strip removes the pre-`]` comma → valid JSON.
        let raw = "{\"a\": [1, 2,";
        let result = extract_and_repair(raw);
        assert!(
            serde_json::from_str::<serde_json::Value>(&result).is_ok(),
            "got: {result}"
        );
    }

    #[test]
    fn test_extract_and_repair_real_kg_style_trailing_comma() {
        // Shape observed live: a trailing comma after the last relation object.
        let raw = "{\n  \"entities\": [{\"name\": \"X\", \"type\": \"Gene\",}],\n  \"relations\": [{\"from\": \"X\", \"to\": \"Y\", \"type\": \"activates\", \"evidence\": \"e\",}]\n}";
        let result = extract_and_repair(raw);
        let v: serde_json::Value = serde_json::from_str(&result).expect(&result);
        assert_eq!(v["entities"][0]["name"], "X");
    }

    #[test]
    fn test_strip_invalid_json_numbers() {
        let raw = "{\"a\": NaN, \"b\": [1, Infinity, -Infinity], \"c\": \"NaN in text\"}";
        let fixed = strip_invalid_json_numbers(raw);
        let v: serde_json::Value = serde_json::from_str(&fixed).expect(&fixed);
        assert!(v["a"].is_null());
        assert!(v["b"][1].is_null());
        assert!(v["b"][2].is_null());
        assert_eq!(v["c"], "NaN in text");
    }

    #[test]
    fn test_nan_inside_identifiers_preserved() {
        // `NaN` inside a word (e.g. a name) must not be touched.
        let raw = "{\"gene\": \"NANOG\", \"x\": NaN}";
        let fixed = strip_invalid_json_numbers(raw);
        let v: serde_json::Value = serde_json::from_str(&fixed).expect(&fixed);
        assert_eq!(v["gene"], "NANOG");
        assert!(v["x"].is_null());
    }
}
