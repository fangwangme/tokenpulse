package usage

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

const copilotParserVersion = "copilot-v3"
const copilotEventName = "gen_ai.client.inference.operation.details"

// CopilotParser reads Copilot OTEL event logs and session-state event logs.
type CopilotParser struct{}

func NewCopilotParser() *CopilotParser { return &CopilotParser{} }

func (p *CopilotParser) Name() string                     { return "copilot" }
func (p *CopilotParser) ParserVersion() string            { return copilotParserVersion }
func (p *CopilotParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *CopilotParser) SessionPaths() []string {
	home, _ := os.UserHomeDir()
	dataDir := os.Getenv("XDG_DATA_HOME")
	if dataDir == "" {
		dataDir = filepath.Join(home, ".local", "share")
	}
	return []string{
		filepath.Join(dataDir, "github-copilot"),
		filepath.Join(home, ".copilot", "session-state"),
	}
}

func (p *CopilotParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	var all []provider.UnifiedMessage
	for _, root := range p.SessionPaths() {
		files := DiscoverFiles(root, []string{"jsonl"}, since)
		all = append(all, ParseFilesParallel(files, p.parseFile)...)
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].Timestamp < all[j].Timestamp })
	return all, nil
}

func (p *CopilotParser) parseFile(path string) []provider.UnifiedMessage {
	if isAgentSessionFile(path) {
		return p.parseAgentSessionFile(path)
	}
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	var messages []provider.UnifiedMessage
	seenResponses := map[string]bool{}
	prevInput := map[[2]string]int64{}
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
		if message, ok := parseOtelEvent(value, seenResponses, prevInput); ok {
			messages = append(messages, message)
		}
	}
	return messages
}

func (p *CopilotParser) parseAgentSessionFile(path string) []provider.UnifiedMessage {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	defaultSessionID := filepath.Base(filepath.Dir(path))
	currentSessionID := defaultSessionID
	currentModel := "unknown"
	var fallback []provider.UnifiedMessage
	var latestShutdown []provider.UnifiedMessage
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
		if sessionID, modelID, ok := extractAgentSessionStart(value); ok {
			currentSessionID = sessionID
			if !isUnknownModel(modelID) {
				currentModel = modelID
			}
			continue
		}
		if modelID, ok := extractAgentModelChange(value); ok {
			if !isUnknownModel(modelID) {
				currentModel = modelID
			}
			continue
		}
		if messages, ok := parseSessionShutdown(value, currentSessionID); ok {
			latestShutdown = messages
			continue
		}
		if message, ok := parseAgentSessionEvent(value, currentSessionID, currentModel); ok {
			fallback = append(fallback, message)
		}
	}
	if latestShutdown != nil {
		return latestShutdown
	}
	return fallback
}

func isAgentSessionFile(path string) bool {
	if filepath.Base(path) != "events.jsonl" {
		return false
	}
	dir := filepath.Dir(path)
	for {
		if filepath.Base(dir) == "session-state" {
			return true
		}
		next := filepath.Dir(dir)
		if next == dir {
			return false
		}
		dir = next
	}
}

func extractSessionID(value map[string]any) (string, bool) {
	resource, _ := value["resource"].(map[string]any)
	if resource == nil {
		return "", false
	}
	attrs, _ := resource["_rawAttributes"].([]any)
	for _, entry := range attrs {
		arr, _ := entry.([]any)
		if len(arr) < 2 {
			continue
		}
		if key, _ := arr[0].(string); key != "session.id" {
			continue
		}
		switch v := arr[1].(type) {
		case string:
			return v, true
		case map[string]any:
			if s, ok := v["value"].(string); ok {
				return s, true
			}
		}
	}
	return "", false
}

func extractAgentSessionStart(value map[string]any) (string, string, bool) {
	if t, _ := value["type"].(string); t != "session.start" {
		return "", "", false
	}
	data, _ := value["data"].(map[string]any)
	if data == nil {
		return "", "", false
	}
	sessionID, _ := data["sessionId"].(string)
	if sessionID == "" {
		return "", "", false
	}
	modelID, _ := extractModelID(data)
	if modelID == "" {
		modelID = "unknown"
	}
	return sessionID, modelID, true
}

func extractAgentModelChange(value map[string]any) (string, bool) {
	if t, _ := value["type"].(string); t != "session.model_change" {
		return "", false
	}
	data, _ := value["data"].(map[string]any)
	if data == nil {
		return "", false
	}
	return extractModelID(data)
}

