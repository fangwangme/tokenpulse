package usage

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// CodexParser reads Codex rollout files. The 0.6.16 reference discovers plain
// .jsonl only; compressed .jsonl.zst files were never parsed.
type CodexParser struct{}

const codexParserVersion = "codex-v4"

func NewCodexParser() *CodexParser { return &CodexParser{} }

func (p *CodexParser) Name() string                     { return "codex" }
func (p *CodexParser) ParserVersion() string            { return codexParserVersion }
func (p *CodexParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *CodexParser) SessionPaths() []string {
	home, _ := os.UserHomeDir()
	codexHome := os.Getenv("CODEX_HOME")
	if codexHome == "" {
		codexHome = filepath.Join(home, ".codex")
	}
	return []string{
		filepath.Join(home, ".codex", "sessions"),
		filepath.Join(home, ".codex", "archived_sessions"),
		filepath.Join(codexHome, "sessions"),
	}
}

type codexUsage struct {
	input     int64
	output    int64
	cached    int64
	reasoning int64
}

func (u codexUsage) total() int64 { return u.input + u.output }

func (u codexUsage) delta(previous codexUsage) (codexUsage, bool) {
	if u.input < previous.input || u.output < previous.output || u.cached < previous.cached || u.reasoning < previous.reasoning {
		return codexUsage{}, false
	}
	return codexUsage{
		input: u.input - previous.input, output: u.output - previous.output,
		cached: u.cached - previous.cached, reasoning: u.reasoning - previous.reasoning,
	}, true
}

func (u codexUsage) tokens() provider.TokenBreakdown {
	cached := u.cached
	if cached > u.input {
		cached = u.input
	}
	if cached < 0 {
		cached = 0
	}
	reasoning := u.reasoning
	if reasoning > u.output {
		reasoning = u.output
	}
	if reasoning < 0 {
		reasoning = 0
	}
	return provider.TokenBreakdown{
		Input: max64(u.input-cached, 0), Output: max64(u.output-reasoning, 0),
		CacheRead: cached, Reasoning: reasoning,
	}
}

func (p *CodexParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	var all []provider.UnifiedMessage
	for _, root := range p.SessionPaths() {
		files := DiscoverFiles(root, []string{"jsonl"}, since)
		all = append(all, ParseFilesParallel(files, p.parseFile)...)
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].Timestamp < all[j].Timestamp })
	return all, nil
}

