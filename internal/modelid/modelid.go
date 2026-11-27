// Package modelid normalizes model ids exactly as the Rust release did.
package modelid

import "strings"

// Canonical is the display and grouping id: one name per model across every
// source. `-preview` is dropped wherever it sits, along with tier and routing
// suffixes.
func Canonical(modelID string) string {
	withoutPreview := make([]string, 0, 8)
	for _, seg := range strings.Split(PricingKey(modelID), "-") {
		if seg != "preview" {
			withoutPreview = append(withoutPreview, seg)
		}
	}
	return stripVariantSuffixes(strings.Join(withoutPreview, "-"))
}

// PricingKey is the catalog key: the same spelling rules as Canonical, but
// `-preview` is kept because catalogs price it separately.
func PricingKey(modelID string) string {
	normalized := strings.ToLower(strings.TrimSpace(modelID))
	if stripped, ok := stripDateSuffix(normalized); ok {
		normalized = stripped
	}
	if idx := strings.LastIndex(normalized, "/"); idx >= 0 {
		normalized = normalized[idx+1:]
	}
	normalized = strings.NewReplacer(".", "-", "_", "-", " ", "-").Replace(normalized)
	normalized = CollapseRepeatedHyphens(normalized)
	for _, prefix := range []string{"antigravity-", "anti-gravity-"} {
		if strings.HasPrefix(normalized, prefix) {
			normalized = strings.TrimPrefix(normalized, prefix)
			break
		}
	}
	return stripVariantSuffixes(normalized)
}

func stripDateSuffix(modelID string) (string, bool) {
	if len(modelID) > 9 && modelID[len(modelID)-9] == '-' && allDigits(modelID[len(modelID)-8:]) {
		return modelID[:len(modelID)-9], true
	}
	if len(modelID) > 11 && modelID[len(modelID)-11] == '-' {
		tail := modelID[len(modelID)-10:]
		ok := true
		for i, c := range []byte(tail) {
			if i == 4 || i == 7 {
				if c != '-' {
					ok = false
					break
				}
			} else if c < '0' || c > '9' {
				ok = false
				break
			}
		}
		if ok {
			return modelID[:len(modelID)-11], true
		}
	}
	return "", false
}

func allDigits(s string) bool {
	if s == "" {
		return false
	}
	for i := 0; i < len(s); i++ {
		if s[i] < '0' || s[i] > '9' {
			return false
		}
	}
	return true
}

var variantSuffixes = []string{"-xhigh", "-high", "-medium", "-low", "-free", "-thinking", "-tiered"}

func stripVariantSuffixes(modelID string) string {
	normalized := modelID
	for {
		stripped := false
		for _, suffix := range variantSuffixes {
			if strings.HasSuffix(normalized, suffix) {
				normalized = strings.TrimSuffix(normalized, suffix)
				stripped = true
				break
			}
		}
		if !stripped {
			break
		}
	}
	normalized = strings.TrimSuffix(normalized, "-0")
	return strings.Trim(normalized, "-")
}

// CollapseRepeatedHyphens turns `a---b` into `a-b`.
func CollapseRepeatedHyphens(value string) string {
	var b strings.Builder
	b.Grow(len(value))
	lastHyphen := false
	for _, r := range value {
		if r == '-' {
			if !lastHyphen {
				b.WriteRune(r)
			}
			lastHyphen = true
		} else {
			b.WriteRune(r)
			lastHyphen = false
		}
	}
	return b.String()
}

// IsPseudo reports ids that are routing aliases, internal features or parser
// fallbacks and can never resolve against a price catalog.
func IsPseudo(modelID string) bool {
	id := strings.ToLower(strings.TrimSpace(modelID))
	return id == "" || id == "unknown" || strings.HasPrefix(id, "auto-") ||
		strings.HasSuffix(id, "-auto-review") || strings.HasSuffix(id, "-default")
}

// DetectProviderFromModel maps a model id to its owning company, as the Rust
// release's usage helper did.
func DetectProviderFromModel(model string) string {
	normalized := Canonical(model)
	switch {
	case strings.HasPrefix(normalized, "codex"),
		strings.HasPrefix(normalized, "gpt"),
		strings.HasPrefix(normalized, "o1"),
		strings.HasPrefix(normalized, "o3"),
		strings.HasPrefix(normalized, "o4"):
		return "openai"
	case strings.HasPrefix(normalized, "antigravity-claude"),
		strings.HasPrefix(normalized, "anti-gravity-claude"),
		strings.HasPrefix(normalized, "claude"),
		strings.HasPrefix(normalized, "sonnet"),
		strings.HasPrefix(normalized, "opus"),
		strings.HasPrefix(normalized, "haiku"):
		return "anthropic"
	case strings.HasPrefix(normalized, "antigravity-gemini"),
		strings.HasPrefix(normalized, "anti-gravity-gemini"),
		strings.HasPrefix(normalized, "gemini"):
		return "google"
	default:
		return "other"
	}
}
