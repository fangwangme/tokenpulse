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

// PiParser reads Pi agent session files.
type PiParser struct{}

const piParserVersion = "pi-v3"

func NewPiParser() *PiParser { return &PiParser{} }

func (p *PiParser) Name() string                     { return "pi" }
func (p *PiParser) ParserVersion() string            { return piParserVersion }
func (p *PiParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *PiParser) SessionPaths() []string {
	home, _ := os.UserHomeDir()
	return []string{filepath.Join(home, ".pi", "agent", "sessions")}
}

type piEntry struct {
	Type      string     `json:"type"`
	ID        string     `json:"id"`
	SessionID string     `json:"session_id"`
	Model     string     `json:"model"`
	ModelID   string     `json:"modelId"`
	Provider  string     `json:"provider"`
	Message   *piMessage `json:"message"`
	Timestamp any        `json:"timestamp"`
}

type piMessage struct {
	Role       string   `json:"role"`
	Model      string   `json:"model"`
	Provider   string   `json:"provider"`
	Usage      *piUsage `json:"usage"`
	Timestamp  *int64   `json:"timestamp"`
	ResponseID string   `json:"responseId"`
}

type piUsage struct {
	Input        *int64 `json:"input"`
	Output       *int64 `json:"output"`
	InputTokens  *int64 `json:"input_tokens"`
	OutputTokens *int64 `json:"output_tokens"`
	CacheRead    *int64 `json:"cacheRead"`
	CacheWrite   *int64 `json:"cacheWrite"`
}

func (p *PiParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	var all []provider.UnifiedMessage
	for _, root := range p.SessionPaths() {
		files := DiscoverFiles(root, []string{"jsonl"}, since)
		all = append(all, ParseFilesParallel(files, p.parseFile)...)
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].Timestamp < all[j].Timestamp })
	return all, nil
}

func (p *PiParser) parseFile(path string) []provider.UnifiedMessage {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	var messages []provider.UnifiedMessage
	var currentSession, currentModel, currentProvider string
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 64*1024), 16*1024*1024)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" {
			continue
		}
		var entry piEntry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue
		}
		switch entry.Type {
		case "session", "header":
			currentSession = entry.ID
			if currentSession == "" {
				currentSession = entry.SessionID
			}
			if currentSession == "" {
				currentSession = strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
			}
			if entry.ModelID != "" {
				currentModel = entry.ModelID
			} else if entry.Model != "" {
				currentModel = entry.Model
			}
			if entry.Provider != "" {
				currentProvider = entry.Provider
			}
		case "model_change":
			if entry.ModelID != "" {
				currentModel = entry.ModelID
			} else if entry.Model != "" {
				currentModel = entry.Model
			}
			if entry.Provider != "" {
				currentProvider = entry.Provider
			}
		case "message", "assistant":
			message := entry.Message
			if message == nil || message.Role != "assistant" || message.Usage == nil {
				continue
			}
			usage := message.Usage
			tokens := provider.TokenBreakdown{
				Input:      max64(firstNonNil(usage.Input, usage.InputTokens), 0),
				Output:     max64(firstNonNil(usage.Output, usage.OutputTokens), 0),
				CacheRead:  max64(deref(usage.CacheRead), 0),
				CacheWrite: max64(deref(usage.CacheWrite), 0),
			}
			if tokens.IsEmpty() {
				continue
			}
			model := message.Model
			if model == "" {
				model = currentModel
			}
			if model == "" {
				model = "unknown"
			}
			providerID := message.Provider
			if providerID == "" {
				providerID = currentProvider
			}
			if providerID == "" {
				providerID = modelid.DetectProviderFromModel(model)
			}
			sessionID := currentSession
			if sessionID == "" {
				sessionID = entry.ID
			}
			if sessionID == "" {
				sessionID = strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
			}
			timestamp := piTimestampMillis(entry.Timestamp)
			if message.Timestamp != nil {
				timestamp = *message.Timestamp
			} else if timestamp == 0 {
				timestamp = time.Now().UnixMilli()
			}
			key := message.ResponseID
			if key == "" {
				key = entry.ID
			}
			if key == "" {
				key = sessionID + ":" + strconvI64(timestamp) + ":" + model
			}
			msg := provider.NewMessage("pi", model, providerID, sessionID, key, timestamp, tokens)
			msg.PricingDay = provider.LocalDateString(timestamp)
			msg.ParserVersion = piParserVersion
			messages = append(messages, msg)
		}
	}
	return messages
}

func firstNonNil(a, b *int64) int64 {
	if a != nil {
		return *a
	}
	if b != nil {
		return *b
	}
	return 0
}

func piTimestampMillis(value any) int64 {
	switch v := value.(type) {
	case string:
		ts, _ := parseRFC3339Millis(v)
		return ts
	case float64:
		return int64(v)
	case json.Number:
		n, _ := v.Int64()
		return n
	}
	return 0
}
