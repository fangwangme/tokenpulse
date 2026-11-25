package usage

import (
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
)

// ModelAlias is one enum → label mapping learned from Antigravity.
type ModelAlias struct {
	RawModelID string `json:"rawModelId"`
	ModelID    string `json:"modelId"`
	Label      string `json:"label"`
	Source     string `json:"source"`
}

type modelAliasHistoryEntry struct {
	RawModelID  string `json:"rawModelId"`
	ModelID     string `json:"modelId"`
	Label       string `json:"label"`
	Source      string `json:"source"`
	FirstSeenAt string `json:"firstSeenAt"`
	LastSeenAt  string `json:"lastSeenAt"`
}

type modelAliasHistory struct {
	Version   int                               `json:"version"`
	UpdatedAt string                            `json:"updatedAt"`
	Aliases   map[string]modelAliasHistoryEntry `json:"aliases"`
}

type staticModelAlias struct {
	raw, label, source string
}

// Static seeds from docs/specs/antigravity-model-aliases.md. The label decides
// the id, exactly as for learned aliases.
var staticModelAliases = []staticModelAlias{
	{"MODEL_PLACEHOLDER_M37", "Gemini 3.1 Pro (High)", "user-initial-mapping;tokscale"},
	{"MODEL_PLACEHOLDER_M36", "Gemini 3.1 Pro (Low)", "user-initial-mapping;tokscale"},
	{"MODEL_PLACEHOLDER_M18", "Gemini 3 Flash", "user-initial-mapping"},
	{"MODEL_PLACEHOLDER_M8", "Gemini 3 Pro (High)", "user-initial-mapping"},
	{"MODEL_PLACEHOLDER_M7", "Gemini 3 Pro (Low)", "user-initial-mapping"},
	{"MODEL_PLACEHOLDER_M9", "Gemini 3 Pro (Image)", "user-initial-mapping"},
	{"MODEL_PLACEHOLDER_M26", "Claude Opus 4.6 (Thinking)", "user-initial-mapping;openusage"},
	{"MODEL_PLACEHOLDER_M35", "Claude Sonnet 4.6 (Thinking)", "user-initial-mapping;antigravity-mobility-cli"},
	{"MODEL_PLACEHOLDER_M12", "Claude Opus 4.5 (Thinking)", "user-initial-mapping"},
	{"MODEL_OPENAI_GPT_OSS_120B_MEDIUM", "GPT-OSS 120B (Medium)", "user-initial-mapping;tokscale"},
	{"MODEL_CLAUDE_4_5_SONNET", "Claude Sonnet 4.5", "user-initial-mapping"},
	{"MODEL_PLACEHOLDER_M47", "Gemini 3 Flash", "tokscale;antigravity-mobility-cli"},
	{"MODEL_PLACEHOLDER_M16", "Gemini 3.1 Pro (High)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M132", "Gemini 3.5 Flash (High)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M20", "Gemini 3.5 Flash (Medium)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M187", "Gemini 3.5 Flash (Low)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M196", "Gemini 3.6 Flash", "observed-gen-metadata"},
	{"MODEL_PLACEHOLDER_M264", "Gemini 3.6 Flash", "observed-gen-metadata"},
	{"MODEL_PLACEHOLDER_M71", "Gemini 3.6 Flash (High)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M72", "Gemini 3.6 Flash (Medium)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M73", "Gemini 3.6 Flash (Low)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M298", "Gemini 3.7 Flash (High)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M299", "Gemini 3.7 Flash (Medium)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M300", "Gemini 3.7 Flash (Low)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M318", "Gemini 3.8 Flash (High)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M319", "Gemini 3.8 Flash (Medium)", "captured-antigravity-get-user-status"},
	{"MODEL_PLACEHOLDER_M320", "Gemini 3.8 Flash (Low)", "captured-antigravity-get-user-status"},
}

func seedAliases() map[string]ModelAlias {
	aliases := map[string]ModelAlias{}
	for _, seed := range staticModelAliases {
		id, ok := labelToModelID(seed.label)
		if !ok {
			continue
		}
		aliases[normalizeAliasKey(seed.raw)] = ModelAlias{
			RawModelID: seed.raw, ModelID: id, Label: seed.label, Source: seed.source,
		}
	}
	return aliases
}

func aliasHistoryPath(cacheRoot string) string {
	if filepath.Base(cacheRoot) == "sessions" {
		return filepath.Join(filepath.Dir(cacheRoot), "model-aliases.json")
	}
	return filepath.Join(cacheRoot, "model-aliases.json")
}

func loadAliasHistory(cacheRoot string) map[string]ModelAlias {
	aliases := seedAliases()
	content, err := os.ReadFile(aliasHistoryPath(cacheRoot))
	if err != nil {
		return aliases
	}
	var history modelAliasHistory
	if err := json.Unmarshal(content, &history); err != nil {
		return aliases
	}
	for key, entry := range history.Aliases {
		if !strings.HasPrefix(strings.ToLower(key), "model") ||
			!strings.HasPrefix(strings.ToLower(entry.RawModelID), "model") {
			continue
		}
		aliases[key] = aliasFromHistory(entry)
	}
	return aliases
}

