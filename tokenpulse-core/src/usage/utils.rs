pub(crate) fn parse_timestamp_str(value: &str) -> Option<i64> {
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(value) {
        return Some(dt.timestamp_millis());
    }

    if let Ok(numeric) = value.parse::<i64>() {
        if numeric >= 1_000_000_000_000 {
            return Some(numeric);
        }
        return Some(numeric * 1000);
    }

    None
}

pub fn normalize_model_name(model_id: &str) -> String {
    let mut normalized = model_id.trim().to_ascii_lowercase();

    if let Some(stripped) = normalized.strip_suffix("-free") {
        normalized = stripped.to_string();
    }

    if let Some(stripped) = strip_date_suffix(&normalized) {
        normalized = stripped;
    }

    if let Some(last_segment) = normalized.rsplit('/').next() {
        normalized = last_segment.to_string();
    }

    normalized = normalized.replace(['.', '_', ' '], "-");
    normalized = collapse_repeated_hyphens(&normalized);

    if let Some(stripped) = normalized.strip_suffix("-0") {
        normalized = stripped.to_string();
    }

    normalized.trim_matches('-').to_string()
}

pub fn detect_provider_from_model(model: &str) -> String {
    let normalized = normalize_model_name(model);

    if normalized.starts_with("codex")
        || normalized.starts_with("gpt")
        || normalized.starts_with("o1")
        || normalized.starts_with("o3")
        || normalized.starts_with("o4")
    {
        "openai".to_string()
    } else if normalized.starts_with("antigravity-claude")
        || normalized.starts_with("anti-gravity-claude")
        || normalized.starts_with("claude")
        || normalized.starts_with("sonnet")
        || normalized.starts_with("opus")
        || normalized.starts_with("haiku")
    {
        "anthropic".to_string()
    } else if normalized.starts_with("antigravity-gemini")
        || normalized.starts_with("anti-gravity-gemini")
        || normalized.starts_with("gemini")
    {
        "google".to_string()
    } else if normalized.contains("nvidia")
        || normalized.contains("nemotron")
        || normalized.contains("deepseek")
        || normalized.contains("glm")
        || normalized.contains("minimax")
    {
        "other".to_string()
    } else {
        "other".to_string()
    }
}

fn strip_date_suffix(model_id: &str) -> Option<String> {
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
    use super::{detect_provider_from_model, normalize_model_name};

    #[test]
    fn normalize_model_name_strips_dates_free_and_dots() {
        assert_eq!(
            normalize_model_name("openai/gpt-4.1-mini-2025-04-14"),
            "gpt-4-1-mini"
        );
        assert_eq!(
            normalize_model_name("moonshotai/kimi-k2.5-free"),
            "kimi-k2-5"
        );
        assert_eq!(normalize_model_name("claude-opus-4.6"), "claude-opus-4-6");
    }

    #[test]
    fn detect_provider_from_model_maps_known_families() {
        assert_eq!(detect_provider_from_model("codex-mini-latest"), "openai");
        assert_eq!(detect_provider_from_model("gpt-4.1"), "openai");
        assert_eq!(detect_provider_from_model("claude-sonnet-4"), "anthropic");
        assert_eq!(
            detect_provider_from_model("anti-gravity-claude-opus-4.1"),
            "anthropic"
        );
        assert_eq!(detect_provider_from_model("gemini-2.5-pro"), "google");
        assert_eq!(
            detect_provider_from_model("antigravity-gemini-3-pro-high"),
            "google"
        );
        assert_eq!(detect_provider_from_model("deepseek-r1"), "other");
        assert_eq!(detect_provider_from_model("glm-4.7"), "other");
        assert_eq!(detect_provider_from_model("some-random-model"), "other");
    }
}
