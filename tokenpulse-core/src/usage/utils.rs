use crate::model_id::strip_date_suffix;

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
    normalized = strip_known_prefix(&normalized);
    normalized = strip_tier_suffix(&normalized);
    normalized = strip_reasoning_suffix(&normalized);

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

fn strip_tier_suffix(model_id: &str) -> String {
    for suffix in ["-high", "-medium", "-low"] {
        if let Some(stripped) = model_id.strip_suffix(suffix) {
            return stripped.to_string();
        }
    }
    model_id.to_string()
}

fn strip_known_prefix(model_id: &str) -> String {
    for prefix in ["antigravity-", "anti-gravity-"] {
        if let Some(stripped) = model_id.strip_prefix(prefix) {
            return stripped.to_string();
        }
    }
    model_id.to_string()
}

fn strip_reasoning_suffix(model_id: &str) -> String {
    model_id
        .strip_suffix("-thinking")
        .unwrap_or(model_id)
        .to_string()
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
        assert_eq!(
            normalize_model_name("antigravity-claude-opus-4-5-thinking-high"),
            "claude-opus-4-5"
        );
        assert_eq!(
            normalize_model_name("anti-gravity-claude-opus-4-6-thinking"),
            "claude-opus-4-6"
        );
        assert_eq!(normalize_model_name("z-ai/glm-5.1-low"), "glm-5-1");
        assert_eq!(normalize_model_name("gemini-3-pro-medium"), "gemini-3-pro-preview");
        assert_eq!(normalize_model_name("gemini-3-flash"), "gemini-3-flash-preview");
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
