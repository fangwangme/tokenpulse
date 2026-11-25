package usage

import (
	"testing"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// Protects antigravity-model-aliases.md "Unversioned families".
func TestUnversionedFamilyResolutionAroundReleaseDates(t *testing.T) {
	releases := ReleaseMap{
		"gemini-3-5-flash": "2026-05-01",
		"gemini-3-6-flash": "2026-08-01",
		"gemini-3-7-flash": "2026-11-01",
	}
	cases := []struct {
		served string
		date   string
		want   string
	}{
		{"gemini-flash-worker-agy", "2026-06-01", "gemini-3-5-flash"},
		{"gemini-flash-worker", "2026-08-01", "gemini-3-6-flash"},
		{"gemini-flash-worker-agy", "2026-08-02", "gemini-3-6-flash"},
		{"gemini-flash-worker-agy", "2026-11-02", "gemini-3-7-flash"},
		{"gemini-flash-worker-agy", "2020-01-01", "gemini-3-5-flash"},
	}
	for _, tc := range cases {
		if got := ResolveAntigravityModel("", tc.served, nil, releases, tc.date); got != tc.want {
			t.Errorf("%s on %s = %s, want %s", tc.served, tc.date, got, tc.want)
		}
	}
}

// Protects antigravity-model-aliases.md "Unversioned families": routing slugs.
func TestUnversionedFamilyRoutingSlugs(t *testing.T) {
	releases := ReleaseMap{
		"gemini-4-carbon": "2026-06-01",
		"gemini-4-argon":  "2026-09-01",
	}
	cases := map[string]string{
		"gemini-pro-orchestrator":        "gemini-4-carbon",
		"gemini-pro-orchestrator-g1-pro": "gemini-4-argon",
	}
	for slug, want := range cases {
		if got := ResolveAntigravityModel("", slug, nil, releases, "2026-10-01"); got != want {
			t.Errorf("%s = %s, want %s", slug, got, want)
		}
	}
	if got := familyFromRoutingSlug("gemini-pro-orchestrator"); got != "carbon" {
		t.Errorf("orchestrator family = %s", got)
	}
	if got := familyFromRoutingSlug("gemini-pro-orchestrator-g1-pro"); got != "argon" {
		t.Errorf("g1-pro family = %s", got)
	}
	if got := familyFromRoutingSlug("gemini-neon-worker-agy"); got != "neon" {
		t.Errorf("worker family = %s", got)
	}
}

// Protects antigravity-model-aliases.md "Unversioned families": labels.
func TestUnversionedFamilyFromLabel(t *testing.T) {
	aliases := seedAliases()
	aliases[normalizeAliasKey("MODEL_PLACEHOLDER_M999")] = ModelAlias{
		RawModelID: "MODEL_PLACEHOLDER_M999", Label: "Orchestrator (Carbon)", Source: "test",
	}
	aliases[normalizeAliasKey("MODEL_PLACEHOLDER_M998")] = ModelAlias{
		RawModelID: "MODEL_PLACEHOLDER_M998", Label: "Gemini Flash", Source: "test",
	}
	releases := ReleaseMap{"gemini-3-8-flash": "2026-09-01", "gemini-3-9-carbon": "2026-10-01"}
	if got := ResolveAntigravityModel("MODEL_PLACEHOLDER_M999", "", aliases, releases, "2026-10-02"); got != "gemini-3-9-carbon" {
		t.Errorf("Carbon label = %s", got)
	}
	if got := ResolveAntigravityModel("MODEL_PLACEHOLDER_M998", "", aliases, releases, "2026-10-02"); got != "gemini-3-8-flash" {
		t.Errorf("Flash label = %s", got)
	}
}

// Protects antigravity-model-aliases.md "Resolution": a versioned alias wins
// over the served name, and the served name stands in when no alias exists.
func TestVersionedAliasAndServedName(t *testing.T) {
	aliases := seedAliases()
	if got := ResolveAntigravityModel("MODEL_PLACEHOLDER_M319", "gemini-3.8-flash-n", aliases, nil, "2026-10-02"); got != "gemini-3-8-flash-medium" {
		t.Errorf("versioned alias = %s", got)
	}
	if got := ResolveAntigravityModel("MODEL_PLACEHOLDER_UNKNOWN", "gemini-4-hydrogen", aliases, nil, "2026-10-02"); got != "gemini-4-hydrogen" {
		t.Errorf("served name = %s", got)
	}
	if got := ResolveAntigravityModel("MODEL_PLACEHOLDER_UNKNOWN", "", aliases, nil, "2026-10-02"); got != "model-placeholder-unknown" {
		t.Errorf("raw enum = %s", got)
	}
	if got := ResolveAntigravityModel("", "gemini-default", aliases, nil, "2026-10-02"); got != "unknown" {
		t.Errorf("pseudo served name = %s", got)
	}
}

// Protects antigravity-model-aliases.md "Unversioned families": a family with
// no catalog entry stays gemini-<family> until one appears.
func TestFamilyWithoutCatalogEntry(t *testing.T) {
	if got := ResolveAntigravityModel("", "gemini-neon-worker", nil, ReleaseMap{}, "2026-10-02"); got != "gemini-neon" {
		t.Errorf("missing family = %s", got)
	}
}

// Protects antigravity-model-aliases.md "Unversioned families": undated
// entries resolve to the highest version only when no entry is dated.
func TestFamilyResolutionFallsBackToHighestUndated(t *testing.T) {
	releases := ReleaseMap{
		"gemini-3-5-flash": "",
		"gemini-3-6-flash": "",
	}
	if got := ResolveAntigravityModel("", "gemini-flash-worker", nil, releases, "2026-10-02"); got != "gemini-3-6-flash" {
		t.Errorf("undated family = %s", got)
	}
}

// Protects usage-sources.md "Antigravity": thinking and response are disjoint.
func TestAntigravityTokensAreDisjoint(t *testing.T) {
	tokens, ok := normalizeAntigravityTokens(10, 100, 70, 5, 0, 30, true)
	if !ok || tokens.Output != 70 || tokens.Reasoning != 30 || tokens.Total() != 115 {
		t.Fatalf("tokens = %+v ok=%v", tokens, ok)
	}
	if _, ok := normalizeAntigravityTokens(10, 100, 60, 0, 0, 30, true); ok {
		t.Fatal("a partition mismatch must drop the record")
	}
	tokens, ok = normalizeAntigravityTokens(10, 100, 0, 0, 0, 30, false)
	if !ok || tokens.Output != 70 || tokens.Reasoning != 30 {
		t.Fatalf("total-only output = %+v", tokens)
	}
}

// Protects usage-sources.md "Antigravity": the protobuf field map.
func TestParseGenMetadataUsage(t *testing.T) {
	// usage { 2: 10, 3: 100, 5: 5, 9: 30, 10: 70, 11: "resp" }
	usage := protoTestVarintField(2, 10)
	usage = append(usage, protoTestVarintField(3, 100)...)
	usage = append(usage, protoTestVarintField(5, 5)...)
	usage = append(usage, protoTestVarintField(9, 30)...)
	usage = append(usage, protoTestVarintField(10, 70)...)
	usage = append(usage, protoTestMessage(11, []byte("resp"))...)
	payload := protoTestMessage(4, usage)
	payload = append(payload, protoTestMessage(19, []byte("gemini-3.8-flash-n"))...)
	pair := protoTestMessage(1, []byte("model_enum"))
	pair = append(pair, protoTestMessage(2, []byte("MODEL_PLACEHOLDER_M319"))...)
	payload = append(payload, protoTestMessage(20, pair)...)
	blob := protoTestMessage(1, payload)
	blob = append(blob, protoTestMessage(4, []byte("request-uuid"))...)

	parsed, ok := parseGenMetadataUsage(blob)
	if !ok {
		t.Fatal("blob did not parse")
	}
	if parsed.responseID != "resp" || parsed.servedModel != "gemini-3.8-flash-n" ||
		parsed.modelEnum != "MODEL_PLACEHOLDER_M319" || parsed.requestUUID != "request-uuid" {
		t.Fatalf("identities = %+v", parsed)
	}
	if parsed.tokens != (provider.TokenBreakdown{Input: 10, Output: 70, CacheRead: 5, Reasoning: 30}) {
		t.Fatalf("tokens = %+v", parsed.tokens)
	}
}

func protoTestVarint(value uint64) []byte {
	var out []byte
	for value >= 0x80 {
		out = append(out, byte(value)|0x80)
		value >>= 7
	}
	return append(out, byte(value))
}

func protoTestMessage(field uint32, payload []byte) []byte {
	key := protoTestVarint(uint64(field)<<3 | 2)
	key = append(key, protoTestVarint(uint64(len(payload)))...)
	return append(key, payload...)
}

func protoTestVarintField(field uint32, value uint64) []byte {
	return append(protoTestVarint(uint64(field)<<3), protoTestVarint(value)...)
}
