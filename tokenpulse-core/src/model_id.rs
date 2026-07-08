pub(crate) fn strip_date_suffix(model_id: &str) -> Option<String> {
    if model_id.len() > 9 {
        let suffix = &model_id[model_id.len() - 8..];
        if suffix.chars().all(|ch| ch.is_ascii_digit())
            && model_id.as_bytes()[model_id.len() - 9] == b'-'
        {
            return Some(model_id[..model_id.len() - 9].to_string());
        }
    }

    if model_id.len() > 11 {
        let suffix = &model_id[model_id.len() - 10..];
        let bytes = suffix.as_bytes();
        let is_dash_date = bytes[4] == b'-'
            && bytes[7] == b'-'
            && suffix
                .chars()
                .enumerate()
                .all(|(idx, ch)| idx == 4 || idx == 7 || ch.is_ascii_digit());
        if is_dash_date && model_id.as_bytes()[model_id.len() - 11] == b'-' {
            return Some(model_id[..model_id.len() - 11].to_string());
        }
    }

    None
}

pub(crate) fn canonical(model_id: &str) -> String {
    let mut normalized = model_id.trim().to_ascii_lowercase();

    if let Some(stripped) = strip_date_suffix(&normalized) {
        normalized = stripped;
    }

    if let Some(last_segment) = normalized.rsplit('/').next() {
        normalized = last_segment.to_string();
    }

    normalized = normalized.replace(['.', '_', ' '], "-");
    normalized = collapse_repeated_hyphens(&normalized);

    // strip prefixes
    for prefix in ["antigravity-", "anti-gravity-"] {
        if let Some(stripped) = normalized.strip_prefix(prefix) {
            normalized = stripped.to_string();
            break;
        }
    }

    // strip suffixes repeatedly
    loop {
        let mut stripped = false;
        for suffix in ["-high", "-medium", "-low", "-free", "-thinking"] {
            if let Some(s) = normalized.strip_suffix(suffix) {
                normalized = s.to_string();
                stripped = true;
                break;
            }
        }
        if !stripped {
            break;
        }
    }

    if let Some(stripped) = normalized.strip_suffix("-0") {
        normalized = stripped.to_string();
    }

    normalized = normalized.trim_matches('-').to_string();

    match normalized.as_str() {
        "gemini-3-pro" | "gemini-3-flash" => {
            normalized.push_str("-preview");
        }
        _ => {}
    }

    normalized
}

pub(crate) fn is_pseudo(model_id: &str) -> bool {
    let id = model_id.trim().to_ascii_lowercase();
    id.is_empty()
        || id == "unknown"
        || id.starts_with("auto-")
        || id.ends_with("-auto-review")
        || id.ends_with("-default")
}

fn collapse_repeated_hyphens(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_was_hyphen = false;

    for ch in value.chars() {
        if ch == '-' {
            if !last_was_hyphen {
                out.push(ch);
            }
            last_was_hyphen = true;
        } else {
            out.push(ch);
            last_was_hyphen = false;
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_canonical() {
        assert_eq!(canonical("gemini-3-flash-high"), "gemini-3-flash-preview");
        assert_eq!(canonical("claude-3-opus-thinking"), "claude-3-opus");
        assert_eq!(canonical("openai/gpt-4.1-mini-2025-04-14"), "gpt-4-1-mini");
        assert_eq!(
            canonical("antigravity-claude-opus-4-5-thinking-high-free"),
            "claude-opus-4-5"
        );
    }
}