func extractModelID(data map[string]any) (string, bool) {
	for _, key := range []string{"model", "selectedModel", "newModel"} {
		if s, ok := data[key].(string); ok {
			if trimmed := strings.TrimSpace(s); trimmed != "" {
				return trimmed, true
			}
		}
	}
	return "", false
}

func isUnknownModel(modelID string) bool {
	trimmed := strings.TrimSpace(modelID)
	return trimmed == "" || strings.EqualFold(trimmed, "unknown")
}

func parseSessionShutdown(value map[string]any, sessionID string) ([]provider.UnifiedMessage, bool) {
	if t, _ := value["type"].(string); t != "session.shutdown" {
		return nil, false
	}
	data, _ := value["data"].(map[string]any)
	if data == nil {
		return nil, false
	}
	ts, ok := value["timestamp"].(string)
	if !ok {
		return nil, false
	}
	timestamp, ok := ParseTimestampString(ts)
	if !ok {
		return nil, false
	}
	metrics, _ := data["modelMetrics"].(map[string]any)
	if metrics == nil {
		return nil, false
	}
	var messages []provider.UnifiedMessage
	for modelID, raw := range metrics {
		metric, _ := raw.(map[string]any)
		if metric == nil {
			continue
		}
		requests, _ := metric["requests"].(map[string]any)
		usage, _ := metric["usage"].(map[string]any)
		if requests == nil || usage == nil {
			continue
		}
		requestCount := 1
		if n, ok := usageInt(requests, "count"); ok && int(n) > requestCount {
			requestCount = int(n)
		}
		totalInput, _ := usageInt(usage, "inputTokens")
		output, _ := usageInt(usage, "outputTokens")
		cacheRead, _ := usageInt(usage, "cacheReadTokens")
		cacheWrite, _ := usageInt(usage, "cacheWriteTokens")
		if totalInput == 0 && output == 0 && cacheRead == 0 && cacheWrite == 0 {
			continue
		}
		effectiveInput := totalInput - max64(cacheRead, 0) - max64(cacheWrite, 0)
		if effectiveInput < 0 {
			effectiveInput = 0
		}
		providerID := modelid.DetectProviderFromModel(modelID)
		for idx := 0; idx < requestCount; idx++ {
			tokens := provider.TokenBreakdown{
				Input:      distributeInt64(effectiveInput, requestCount, idx),
				Output:     distributeInt64(max64(output, 0), requestCount, idx),
				CacheRead:  distributeInt64(max64(cacheRead, 0), requestCount, idx),
				CacheWrite: distributeInt64(max64(cacheWrite, 0), requestCount, idx),
			}
			key := "copilot-session:" + sessionID + ":" + modelID + ":" + itoaInt(idx)
			msg := provider.NewMessage("copilot", modelID, providerID, sessionID, key, timestamp+int64(idx), tokens)
			msg.ParserVersion = copilotParserVersion
			messages = append(messages, msg)
		}
	}
	if len(messages) == 0 {
		return nil, false
	}
	return messages, true
}

func extractHRTime(value map[string]any) (int64, bool) {
	arr, _ := value["hrTime"].([]any)
	if len(arr) < 2 {
		return 0, false
	}
	seconds, ok := numberInt64(arr[0])
	if !ok {
		return 0, false
	}
	nanos, _ := numberInt64(arr[1])
	return seconds*1000 + nanos/1_000_000, true
}

