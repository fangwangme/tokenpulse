package usage

import (
	"bufio"
	"encoding/json"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/fangwangme/tokenpulse/internal/provider"
)

// GeminiParser reads Gemini CLI session files (legacy JSON and streamed JSONL).
type GeminiParser struct{}

const geminiParserVersion = "gemini-v3"

func NewGeminiParser() *GeminiParser { return &GeminiParser{} }

func (p *GeminiParser) Name() string                     { return "gemini" }
func (p *GeminiParser) ParserVersion() string            { return geminiParserVersion }
func (p *GeminiParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *GeminiParser) SessionPaths() []string {
	home, _ := os.UserHomeDir()
	return []string{filepath.Join(home, ".gemini", "tmp")}
}

// GeminiTokens is the token object of one Gemini message.
type GeminiTokens struct {
	Input    *int64 `json:"input"`
	Output   *int64 `json:"output"`
	Cached   *int64 `json:"cached"`
	Thoughts *int64 `json:"thoughts"`
	Tool     *int64 `json:"tool"`
	Total    *int64 `json:"total"`
}

func (t GeminiTokens) Breakdown() provider.TokenBreakdown {
	input := max64(deref(t.Input), 0)
	output := max64(deref(t.Output), 0)
	cacheRead := max64(deref(t.Cached), 0)
	reasoning := max64(deref(t.Thoughts), 0)
	tool := max64(deref(t.Tool), 0)
	total := max64(deref(t.Total), 0)
	if total > 0 && total == input+output+reasoning+tool {
		input -= cacheRead
		if input < 0 {
			input = 0
		}
	}
	known := input + output + cacheRead + reasoning + tool
	remaining := total - known
	if remaining < 0 {
		remaining = 0
	}
	return provider.TokenBreakdown{
		Input: input + tool + remaining, Output: output, CacheRead: cacheRead, Reasoning: reasoning,
	}
}

func (p *GeminiParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	var all []provider.UnifiedMessage
	for _, root := range p.SessionPaths() {
		files := DiscoverFiles(root, []string{"json", "jsonl"}, since)
		filtered := files[:0]
		for _, file := range files {
			name := filepath.Base(file)
			if strings.HasPrefix(name, "session-") || strings.HasSuffix(name, ".jsonl") {
				filtered = append(filtered, file)
			}
		}
		all = append(all, ParseFilesParallel(filtered, p.parseFile)...)
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].Timestamp < all[j].Timestamp })
	return all, nil
}

func (p *GeminiParser) parseFile(path string) []provider.UnifiedMessage {
	if strings.HasSuffix(path, ".jsonl") {
		return p.parseJSONL(path)
	}
	return p.parseJSON(path)
}

type geminiSession struct {
	SessionID   string          `json:"sessionId"`
	StartTime   string          `json:"startTime"`
	LastUpdated string          `json:"lastUpdated"`
	Messages    []geminiMessage `json:"messages"`
}

type geminiMessage struct {
	ID        string        `json:"id"`
	Timestamp string        `json:"timestamp"`
	Type      string        `json:"type"`
	Model     string        `json:"model"`
	Tokens    *GeminiTokens `json:"tokens"`
}

func (p *GeminiParser) parseJSON(path string) []provider.UnifiedMessage {
	content, err := os.ReadFile(path)
	if err != nil {
		return nil
	}
	var session geminiSession
	if err := json.Unmarshal(content, &session); err != nil {
		return nil
	}
	sessionID := session.SessionID
	if sessionID == "" {
		sessionID = strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	}
	var messages []provider.UnifiedMessage
	for index, message := range session.Messages {
		if message.Type != "gemini" || message.Tokens == nil || message.Model == "" {
			continue
		}
		timestamp := int64(0)
		if ts, ok := parseRFC3339Millis(message.Timestamp); ok {
			timestamp = ts
		} else if ts, ok := parseRFC3339Millis(session.LastUpdated); ok {
			timestamp = ts
		} else if ts, ok := parseRFC3339Millis(session.StartTime); ok {
			timestamp = ts
		}
		key := message.ID
		if key == "" {
			key = sessionID + ":" + itoaInt(index) + ":" + message.Model + ":" + strconvI64(deref(message.Tokens.Input))
		}
		msg := provider.NewMessage("gemini", message.Model, "google", sessionID, key, timestamp, message.Tokens.Breakdown())
		msg.ParserVersion = geminiParserVersion
		messages = append(messages, msg)
	}
	return messages
}

func (p *GeminiParser) parseJSONL(path string) []provider.UnifiedMessage {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()
	sessionID := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	var fallback int64
	var messages []provider.UnifiedMessage
	positions := map[string]int{}
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 64*1024), 16*1024*1024)
	index := 0
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		idx := index
		index++
		if line == "" {
			continue
		}
		var value map[string]any
		if err := json.Unmarshal([]byte(line), &value); err != nil {
			continue
		}
		if id, ok := value["sessionId"].(string); ok {
			sessionID = id
		}
		for _, field := range []string{"lastUpdated", "startTime"} {
			if s, ok := value[field].(string); ok {
				if ts, ok := parseRFC3339Millis(s); ok {
					fallback = ts
				}
			}
		}
		if t, _ := value["type"].(string); t != "gemini" {
			continue
		}
		modelID, _ := value["model"].(string)
		tokensRaw, ok := value["tokens"]
		if modelID == "" || !ok {
			continue
		}
		tokens := geminiTokensFromValue(tokensRaw)
		timestamp := fallback
		if s, ok := value["timestamp"].(string); ok {
			if ts, ok := parseRFC3339Millis(s); ok {
				timestamp = ts
			}
		}
		key, _ := value["id"].(string)
		if key == "" {
			key = sessionID + ":" + itoaInt(idx) + ":" + modelID + ":" + strconvI64(deref(tokens.Input))
		}
		msg := provider.NewMessage("gemini", modelID, "google", sessionID, key, timestamp, tokens.Breakdown())
		msg.ParserVersion = geminiParserVersion
		if pos, ok := positions[key]; ok {
			messages[pos] = msg
		} else {
			positions[key] = len(messages)
			messages = append(messages, msg)
		}
	}
	return messages
}

func geminiTokensFromValue(raw any) GeminiTokens {
	data, err := json.Marshal(raw)
	if err != nil {
		return GeminiTokens{}
	}
	var tokens GeminiTokens
	if err := json.Unmarshal(data, &tokens); err != nil {
		return GeminiTokens{}
	}
	return tokens
}
