use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Resolves the cheapest active model for an agent dynamically from local catalogs.
///
/// Returns an error if the model catalog cannot be read or no suitable model is found.
/// Zero tolerance: no fallbacks to hardcoded models are allowed.
pub fn resolve_cheapest_model(agent: &str) -> Result<String> {
    match agent {
        "claude" => {
            let dir = claude_catalog_dir()
                .ok_or_else(|| anyhow!("Failed to resolve Claude catalog directory"))?;
            resolve_claude_cheapest_model_from_dir(&dir)
        }
        "codex" => {
            let file = codex_cache_path()
                .ok_or_else(|| anyhow!("Failed to resolve Codex models cache path"))?;
            resolve_codex_cheapest_model_from_file(&file)
        }
        "antigravity" => {
            let file = antigravity_aliases_path()
                .ok_or_else(|| anyhow!("Failed to resolve Antigravity model aliases path"))?;
            resolve_antigravity_cheapest_model_from_file(&file)
        }
        _ => Err(anyhow!(
            "Unknown agent '{}' for dynamic model resolution",
            agent
        )),
    }
}

pub fn claude_catalog_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude").join("cache").join("model-catalog"))
}

pub fn codex_cache_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".codex").join("models_cache.json"))
}

pub fn antigravity_aliases_path() -> Option<PathBuf> {
    dirs::data_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
        .map(|d| {
            d.join("tokenpulse")
                .join("antigravity-cache")
                .join("model-aliases.json")
        })
}

/// Resolves the newest active Haiku model from Claude's model catalog directory.
pub fn resolve_claude_cheapest_model_from_dir(dir: &Path) -> Result<String> {
    if !dir.exists() || !dir.is_dir() {
        return Err(anyhow!(
            "Claude catalog directory not found at '{}'",
            dir.display()
        ));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("Failed to read '{}'", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            let mtime = entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            entries.push((mtime, path));
        }
    }

    // Sort by mtime descending (most recently modified first)
    entries.sort_by(|a, b| b.0.cmp(&a.0));

    if entries.is_empty() {
        return Err(anyhow!(
            "No JSON catalog files found in '{}'",
            dir.display()
        ));
    }

    for (_, path) in entries {
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let value: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Extract models array from either catalog.config.models or models directly
        let models_opt = value
            .pointer("/catalog/config/models")
            .and_then(|v| v.as_array())
            .or_else(|| value.get("models").and_then(|v| v.as_array()))
            .or_else(|| value.as_array());

        let Some(models) = models_opt else {
            continue;
        };

        let mut main_candidates = Vec::new();
        let mut fallback_candidates = Vec::new();

        for m in models {
            let id = m.get("id").and_then(|v| v.as_str());
            let short_name = m.get("short_name").and_then(|v| v.as_str());
            let name = m.get("name").and_then(|v| v.as_str());
            let section = m.get("section").and_then(|v| v.as_str()).unwrap_or("");

            let is_haiku = short_name.is_some_and(|s| s.eq_ignore_ascii_case("haiku"))
                || name.is_some_and(|n| n.to_ascii_lowercase().contains("haiku"))
                || id.is_some_and(|i| i.to_ascii_lowercase().contains("haiku"));

            if is_haiku {
                if let Some(id_str) = id {
                    if section.eq_ignore_ascii_case("main") {
                        main_candidates.push(id_str.to_string());
                    } else {
                        fallback_candidates.push(id_str.to_string());
                    }
                }
            }
        }

        if let Some(first_main) = main_candidates.into_iter().next() {
            return Ok(first_main);
        }
        if let Some(first_fallback) = fallback_candidates.into_iter().next() {
            return Ok(first_fallback);
        }
    }

    Err(anyhow!(
        "No active Haiku model found in Claude model catalog at '{}'",
        dir.display()
    ))
}

/// Resolves the newest active Luna model from Codex's models cache JSON file.
pub fn resolve_codex_cheapest_model_from_file(file: &Path) -> Result<String> {
    if !file.exists() || !file.is_file() {
        return Err(anyhow!(
            "Codex models cache not found at '{}'",
            file.display()
        ));
    }

    let content = fs::read_to_string(file)
        .with_context(|| format!("Failed to read Codex models cache at '{}'", file.display()))?;
    let value: serde_json::Value = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse Codex models cache at '{}'", file.display()))?;

    let models = value
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            anyhow!(
                "Invalid models list in Codex models cache at '{}'",
                file.display()
            )
        })?;

    let mut active_lunas = Vec::new();

    for m in models {
        let slug = m.get("slug").and_then(|v| v.as_str());
        let display_name = m.get("display_name").and_then(|v| v.as_str()).unwrap_or("");
        let description = m.get("description").and_then(|v| v.as_str()).unwrap_or("");

        let Some(slug_str) = slug else {
            continue;
        };

        if slug_str.to_ascii_lowercase().contains("luna")
            || display_name.to_ascii_lowercase().contains("luna")
        {
            let is_older = description.to_ascii_lowercase().contains("older")
                || display_name.to_ascii_lowercase().contains("older");

            if !is_older {
                active_lunas.push(slug_str.to_string());
            }
        }
    }

    sort_version_strings_descending(&mut active_lunas);
    if let Some(best) = active_lunas.into_iter().next() {
        return Ok(best);
    }

    Err(anyhow!(
        "No active Luna model found in Codex models cache at '{}'",
        file.display()
    ))
}

