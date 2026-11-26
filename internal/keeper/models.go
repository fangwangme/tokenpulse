package keeper

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// ResolveCheapestModel finds the newest cheap model an agent CLI accepts from
// its local catalogs. There is deliberately no fallback.
func ResolveCheapestModel(agent, dataDir string) (string, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return "", errors.New("home directory not found")
	}
	switch agent {
	case "claude":
		return resolveClaude(filepath.Join(home, ".claude", "cache", "model-catalog"))
	case "codex":
		return resolveCodex(filepath.Join(home, ".codex", "models_cache.json"))
	case "antigravity":
		return resolveAntigravity(filepath.Join(dataDir, "antigravity-cache", "model-aliases.json"))
	default:
		return "", fmt.Errorf("no model catalog for Keeper agent '%s'", agent)
	}
}

func resolveClaude(dir string) (string, error) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", fmt.Errorf("failed to read Claude model catalog '%s': %w", dir, err)
	}
	var newest string
	var newestTime time.Time
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		if newest == "" || info.ModTime().After(newestTime) {
			newest = filepath.Join(dir, entry.Name())
			newestTime = info.ModTime()
		}
	}
	if newest == "" {
		return "", fmt.Errorf("no catalog files in '%s'", dir)
	}
	value, err := readJSON(newest)
	if err != nil {
		return "", err
	}
	models, _ := value["catalog"].(map[string]any)
	config, _ := models["config"].(map[string]any)
	list, _ := config["models"].([]any)
	if list == nil {
		return "", fmt.Errorf("no model list in '%s'", newest)
	}
	var ids []string
	for _, raw := range list {
		model, _ := raw.(map[string]any)
		if stringField(model, "section") != "main" {
			continue
		}
		id := stringField(model, "id")
		if strings.Contains(id, "haiku") {
			ids = append(ids, id)
		}
	}
	if id := newestVersion(ids); id != "" {
		return id, nil
	}
	return "", fmt.Errorf("no active Haiku model in '%s'", newest)
}

func resolveCodex(file string) (string, error) {
	value, err := readJSON(file)
	if err != nil {
		return "", err
	}
	list, _ := value["models"].([]any)
	if list == nil {
		return "", fmt.Errorf("no model list in '%s'", file)
	}
	var ids []string
	for _, raw := range list {
		model, _ := raw.(map[string]any)
		if stringField(model, "visibility") == "hide" {
			continue
		}
		if strings.Contains(strings.ToLower(stringField(model, "description")), "older") {
			continue
		}
		slug := stringField(model, "slug")
		if strings.Contains(slug, "luna") {
			ids = append(ids, slug)
		}
	}
	if id := newestVersion(ids); id != "" {
		return id, nil
	}
	return "", fmt.Errorf("no active Luna model in '%s'", file)
}

func resolveAntigravity(file string) (string, error) {
	value, err := readJSON(file)
	if err != nil {
		return "", err
	}
	aliases, _ := value["aliases"].(map[string]any)
	if aliases == nil {
		return "", fmt.Errorf("no alias map in '%s'", file)
	}
	var ids []string
	for _, raw := range aliases {
		alias, _ := raw.(map[string]any)
		label := stringField(alias, "label")
		if id := antigravityCLIModelID(label); id != "" && strings.Contains(id, "flash") && strings.HasSuffix(id, "-low") {
			ids = append(ids, id)
		}
	}
	if id := newestVersion(ids); id != "" {
		return id, nil
	}
	return "", fmt.Errorf("no Flash (Low) model in '%s'", file)
}

func antigravityCLIModelID(label string) string {
	replaced := strings.NewReplacer("(", " ", ")", " ").Replace(label)
	fields := strings.Fields(replaced)
	for i := range fields {
		fields[i] = strings.ToLower(fields[i])
	}
	return strings.Join(fields, "-")
}

// ResolveAntigravityGroupModel picks a non-primary group's model from what
// `agy models` lists now.
func ResolveAntigravityGroupModel(group string, groups []string) (string, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, "agy", "models")
	cmd.Stdin = nil
	out, err := cmd.Output()
	if ctx.Err() == context.DeadlineExceeded {
		return "", errors.New("`agy models` timed out after 30s")
	}
	if err != nil {
		return "", fmt.Errorf("failed to run `agy models`: %w", err)
	}
	return SelectAntigravityGroupModel(string(out), group, groups)
}

