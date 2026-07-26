//! Shared parsing of model output into card names / counts.
//!
//! Every backend produces plain-text output of the same shape (one card name
//! per line for pass 1; `COUNT CARD_NAME` lines for pass 2), so parsing is
//! centralised here and unit-tested against fixed CLI-style output.

/// Parse pass-1 output: one card name per non-empty line.
pub fn card_names(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// Parse pass-2 output: `COUNT CARD_NAME` lines into `(name, count)` pairs.
///
/// Lines that do not start with an integer count are ignored, so incidental
/// commentary a CLI might emit is skipped rather than mis-parsed.
pub fn counts(text: &str) -> Vec<(String, u8)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((count_str, card_name)) = line.split_once(' ') {
            if let Ok(count) = count_str.trim().parse::<u8>() {
                let card_name = card_name.trim().to_string();
                if !card_name.is_empty() {
                    out.push((card_name, count));
                }
            }
        }
    }
    out
}

/// Extract the assistant text from a `claude -p --output-format json` blob.
///
/// The CLI emits a single JSON object with a top-level `"result"` string. If
/// the payload is not valid JSON (or lacks `result`), the raw text is returned
/// unchanged so a plain-text fallback still parses.
pub fn claude_json_result(raw: &str) -> String {
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) {
        if let Some(serde_json::Value::String(s)) = map.get("result") {
            return s.clone();
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_card_names_trimming_and_filtering_blanks() {
        let out = "Lightning Bolt\n  Counterspell  \n\n Birds of Paradise\n";
        assert_eq!(
            card_names(out),
            vec![
                "Lightning Bolt".to_string(),
                "Counterspell".to_string(),
                "Birds of Paradise".to_string(),
            ]
        );
    }

    #[test]
    fn parses_counts_and_ignores_non_count_lines() {
        let out =
            "Here are the counts:\n2 Lightning Bolt\n1 Mountain\n\nbogus line\n3 Birds of Paradise";
        assert_eq!(
            counts(out),
            vec![
                ("Lightning Bolt".to_string(), 2),
                ("Mountain".to_string(), 1),
                ("Birds of Paradise".to_string(), 3),
            ]
        );
    }

    #[test]
    fn multiword_card_names_survive_count_parse() {
        assert_eq!(
            counts("4 Birds of Paradise"),
            vec![("Birds of Paradise".to_string(), 4)]
        );
    }

    #[test]
    fn extracts_result_field_from_claude_json() {
        let raw = r#"{"type":"result","is_error":false,"result":"Lightning Bolt\nCounterspell","session_id":"x"}"#;
        assert_eq!(
            claude_json_result(raw),
            "Lightning Bolt\nCounterspell".to_string()
        );
        // And the full parse pipeline turns it into names:
        assert_eq!(
            card_names(&claude_json_result(raw)),
            vec!["Lightning Bolt".to_string(), "Counterspell".to_string()]
        );
    }

    #[test]
    fn claude_json_falls_back_to_raw_when_not_json() {
        let raw = "Lightning Bolt\nCounterspell";
        assert_eq!(claude_json_result(raw), raw.to_string());
    }
}