/// Resolves the newest active Flash (preferring -low tier) model from Antigravity's model aliases JSON file.
pub fn resolve_antigravity_cheapest_model_from_file(file: &Path) -> Result<String> {
    if !file.exists() || !file.is_file() {
        return Err(anyhow!(
            "Antigravity model aliases not found at '{}'",
            file.display()
        ));
    }

    let content = fs::read_to_string(file).with_context(|| {
        format!(
            "Failed to read Antigravity model aliases at '{}'",
            file.display()
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&content).with_context(|| {
        format!(
            "Failed to parse Antigravity model aliases at '{}'",
            file.display()
        )
    })?;

    let aliases = value
        .get("aliases")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            anyhow!(
                "Invalid aliases map in Antigravity model aliases at '{}'",
                file.display()
            )
        })?;

    let mut low_flash_models = Vec::new();
    let mut other_flash_models = Vec::new();

    for (_, item) in aliases {
        let model_id = item.get("modelId").and_then(|v| v.as_str());
        let label = item.get("label").and_then(|v| v.as_str()).unwrap_or("");

        let Some(mid) = model_id else {
            continue;
        };

        let is_flash = mid.to_ascii_lowercase().contains("flash")
            || label.to_ascii_lowercase().contains("flash");

        if is_flash {
            let is_low_tier = mid.to_ascii_lowercase().ends_with("-low")
                || label.to_ascii_lowercase().contains("(low)");

            if is_low_tier {
                low_flash_models.push(mid.to_string());
            } else {
                other_flash_models.push(mid.to_string());
            }
        }
    }

    // Prefer low-tier flash models, sorted descending by version
    sort_version_strings_descending(&mut low_flash_models);
    if let Some(best) = low_flash_models.into_iter().next() {
        return Ok(best);
    }

    sort_version_strings_descending(&mut other_flash_models);
    if let Some(best) = other_flash_models.into_iter().next() {
        return Ok(best);
    }

    Err(anyhow!(
        "No active Flash model found in Antigravity model aliases at '{}'",
        file.display()
    ))
}

/// Helper function to sort model strings containing version numbers descending.
/// E.g. "gemini-3.8-flash-low" comes before "gemini-3.7-flash-low",
/// and "gpt-6-luna" comes before "gpt-5.6-luna".
fn sort_version_strings_descending(models: &mut [String]) {
    models.sort_by(|a, b| {
        let ver_a = extract_version_tuple(a);
        let ver_b = extract_version_tuple(b);
        ver_b.cmp(&ver_a).then_with(|| b.cmp(a))
    });
}