func parseOtelEvent(value map[string]any, seen map[string]bool, prevInput map[[2]string]int64) (provider.UnifiedMessage, bool) {
	attrs, _ := value["attributes"].(map[string]any)
	if attrs == nil {
		return provider.UnifiedMessage{}, false
	}
	if event, _ := attrs["event.name"].(string); event != copilotEventName {
		return provider.UnifiedMessage{}, false
	}
	totalInput, _ := usageInt(attrs, "gen_ai.usage.input_tokens")
	output, _ := usageInt(attrs, "gen_ai.usage.output_tokens")
	responseID, _ := attrs["gen_ai.response.id"].(string)
	if responseID != "" {
		if seen[responseID] {
			return provider.UnifiedMessage{}, false
		}
		seen[responseID] = true
	}
	modelID := "unknown"
	for _, key := range []string{"gen_ai.response.model", "gen_ai.request.model"} {
		if s, ok := attrs[key].(string); ok && s != "" {
			modelID = s
			break
		}
	}
	sessionID, _ := extractSessionID(value)
	if sessionID == "" {
		sessionID = "unknown"
	}
	timestamp, _ := extractHRTime(value)
	providerID := modelid.DetectProviderFromModel(modelID)

	cacheKey := [2]string{sessionID, modelID}
	var effectiveInput, cacheRead, cacheWrite int64
	if read, ok := usageInt(attrs, "gen_ai.usage.cache_read.input_tokens"); ok {
		cacheRead = max64(read, 0)
		cacheWrite, _ = usageInt(attrs, "gen_ai.usage.cache_creation.input_tokens")
		cacheWrite = max64(cacheWrite, 0)
		effectiveInput = max64(totalInput-max64(cacheRead, 0)-max64(cacheWrite, 0), 0)
	} else if _, ok := usageInt(attrs, "gen_ai.usage.cache_creation.input_tokens"); ok {
		cacheWrite, _ = usageInt(attrs, "gen_ai.usage.cache_creation.input_tokens")
		cacheWrite = max64(cacheWrite, 0)
		effectiveInput = max64(totalInput-cacheWrite, 0)
	} else {
		effectiveInput = max64(totalInput, 0)
		if prev, ok := prevInput[cacheKey]; ok && totalInput >= prev {
			cacheRead = prev
			effectiveInput = totalInput - prev
		}
		prevInput[cacheKey] = totalInput
		effectiveInput = max64(effectiveInput, 0)
		cacheRead = max64(cacheRead, 0)
	}

	if effectiveInput == 0 && output == 0 && cacheRead == 0 && cacheWrite == 0 {
		return provider.UnifiedMessage{}, false
	}
	tokens := provider.TokenBreakdown{Input: effectiveInput, Output: max64(output, 0), CacheRead: cacheRead, CacheWrite: cacheWrite}
	key := ""
	if responseID != "" {
		key = "copilot:" + responseID
	} else {
		key = "copilot:" + sessionID + ":" + strconvI64(timestamp) + ":" + modelID + ":" +
			strconvI64(totalInput) + ":" + strconvI64(output)
	}
	msg := provider.NewMessage("copilot", modelID, providerID, sessionID, key, timestamp, tokens)
	msg.ParserVersion = copilotParserVersion
	return msg, true
}

func parseAgentSessionEvent(value map[string]any, sessionID, modelID string) (provider.UnifiedMessage, bool) {
	if t, _ := value["type"].(string); t != "assistant.message" {
		return provider.UnifiedMessage{}, false
	}
	data, _ := value["data"].(map[string]any)
	if data == nil {
		return provider.UnifiedMessage{}, false
	}
	outputTokens, _ := usageInt(data, "outputTokens")
	if outputTokens <= 0 {
		return provider.UnifiedMessage{}, false
	}
	ts, _ := value["timestamp"].(string)
	timestamp, ok := ParseTimestampString(ts)
	if !ok {
		return provider.UnifiedMessage{}, false
	}
	if eventModel, ok := extractModelID(data); ok {
		modelID = eventModel
	}
	if isUnknownModel(modelID) {
		return provider.UnifiedMessage{}, false
	}
	messageID, _ := data["messageId"].(string)
	interactionID, _ := data["interactionId"].(string)
	key := "copilot-agent:" + interactionID + ":" + messageID
	msg := provider.NewMessage("copilot", modelID, modelid.DetectProviderFromModel(modelID), sessionID, key, timestamp,
		provider.TokenBreakdown{Output: outputTokens})
	msg.ParserVersion = copilotParserVersion
	return msg, true
}

func distributeInt64(total int64, parts, idx int) int64 {
	if parts == 0 {
		return 0
	}
	base := total / int64(parts)
	remainder := total % int64(parts)
	if remainder < 0 {
		remainder = 0
	}
	if int64(idx) < remainder {
		return base + 1
	}
	return base
}

func usageInt(value map[string]any, key string) (int64, bool) {
	return numberInt64(value[key])
}

func numberInt64(value any) (int64, bool) {
	switch v := value.(type) {
	case float64:
		return int64(v), true
	case int64:
		return v, true
	case json.Number:
		n, err := v.Int64()
		return n, err == nil
	}
	return 0, false
}
