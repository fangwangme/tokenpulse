use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use std::fs;
use std::path::Path;

/// Resolves the newest cheap model an agent's CLI accepts, from the catalogs the
/// CLIs cache locally.
///
/// There is deliberately no fallback: a hardcoded default goes stale and would
/// silently ping a retired model, so a missing model is an error.
pub fn resolve_cheapest_model(agent: &str) -> Result<String> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("home directory not found"))?;
    match agent {
        "claude" => resolve_claude(&home.join(".claude").join("cache").join("model-catalog")),
        "codex" => resolve_codex(&home.join(".codex").join("models_cache.json")),
        "antigravity" => {
            resolve_antigravity(&crate::usage::antigravity::default_model_alias_history_path())
        }
        _ => Err(anyhow!("no model catalog for Keeper agent '{agent}'")),
    }
}

/// Newest Haiku in the `main` section of the most recently written catalog.
/// Every account offers the same models, so whichever account wrote last will do.
fn resolve_claude(dir: &Path) -> Result<String> {
    let catalog = fs::read_dir(dir)
        .with_context(|| format!("failed to read Claude model catalog '{}'", dir.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .max_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok())
        .ok_or_else(|| anyhow!("no catalog files in '{}'", dir.display()))?
        .path();
    let value = read_json(&catalog)?;
    let models = value
        .pointer("/catalog/config/models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no model list in '{}'", catalog.display()))?;

    newest_version(
        models
            .iter()
            .filter(|m| str_field(m, "section") == Some("main"))
            .filter_map(|m| str_field(m, "id"))
            .filter(|id| id.contains("haiku")),
    )
    .ok_or_else(|| anyhow!("no active Haiku model in '{}'", catalog.display()))
}

/// Newest Luna the Codex picker offers: hidden models and ones described as
/// older are skipped.
fn resolve_codex(file: &Path) -> Result<String> {
    let value = read_json(file)?;
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("no model list in '{}'", file.display()))?;

    newest_version(
        models
            .iter()
            .filter(|m| str_field(m, "visibility") != Some("hide"))
            .filter(|m| {
                !str_field(m, "description")
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .contains("older")
            })
            .filter_map(|m| str_field(m, "slug"))
            .filter(|slug| slug.contains("luna")),
    )
    .ok_or_else(|| anyhow!("no active Luna model in '{}'", file.display()))
}

/// Newest low-tier Flash from the alias history the Antigravity sync writes.
fn resolve_antigravity(file: &Path) -> Result<String> {
    let value = read_json(file)?;
    let aliases = value
        .get("aliases")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("no alias map in '{}'", file.display()))?;

    // `modelId` is TokenPulse's pricing id (`gemini-3-8-flash-low`); `agy` only
    // accepts the dotted form, which the label still carries.
    let ids: Vec<String> = aliases
        .values()
        .filter_map(|alias| str_field(alias, "label"))
        .filter_map(antigravity_cli_model_id)
        .filter(|id| id.contains("flash") && id.ends_with("-low"))
        .collect();

    newest_version(ids.iter().map(String::as_str))
        .ok_or_else(|| anyhow!("no Flash (Low) model in '{}'", file.display()))
}

/// `agy --model` id for an Antigravity label: `Gemini 3.8 Flash (Low)` →
/// `gemini-3.8-flash-low`, matching what `agy models` lists.
fn antigravity_cli_model_id(label: &str) -> Option<String> {
    let id = label
        .replace(['(', ')'], " ")
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join("-");
    (!id.is_empty()).then_some(id)
}

fn read_json(path: &Path) -> Result<Value> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse '{}'", path.display()))
}

fn str_field<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(Value::as_str)
}

/// The id with the highest version numbers: `gpt-6-luna` beats `gpt-5.6-luna`.
fn newest_version<'a>(ids: impl Iterator<Item = &'a str>) -> Option<String> {
    ids.max_by(|a, b| {
        version_numbers(a)
            .cmp(&version_numbers(b))
            .then_with(|| a.cmp(b))
    })
    .map(str::to_string)
}