/// Extracts a sequence of numeric components from a model ID string for semantic-style comparison.
/// E.g. "gemini-3.8-flash-low" -> vec![3, 8], "gpt-6-luna" -> vec![6], "gpt-5.6-luna" -> vec![5, 6].
fn extract_version_tuple(s: &str) -> Vec<u32> {
    let mut nums = Vec::new();
    let mut current_num = String::new();

    for ch in s.chars() {
        if ch.is_ascii_digit() {
            current_num.push(ch);
        } else if !current_num.is_empty() {
            if let Ok(n) = current_num.parse::<u32>() {
                nums.push(n);
            }
            current_num.clear();
        }
    }
    if !current_num.is_empty() {
        if let Ok(n) = current_num.parse::<u32>() {
            nums.push(n);
        }
    }
    nums
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_extract_version_tuple() {
        assert_eq!(extract_version_tuple("gemini-3.8-flash-low"), vec![3, 8]);
        assert_eq!(extract_version_tuple("gemini-3.7-flash-low"), vec![3, 7]);
        assert_eq!(extract_version_tuple("gpt-6-luna"), vec![6]);
        assert_eq!(extract_version_tuple("gpt-5.6-luna"), vec![5, 6]);
    }

    #[test]
    fn test_resolve_claude_cheapest_model() {
        let temp = tempdir().unwrap();
        let cat_file = temp.path().join("tok-test-ccd.json");

        let json_content = r#"{
            "catalog": {
                "config": {
                    "models": [
                        { "id": "claude-opus-5", "name": "Opus 5", "short_name": "Opus", "section": "main" },
                        { "id": "claude-haiku-5-5", "name": "Haiku 5.5", "short_name": "Haiku", "section": "main" },
                        { "id": "claude-haiku-4-5-20251001", "name": "Haiku 4.5", "short_name": "Haiku", "section": "overflow" }
                    ]
                }
            }
        }"#;
        fs::write(&cat_file, json_content).unwrap();

        let resolved = resolve_claude_cheapest_model_from_dir(temp.path()).unwrap();
        assert_eq!(resolved, "claude-haiku-5-5");
    }

    #[test]
    fn test_resolve_claude_cheapest_model_no_haiku() {
        let temp = tempdir().unwrap();
        let cat_file = temp.path().join("tok-test-ccd.json");
        let json_content = r#"{
            "catalog": {
                "config": {
                    "models": [
                        { "id": "claude-opus-5", "name": "Opus 5", "short_name": "Opus", "section": "main" }
                    ]
                }
            }
        }"#;
        fs::write(&cat_file, json_content).unwrap();

        let err = resolve_claude_cheapest_model_from_dir(temp.path()).unwrap_err();
        assert!(err.to_string().contains("No active Haiku model found"));
    }

    #[test]
    fn test_resolve_codex_cheapest_model() {
        let temp = tempdir().unwrap();
        let cache_file = temp.path().join("models_cache.json");

        let json_content = r#"{
            "models": [
                {
                    "slug": "gpt-6-luna",
                    "display_name": "GPT-6-Luna",
                    "description": "Fast and affordable model for easier tasks."
                },
                {
                    "slug": "gpt-5.6-luna",
                    "display_name": "GPT-5.6-Luna",
                    "description": "Older fast and efficient model."
                }
            ]
        }"#;
        fs::write(&cache_file, json_content).unwrap();

        let resolved = resolve_codex_cheapest_model_from_file(&cache_file).unwrap();
        assert_eq!(resolved, "gpt-6-luna");
    }

    #[test]
    fn test_resolve_codex_cheapest_model_missing_file() {
        let temp = tempdir().unwrap();
        let cache_file = temp.path().join("missing.json");
        let err = resolve_codex_cheapest_model_from_file(&cache_file).unwrap_err();
        assert!(err.to_string().contains("Codex models cache not found"));
    }

    #[test]
    fn test_resolve_codex_cheapest_model_only_older_fails() {
        let temp = tempdir().unwrap();
        let cache_file = temp.path().join("models_cache.json");
        let json_content = r#"{
            "models": [
                {
                    "slug": "gpt-5.6-luna",
                    "display_name": "GPT-5.6-Luna",
                    "description": "Older fast and efficient model."
                }
            ]
        }"#;
        fs::write(&cache_file, json_content).unwrap();
        let err = resolve_codex_cheapest_model_from_file(&cache_file).unwrap_err();
        assert!(err.to_string().contains("No active Luna model found"));
    }

    #[test]
    fn test_resolve_antigravity_cheapest_model() {
        let temp = tempdir().unwrap();
        let aliases_file = temp.path().join("model-aliases.json");

        let json_content = r#"{
            "version": 1,
            "aliases": {
                "m319": {
                    "modelId": "gemini-3-8-flash-medium",
                    "label": "Gemini 3.8 Flash (Medium)"
                },
                "m320": {
                    "modelId": "gemini-3-8-flash-low",
                    "label": "Gemini 3.8 Flash (Low)"
                },
                "m300": {
                    "modelId": "gemini-3-7-flash-low",
                    "label": "Gemini 3.7 Flash (Low)"
                }
            }
        }"#;
        fs::write(&aliases_file, json_content).unwrap();

        let resolved = resolve_antigravity_cheapest_model_from_file(&aliases_file).unwrap();
        assert_eq!(resolved, "gemini-3-8-flash-low");
    }

    #[test]
    fn test_resolve_antigravity_cheapest_model_no_flash() {
        let temp = tempdir().unwrap();
        let aliases_file = temp.path().join("model-aliases.json");

        let json_content = r#"{
            "version": 1,
            "aliases": {
                "m7": {
                    "modelId": "gemini-3-pro-low",
                    "label": "Gemini 3 Pro (Low)"
                }
            }
        }"#;
        fs::write(&aliases_file, json_content).unwrap();

        let err = resolve_antigravity_cheapest_model_from_file(&aliases_file).unwrap_err();
        assert!(err.to_string().contains("No active Flash model found"));
    }
}