// SelectAntigravityGroupModel picks the cheapest tier, then newest version,
// then lowest thinking level among models of the group's slug.
func SelectAntigravityGroupModel(output, group string, groups []string) (string, error) {
	slug := groupSlug(group)
	slugs := make([]string, 0, len(groups)+1)
	for _, g := range groups {
		slugs = append(slugs, groupSlug(g))
	}
	slugs = append(slugs, slug)
	var candidates []string
	for _, line := range strings.Split(output, "\n") {
		parts := strings.SplitN(line, "\t", 2)
		if len(parts) != 2 {
			continue
		}
		id := strings.TrimSpace(parts[0])
		label := strings.TrimSpace(parts[1])
		if id == "" || label == "" {
			continue
		}
		if owningSlug(id, slugs) == slug {
			candidates = append(candidates, id)
		}
	}
	if len(candidates) == 0 {
		return "", fmt.Errorf("`agy models` lists no model for Antigravity group '%s'", group)
	}
	best := candidates[0]
	for _, candidate := range candidates[1:] {
		if betterGroupModel(candidate, best) {
			best = candidate
		}
	}
	return best, nil
}

func betterGroupModel(a, b string) bool {
	if modelTier(a) != modelTier(b) {
		return modelTier(a) < modelTier(b)
	}
	if cmp := compareVersions(versionNumbers(a), versionNumbers(b)); cmp != 0 {
		return cmp > 0
	}
	if thinkingRank(a) != thinkingRank(b) {
		return thinkingRank(a) < thinkingRank(b)
	}
	return a < b
}

func groupSlug(group string) string {
	fields := strings.Fields(group)
	for i := range fields {
		fields[i] = strings.ToLower(fields[i])
	}
	return strings.Join(fields, "-")
}

func owningSlug(id string, slugs []string) string {
	best := ""
	for _, slug := range slugs {
		if slug == "" || !strings.HasPrefix(id, slug) {
			continue
		}
		rest := id[len(slug):]
		if rest != "" && !strings.HasPrefix(rest, "-") {
			continue
		}
		if len(slug) > len(best) {
			best = slug
		}
	}
	return best
}

func modelTier(id string) int {
	for _, tier := range []string{"flash", "haiku", "lite"} {
		if strings.Contains(id, tier) {
			return 0
		}
	}
	if strings.Contains(id, "opus") {
		return 2
	}
	return 1
}

func thinkingRank(id string) int {
	switch {
	case strings.HasSuffix(id, "-low"):
		return 0
	case strings.HasSuffix(id, "-medium"):
		return 2
	case strings.HasSuffix(id, "-high"):
		return 3
	default:
		return 1
	}
}

func readJSON(path string) (map[string]any, error) {
	content, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read '%s': %w", path, err)
	}
	var value map[string]any
	if err := json.Unmarshal(content, &value); err != nil {
		return nil, fmt.Errorf("failed to parse '%s': %w", path, err)
	}
	return value, nil
}

func stringField(value map[string]any, key string) string {
	s, _ := value[key].(string)
	return s
}

func newestVersion(ids []string) string {
	best := ""
	for _, id := range ids {
		if best == "" || compareVersions(versionNumbers(id), versionNumbers(best)) > 0 ||
			(compareVersions(versionNumbers(id), versionNumbers(best)) == 0 && id > best) {
			best = id
		}
	}
	return best
}

func versionNumbers(id string) []int {
	var out []int
	for _, run := range strings.FieldsFunc(id, func(r rune) bool { return r < '0' || r > '9' }) {
		if n, err := strconv.Atoi(run); err == nil {
			out = append(out, n)
		}
	}
	return out
}

func compareVersions(a, b []int) int {
	for i := 0; i < len(a) || i < len(b); i++ {
		left, right := 0, 0
		if i < len(a) {
			left = a[i]
		}
		if i < len(b) {
			right = b[i]
		}
		if left != right {
			if left > right {
				return 1
			}
			return -1
		}
	}
	return 0
}