func aliasFromHistory(entry modelAliasHistoryEntry) ModelAlias {
	id := entry.ModelID
	if labelID, ok := labelToModelID(entry.Label); ok {
		id = labelID
	}
	return ModelAlias{RawModelID: entry.RawModelID, ModelID: id, Label: entry.Label, Source: entry.Source}
}

// mergeAndSaveAliasHistory merges static, stored and freshly learned aliases
// and writes the merged table, keeping firstSeenAt and updating lastSeenAt.
func mergeAndSaveAliasHistory(cacheRoot string, dynamic map[string]ModelAlias) map[string]ModelAlias {
	path := aliasHistoryPath(cacheRoot)
	if len(dynamic) == 0 {
		if _, err := os.Stat(path); err == nil {
			return loadAliasHistory(cacheRoot)
		}
	}
	now := time.Now().UTC().Format(time.RFC3339Nano)
	history := modelAliasHistory{Version: 1, UpdatedAt: now, Aliases: map[string]modelAliasHistoryEntry{}}
	if content, err := os.ReadFile(path); err == nil {
		var loaded modelAliasHistory
		if err := json.Unmarshal(content, &loaded); err == nil {
			history = loaded
		}
	}
	if history.Aliases == nil {
		history.Aliases = map[string]modelAliasHistoryEntry{}
	}
	for key, entry := range history.Aliases {
		if !strings.HasPrefix(strings.ToLower(key), "model") ||
			!strings.HasPrefix(strings.ToLower(entry.RawModelID), "model") {
			delete(history.Aliases, key)
		}
	}
	merged := seedAliases()
	stored := map[string]ModelAlias{}
	for key, entry := range history.Aliases {
		stored[key] = aliasFromHistory(entry)
	}
	for key, alias := range stored {
		merged[key] = alias
	}
	for key, alias := range dynamic {
		merged[key] = alias
	}
	history.Version = 1
	history.UpdatedAt = now
	for key, alias := range merged {
		if !strings.HasPrefix(strings.ToLower(alias.RawModelID), "model") {
			continue
		}
		entry, ok := history.Aliases[key]
		if !ok {
			entry = modelAliasHistoryEntry{
				RawModelID: alias.RawModelID, ModelID: alias.ModelID, Label: alias.Label,
				Source: alias.Source, FirstSeenAt: now, LastSeenAt: now,
			}
		}
		entry.RawModelID = alias.RawModelID
		entry.ModelID = alias.ModelID
		entry.Label = alias.Label
		entry.Source = alias.Source
		entry.LastSeenAt = now
		history.Aliases[key] = entry
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err == nil {
		if data, err := json.MarshalIndent(history, "", "  "); err == nil {
			_ = os.WriteFile(path, append(data, '\n'), 0o644)
		}
	}
	return merged
}

// labelToModelID turns a display label into the id its spelling names.
func labelToModelID(label string) (string, bool) {
	replaced := strings.NewReplacer("(", " ", ")", " ").Replace(label)
	id := formatModelID(replaced)
	if id == "" || modelid.IsPseudo(id) {
		return "", false
	}
	return id, true
}

// formatModelID applies spelling only: lowercase, no antigravity prefix, dots,
// underscores and spaces as single hyphens.
func formatModelID(modelID string) string {
	normalized := strings.ToLower(strings.TrimSpace(modelID))
	normalized = strings.NewReplacer(".", "-", "_", "-", " ", "-").Replace(normalized)
	for _, prefix := range []string{"antigravity-", "anti-gravity-"} {
		if strings.HasPrefix(normalized, prefix) {
			normalized = strings.TrimPrefix(normalized, prefix)
			break
		}
	}
	return strings.Trim(modelid.CollapseRepeatedHyphens(normalized), "-")
}

func normalizeAliasKey(modelID string) string { return strings.ToLower(strings.TrimSpace(modelID)) }

func aliasKeyCandidates(modelID string) []string {
	key := normalizeAliasKey(modelID)
	legacy := strings.ReplaceAll(key, "-", "_")
	if legacy == key {
		return []string{key}
	}
	return []string{key, legacy}
}

func findAlias(rawModelID string, aliases map[string]ModelAlias) (ModelAlias, bool) {
	for _, key := range aliasKeyCandidates(rawModelID) {
		if alias, ok := aliases[key]; ok {
			return alias, true
		}
	}
	return ModelAlias{}, false
}

// ReleaseMap maps catalog model id → release date (YYYY-MM-DD).
type ReleaseMap map[string]string

// ResolveAntigravityModel resolves one generation's model id.
//
// A versioned alias wins; an unversioned family resolves by stepDate; the
// served name and then the raw enum stand in when no alias names the enum.
func ResolveAntigravityModel(modelEnum, servedModel string, aliases map[string]ModelAlias, releases ReleaseMap, stepDate string) string {
	enum := strings.TrimSpace(modelEnum)
	served := strings.TrimSpace(servedModel)

	if enum != "" {
		if alias, ok := findAlias(enum, aliases); ok {
			if family := familyFromLabel(alias.Label); family != "" {
				return resolveFamily(family, releases, stepDate)
			}
			return formatModelID(alias.ModelID)
		}
	}
	if served != "" && !modelid.IsPseudo(served) {
		if family := familyFromRoutingSlug(served); family != "" {
			return resolveFamily(family, releases, stepDate)
		}
		return formatModelID(served)
	}
	if enum != "" {
		if alias, ok := findAlias(enum, aliases); ok {
			if family := familyFromLabel(alias.Label); family != "" {
				return resolveFamily(family, releases, stepDate)
			}
		}
		return formatModelID(enum)
	}
	if served != "" {
		if family := familyFromRoutingSlug(served); family != "" {
			return resolveFamily(family, releases, stepDate)
		}
	}
	return "unknown"
}

// familyFromLabel returns a family codename when the label names only one and
// carries no version.
func familyFromLabel(label string) string {
	if strings.TrimSpace(label) == "" {
		return ""
	}
	cleaned := strings.NewReplacer("(", " ", ")", " ").Replace(label)
	hasDigit := false
	for _, r := range cleaned {
		if r >= '0' && r <= '9' {
			hasDigit = true
			break
		}
	}
	if hasDigit {
		return ""
	}
	var words []string
	for _, token := range strings.Fields(strings.ToLower(cleaned)) {
		if token == "gemini" || token == "orchestrator" || token == "worker" ||
			token == "agent" || token == "step" || token == "high" || token == "medium" ||
			token == "low" || token == "thinking" || token == "image" {
			continue
		}
		words = append(words, token)
	}
	if len(words) != 1 {
		return ""
	}
	return words[0]
}

// familyFromRoutingSlug extracts the family a routing slug names.
func familyFromRoutingSlug(slug string) string {
	lower := strings.ToLower(strings.TrimSpace(slug))
	const orchestrator = "gemini-pro-orchestrator"
	if lower == orchestrator {
		return "carbon"
	}
	if strings.HasPrefix(lower, orchestrator+"-") {
		rest := strings.TrimPrefix(lower, orchestrator+"-")
		if rest == "" {
			return ""
		}
		if rest == "g1-pro" {
			return "argon"
		}
		return rest
	}
	if strings.HasPrefix(lower, "gemini-") && strings.HasSuffix(lower, "-worker") {
		return strings.TrimSuffix(strings.TrimPrefix(lower, "gemini-"), "-worker")
	}
	if strings.HasPrefix(lower, "gemini-") && strings.HasSuffix(lower, "-worker-agy") {
		return strings.TrimSuffix(strings.TrimPrefix(lower, "gemini-"), "-worker-agy")
	}
	return ""
}

type geminiFamilyModel struct {
	model   string
	major   int
	minor   int
	release string
	dated   bool
}

// resolveFamily picks the family's official model current on stepDate.
func resolveFamily(family string, releases ReleaseMap, stepDate string) string {
	family = strings.ToLower(strings.TrimSpace(family))
	if family == "" {
		return "unknown"
	}
	var candidates []geminiFamilyModel
	for model, release := range releases {
		lower := strings.ToLower(model)
		if !strings.HasPrefix(lower, "gemini-") {
			continue
		}
		rest := strings.TrimPrefix(lower, "gemini-")
		parts := strings.Split(rest, "-")
		if len(parts) < 2 {
			continue
		}
		major, err := strconv.Atoi(parts[0])
		if err != nil {
			continue
		}
		minor := 0
		familyParts := parts[1:]
		if len(parts) >= 3 {
			if m, err := strconv.Atoi(parts[1]); err == nil {
				minor = m
				familyParts = parts[2:]
			}
		}
		if strings.Join(familyParts, "-") != family {
			continue
		}
		candidates = append(candidates, geminiFamilyModel{
			model: model, major: major, minor: minor, release: release, dated: release != "",
		})
	}
	if len(candidates) == 0 {
		return formatModelID("gemini-" + family)
	}
	sort.SliceStable(candidates, func(i, j int) bool {
		if candidates[i].major != candidates[j].major {
			return candidates[i].major > candidates[j].major
		}
		return candidates[i].minor > candidates[j].minor
	})
	if stepDate != "" {
		for _, candidate := range candidates {
			if candidate.dated && candidate.release <= stepDate {
				return formatModelID(candidate.model)
			}
		}
	}
	var dated []geminiFamilyModel
	for _, candidate := range candidates {
		if candidate.dated {
			dated = append(dated, candidate)
		}
	}
	if len(dated) > 0 {
		earliest := dated[0]
		for _, candidate := range dated[1:] {
			if candidate.release < earliest.release ||
				(candidate.release == earliest.release &&
					(candidate.major > earliest.major ||
						(candidate.major == earliest.major && candidate.minor > earliest.minor))) {
				earliest = candidate
			}
		}
		return formatModelID(earliest.model)
	}
	return formatModelID(candidates[0].model)
}
