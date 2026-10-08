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

/// Display and grouping id: one name per model across every source.
///
/// The `preview` segment is dropped wherever it sits, along with the tier and
/// routing suffixes. Vendors attach it arbitrarily and a GA release is a new
/// version rather than the same model promoted, so keeping it would only split
/// one model's usage in two.
pub(crate) fn canonical(model_id: &str) -> String {
    let without_preview = pricing_key(model_id)
        .split('-')
        .filter(|segment| *segment != "preview")
        .collect::<Vec<_>>()
        .join("-");
    strip_variant_suffixes(&without_preview)
}

/// Pricing-catalog key: the same spelling rules as [`canonical`], but
/// `-preview` is kept. Catalogs list `X` and `X-preview` as separate entries,
/// sometimes at different prices, so merging them would let one overwrite the
/// other.
pub(crate) fn pricing_key(model_id: &str) -> String {
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

    strip_variant_suffixes(&normalized)
}

/// Strips tier, thinking and routing suffixes repeatedly. `-tiered` is an
/// Antigravity routing suffix, not a model family, so it must not split usage
/// or pricing away from the base model.
fn strip_variant_suffixes(model_id: &str) -> String {
    let mut normalized = model_id.to_string();
    loop {
        let mut stripped = false;
        for suffix in ["-high", "-medium", "-low", "-free", "-thinking", "-tiered"] {
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

    normalized.trim_matches('-').to_string()
}

/// Pseudo-model ids reported by agent logs that do not correspond to a real,
/// purchasable model: routing aliases (`auto-gemini-3`, `gemini-default`),
/// internal features (`codex-auto-review`), and parser fallbacks (`unknown`).
/// They can never resolve against the pricing catalog, so they must not
/// trigger on-demand pricing refreshes or keep cost repairs pending forever.
pub(crate) fn is_pseudo(model_id: &str) -> bool {
    let id = model_id.trim().to_ascii_lowercase();
    id.is_empty()
        || id == "unknown"
        || id.starts_with("auto-")
        || id.ends_with("-auto-review")
        || id.ends_with("-default")
}

pub(crate) fn collapse_repeated_hyphens(value: &str) -> String {
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
        assert_eq!(canonical("gemini-3-flash-high"), "gemini-3-flash");
        assert_eq!(canonical("claude-3-opus-thinking"), "claude-3-opus");
        assert_eq!(canonical("openai/gpt-4.1-mini-2025-04-14"), "gpt-4-1-mini");
        assert_eq!(
            canonical("antigravity-claude-opus-4-5-thinking-high-free"),
            "claude-opus-4-5"
        );
    }

    #[test]
    fn test_canonical_strips_trailing_tiered_routing_suffix() {
        assert_eq!(canonical("gemini-3.6-flash-tiered"), "gemini-3-6-flash");
        assert_eq!(canonical("gemini-3-6-flash-tiered"), "gemini-3-6-flash");

        // `tiered` is only a routing suffix at the end of the id.
        assert_eq!(canonical("gemini-tiered-flash"), "gemini-tiered-flash");
        assert_eq!(canonical("tiered-model-x"), "tiered-model-x");
    }

    #[test]
    fn test_is_pseudo() {
        for id in [
            "",
            "unknown",
            "Unknown",
            "auto-gemini-3",
            "codex-auto-review",
            "gemini-default",
        ] {
            assert!(is_pseudo(id), "{id:?} should be pseudo");
        }
        for id in ["gpt-5.4", "claude-opus-4-6", "moonshotai/kimi-k2.5"] {
            assert!(!is_pseudo(id), "{id:?} should not be pseudo");
        }
    }

    #[test]
    fn test_canonical_drops_preview_wherever_it_sits() {
        assert_eq!(canonical("gemini-3.1-pro-preview-high"), "gemini-3-1-pro");
        assert_eq!(canonical("gemini-3-pro-preview"), "gemini-3-pro");
        assert_eq!(
            canonical("gemini-3-pro-preview-image"),
            "gemini-3-pro-image"
        );
        assert_eq!(canonical("google/gemini-3-flash-preview"), "gemini-3-flash");
        // Nothing is appended to ids that never carried it.
        assert_eq!(canonical("gemini-3-pro"), "gemini-3-pro");
        assert_eq!(canonical("gemini-3-flash"), "gemini-3-flash");
    }

    #[test]
    fn test_pricing_key_keeps_preview_but_shares_spelling_rules() {
        assert_eq!(
            pricing_key("gemini-3.1-pro-preview-high"),
            "gemini-3-1-pro-preview"
        );
        assert_eq!(pricing_key("hy3-preview"), "hy3-preview");
        assert_eq!(pricing_key("hy3"), "hy3");
        for spelling in [
            "gemini-3.1-pro-high",
            "gemini_3_1_pro_high",
            "gemini-3-1-pro-high",
            "Gemini 3.1 Pro High",
        ] {
            assert_eq!(pricing_key(spelling), "gemini-3-1-pro", "{spelling}");
        }
    }
}