/// Numeric runs in a model id: `gemini-3.8-flash-low` → `[3, 8]`.
fn version_numbers(id: &str) -> Vec<u32> {
    id.split(|c: char| !c.is_ascii_digit())
        .filter_map(|run| run.parse().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_version_numbers() {
        assert_eq!(version_numbers("gemini-3.8-flash-low"), vec![3, 8]);
        assert_eq!(version_numbers("gpt-6-luna"), vec![6]);
        assert_eq!(version_numbers("gpt-5.6-luna"), vec![5, 6]);
    }

    #[test]
    fn test_resolve_claude_newest_main_haiku() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("tok-test-ccd.json"),
            r#"{ "catalog": { "config": { "models": [
                { "id": "claude-opus-5", "section": "main" },
                { "id": "claude-haiku-5", "section": "main" },
                { "id": "claude-haiku-5-5", "section": "main" },
                { "id": "claude-haiku-6", "section": "overflow" }
            ] } } }"#,
        )
        .unwrap();

        assert_eq!(resolve_claude(temp.path()).unwrap(), "claude-haiku-5-5");
    }

    #[test]
    fn test_resolve_claude_without_main_haiku_fails() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("tok-test-ccd.json"),
            r#"{ "catalog": { "config": { "models": [
                { "id": "claude-opus-5", "section": "main" },
                { "id": "claude-haiku-4-5-20251001", "section": "overflow" }
            ] } } }"#,
        )
        .unwrap();

        let err = resolve_claude(temp.path()).unwrap_err();
        assert!(err.to_string().contains("no active Haiku model"));
    }

    #[test]
    fn test_resolve_codex_skips_hidden_and_older() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("models_cache.json");
        fs::write(
            &file,
            r#"{ "models": [
                { "slug": "gpt-6.1-luna-preview", "description": "Preview.", "visibility": "hide" },
                { "slug": "gpt-6-luna", "description": "Fast and affordable.", "visibility": "list" },
                { "slug": "gpt-5.6-luna", "description": "Older fast model.", "visibility": "list" }
            ] }"#,
        )
        .unwrap();

        assert_eq!(resolve_codex(&file).unwrap(), "gpt-6-luna");
    }

    #[test]
    fn test_resolve_codex_only_older_fails() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("models_cache.json");
        fs::write(
            &file,
            r#"{ "models": [ { "slug": "gpt-5.6-luna", "description": "Older fast model." } ] }"#,
        )
        .unwrap();

        let err = resolve_codex(&file).unwrap_err();
        assert!(err.to_string().contains("no active Luna model"));
    }

    #[test]
    fn test_resolve_codex_missing_file_fails() {
        let temp = tempdir().unwrap();
        assert!(resolve_codex(&temp.path().join("missing.json")).is_err());
    }

    #[test]
    fn test_resolve_antigravity_newest_flash_low_in_cli_form() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("model-aliases.json");
        fs::write(
            &file,
            r#"{ "version": 1, "aliases": {
                "m319": { "modelId": "gemini-3-8-flash-medium", "label": "Gemini 3.8 Flash (Medium)" },
                "m320": { "modelId": "gemini-3-8-flash-low", "label": "Gemini 3.8 Flash (Low)" },
                "m300": { "modelId": "gemini-3-7-flash-low", "label": "Gemini 3.7 Flash (Low)" }
            } }"#,
        )
        .unwrap();

        assert_eq!(resolve_antigravity(&file).unwrap(), "gemini-3.8-flash-low");
    }

    #[test]
    fn test_resolve_antigravity_without_flash_low_fails() {
        let temp = tempdir().unwrap();
        let file = temp.path().join("model-aliases.json");
        fs::write(
            &file,
            r#"{ "version": 1, "aliases": {
                "m7": { "modelId": "gemini-3-pro-low", "label": "Gemini 3 Pro (Low)" },
                "m18": { "modelId": "gemini-3-flash", "label": "Gemini 3 Flash" }
            } }"#,
        )
        .unwrap();

        let err = resolve_antigravity(&file).unwrap_err();
        assert!(err.to_string().contains("no Flash (Low) model"));
    }

    #[test]
    fn test_antigravity_cli_model_id_matches_agy_models() {
        assert_eq!(
            antigravity_cli_model_id("Gemini 3.8 Flash (Low)").as_deref(),
            Some("gemini-3.8-flash-low")
        );
        assert_eq!(antigravity_cli_model_id("  "), None);
    }
}