func (p *CodexParser) parseFile(path string) []provider.UnifiedMessage {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	var messages []provider.UnifiedMessage
	var currentModel string
	var previousTotals *codexUsage
	inheritsHistory := false
	turnStarted := false
	providerID := "openai"
	sessionID := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))

	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 64*1024), 16*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var value map[string]any
		if err := json.Unmarshal([]byte(line), &value); err != nil {
			continue
		}
		entryType, _ := value["type"].(string)
		payload, _ := value["payload"].(map[string]any)

		if entryType == "session_meta" {
			if payload != nil {
				if id, ok := payload["id"].(string); ok && id != "" {
					sessionID = id
				}
				if prov, ok := payload["model_provider"].(string); ok && prov != "" {
					providerID = prov
				}
			}
			if model, ok := extractCodexModel(payload, &value); ok {
				currentModel = model
			}
			if payload != nil {
				if s, ok := payload["forked_from_id"].(string); ok && s != "" {
					inheritsHistory = true
				}
				if s, ok := payload["parent_thread_id"].(string); ok && s != "" {
					inheritsHistory = true
				}
				if source, ok := payload["source"].(map[string]any); ok {
					if _, ok := source["subagent"]; ok {
						inheritsHistory = true
					}
				}
			}
			continue
		}

		if entryType == "turn_context" {
			turnStarted = true
		}
		if model, ok := extractCodexModel(payload, &value); ok {
			currentModel = model
		}
		if entryType != "event_msg" {
			continue
		}
		if payload == nil {
			continue
		}
		if t, _ := payload["type"].(string); t != "token_count" {
			continue
		}
		info, _ := payload["info"].(map[string]any)
		if info == nil {
			continue
		}

		if inheritsHistory && !turnStarted {
			if total, ok := parseCodexUsage(info["total_token_usage"]); ok && total.total() > 0 {
				previousTotals = &total
			}
			continue
		}

		if model, ok := extractCodexModel(info, &value); ok {
			currentModel = model
		}
		modelID := currentModel
		if modelID == "" {
			modelID = "unknown"
		}
		timestamp, _ := extractTimestampMS(value)
		if timestamp == 0 {
			timestamp, _ = extractTimestampMS(payload)
		}

		lastUsage, hasLast := parseCodexUsage(info["last_token_usage"])
		if hasLast && lastUsage.total() == 0 {
			hasLast = false
		}
		totalUsage, hasTotal := parseCodexUsage(info["total_token_usage"])
		if hasTotal && totalUsage.total() == 0 {
			hasTotal = false
		}

		var usage codexUsage
		if hasLast {
			if previousTotals != nil && hasTotal {
				if totalUsage.total() == previousTotals.total() {
					continue
				}
				if totalUsage.total() < previousTotals.total() && totalUsage.total()+lastUsage.total() >= previousTotals.total() {
					continue
				}
			}
			if hasTotal {
				previousTotals = &totalUsage
			}
			usage = lastUsage
		} else if hasTotal && previousTotals != nil {
			delta, ok := totalUsage.delta(*previousTotals)
			previousTotals = &totalUsage
			if !ok || delta.total() <= 0 {
				continue
			}
			usage = delta
		} else if hasTotal {
			previousTotals = &totalUsage
			usage = totalUsage
		} else {
			continue
		}

		tokens := usage.tokens()
		if tokens.IsEmpty() {
			continue
		}
		key := sessionID + ":" + strconv.FormatInt(timestamp, 10) + ":" + modelID + ":" +
			strconv.FormatInt(tokens.Input, 10) + ":" + strconv.FormatInt(tokens.Output, 10) + ":" +
			strconv.FormatInt(tokens.CacheRead, 10) + ":" + strconv.FormatInt(tokens.Reasoning, 10)
		msg := provider.NewMessage("codex", modelID, providerID, sessionID, key, timestamp, tokens)
		msg.ParserVersion = codexParserVersion
		messages = append(messages, msg)
	}
	return messages
}

func extractCodexModel(primary map[string]any, fallback *map[string]any) (string, bool) {
	source := primary
	if source == nil {
		source = *fallback
	}
	if source == nil {
		return "", false
	}
	for _, key := range []string{"model", "model_name"} {
		if s, ok := source[key].(string); ok && s != "" {
			return s, true
		}
	}
	if info, ok := source["model_info"].(map[string]any); ok {
		if s, ok := info["slug"].(string); ok && s != "" {
			return s, true
		}
	}
	if payload, ok := source["payload"].(map[string]any); ok {
		if s, ok := payload["model"].(string); ok && s != "" {
			return s, true
		}
	}
	return "", false
}

func parseCodexUsage(raw any) (codexUsage, bool) {
	value, ok := raw.(map[string]any)
	if !ok {
		return codexUsage{}, false
	}
	input, ok := codexInt(value, "input_tokens", "input")
	if !ok {
		return codexUsage{}, false
	}
	output, _ := codexInt(value, "output_tokens", "output")
	cached, _ := codexInt(value, "cached_input_tokens", "cache_read_input_tokens", "cached_tokens")
	reasoning, _ := codexInt(value, "reasoning_output_tokens", "reasoning_tokens")
	return codexUsage{input: max64(input, 0), output: max64(output, 0), cached: max64(cached, 0), reasoning: max64(reasoning, 0)}, true
}

func codexInt(value map[string]any, keys ...string) (int64, bool) {
	for _, key := range keys {
		switch v := value[key].(type) {
		case float64:
			return int64(v), true
		case int64:
			return v, true
		case json.Number:
			n, err := v.Int64()
			if err == nil {
				return n, true
			}
		}
	}
	return 0, false
}

func extractTimestampMS(value map[string]any) (int64, bool) {
	switch v := value["timestamp"].(type) {
	case float64:
		return int64(v), true
	case int64:
		return v, true
	case json.Number:
		n, err := v.Int64()
		return n, err == nil
	case string:
		return parseRFC3339Millis(v)
	}
	return 0, false
}
