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

	"github.com/fangwangme/tokenpulse/internal/modelid"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

// IncrementalMode selects how a refresh merges a source's rows.
type IncrementalMode int

const (
	UpsertMessages IncrementalMode = iota
	ReplaceChangedSessions
)

type Parser interface {
	Name() string
	ParserVersion() string
	SessionPaths() []string
	Parse(since *time.Time) ([]provider.UnifiedMessage, error)
	IncrementalMode() IncrementalMode
}

// SessionPreservingParser is a parser whose full parse must replace only the
// sessions whose source files are readable, keeping rows for deleted files.
type SessionPreservingParser interface {
	PreserveMissingSessions() bool
}

// ClaudeParser reads Claude Code transcripts.
type ClaudeParser struct{}

const claudeParserVersion = "claude-v4"

func NewClaudeParser() *ClaudeParser { return &ClaudeParser{} }

func (p *ClaudeParser) Name() string                     { return "claude" }
func (p *ClaudeParser) ParserVersion() string            { return claudeParserVersion }
func (p *ClaudeParser) IncrementalMode() IncrementalMode { return ReplaceChangedSessions }

func (p *ClaudeParser) SessionPaths() []string {
	home, _ := os.UserHomeDir()
	return []string{
		filepath.Join(home, ".claude", "projects"),
		filepath.Join(home, ".claude", "transcripts"),
	}
}

type claudeEntry struct {
	Type      string         `json:"type"`
	RequestID string         `json:"requestId"`
	SessionID string         `json:"session_id"`
	Timestamp string         `json:"timestamp"`
	Message   *claudeMessage `json:"message"`
}

type claudeMessage struct {
	ID       string          `json:"id"`
	Model    string          `json:"model"`
	Provider string          `json:"provider"`
	Content  json.RawMessage `json:"content"`
	Usage    *claudeUsage    `json:"usage"`
}

type claudeUsage struct {
	InputTokens              *int64 `json:"input_tokens"`
	OutputTokens             *int64 `json:"output_tokens"`
	CacheReadInputTokens     *int64 `json:"cache_read_input_tokens"`
	CacheCreationInputTokens *int64 `json:"cache_creation_input_tokens"`
}

func (p *ClaudeParser) Parse(since *time.Time) ([]provider.UnifiedMessage, error) {
	var all []provider.UnifiedMessage
	positions := map[string]int{}
	for _, root := range p.SessionPaths() {
		files := DiscoverFiles(root, []string{"jsonl"}, since)
		for _, msg := range ParseFilesParallel(files, p.parseFile) {
			upsertClaudeMessage(&all, positions, msg)
		}
	}
	sort.SliceStable(all, func(i, j int) bool { return all[i].Timestamp < all[j].Timestamp })
	return all, nil
}

func (p *ClaudeParser) parseFile(path string) []provider.UnifiedMessage {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()

	sessionID := strings.TrimSuffix(filepath.Base(path), filepath.Ext(path))
	routeHint := ""
	var messages []provider.UnifiedMessage
	positions := map[string]int{}
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 64*1024), 16*1024*1024)
	lineIndex := 0
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		idx := lineIndex
		lineIndex++
		if line == "" {
			continue
		}
		var entry claudeEntry
		if err := json.Unmarshal([]byte(line), &entry); err != nil {
			continue
		}
		if hint, ok := claudeRouteHint(&entry); ok {
			routeHint = hint
		}
		if entry.Type != "assistant" || entry.Message == nil || entry.Message.Usage == nil || entry.Message.Model == "" {
			continue
		}
		msg := entry.Message
		timestamp, _ := parseRFC3339Millis(entry.Timestamp)
		usage := msg.Usage
		key := ""
		switch {
		case msg.ID != "" && entry.RequestID != "":
			key = msg.ID + ":" + entry.RequestID
		case msg.ID != "":
			key = msg.ID
		default:
			key = sessionID + ":" + strconv.FormatInt(timestamp, 10) + ":" + msg.Model + ":" + strconv.Itoa(idx) + ":" +
				strconv.FormatInt(deref(usage.InputTokens), 10) + ":" + strconv.FormatInt(deref(usage.OutputTokens), 10) + ":" + strconv.FormatInt(deref(usage.CacheReadInputTokens), 10)
		}
		tokens := provider.TokenBreakdown{
			Input:      max64(deref(usage.InputTokens), 0),
			Output:     max64(deref(usage.OutputTokens), 0),
			CacheRead:  max64(deref(usage.CacheReadInputTokens), 0),
			CacheWrite: max64(deref(usage.CacheCreationInputTokens), 0),
		}
		if tokens.IsEmpty() {
			continue
		}
		providerID := routeHint
		if providerID == "" {
			providerID = strings.ToLower(msg.Provider)
		}
		if providerID == "" {
			providerID = modelid.DetectProviderFromModel(msg.Model)
		}
		session := entry.SessionID
		if session == "" {
			session = sessionID
		}
		m := provider.NewMessage("claude", msg.Model, providerID, session, key, timestamp, tokens)
		m.ParserVersion = claudeParserVersion
		upsertClaudeMessage(&messages, positions, m)
	}
	return messages
}

func claudeRouteHint(entry *claudeEntry) (string, bool) {
	if entry.Type != "user" || entry.Message == nil {
		return "", false
	}
	text, ok := contentText(entry.Message.Content)
	if !ok {
		return "", false
	}
	cleaned := stripANSI(text)
	const marker = "Set model to "
	pos := strings.Index(cleaned, marker)
	if pos < 0 {
		return "", false
	}
	candidate := strings.TrimSpace(cleaned[pos+len(marker):])
	if end := strings.Index(candidate, "</local-command-stdout>"); end >= 0 {
		candidate = candidate[:end]
	}
	if before, _, found := strings.Cut(candidate, " with "); found {
		candidate = before
	}
	candidate = strings.TrimSpace(candidate)
	if candidate == "" {
		return "", false
	}
	segments := []string{}
	for _, segment := range strings.Split(candidate, "/") {
		if s := strings.TrimSpace(segment); s != "" {
			segments = append(segments, s)
		}
	}
	if len(segments) < 3 {
		return "", true
	}
	hint := strings.ToLower(strings.ReplaceAll(segments[len(segments)-3], "_", "-"))
	return hint, true
}

func contentText(raw json.RawMessage) (string, bool) {
	if len(raw) == 0 {
		return "", false
	}
	var text string
	if err := json.Unmarshal(raw, &text); err == nil {
		return text, true
	}
	var blocks []map[string]any
	if err := json.Unmarshal(raw, &blocks); err == nil {
		for _, block := range blocks {
			if t, _ := block["type"].(string); t == "text" {
				if s, ok := block["text"].(string); ok {
					return s, true
				}
			}
		}
	}
	return "", false
}

func stripANSI(text string) string {
	var b strings.Builder
	runes := []rune(text)
	for i := 0; i < len(runes); i++ {
		if runes[i] != '\x1b' {
			b.WriteRune(runes[i])
			continue
		}
		if i+1 < len(runes) && runes[i+1] == '[' {
			i += 2
			for i < len(runes) {
				if runes[i] >= '@' && runes[i] <= '~' {
					break
				}
				i++
			}
		}
	}
	return b.String()
}

func upsertClaudeMessage(messages *[]provider.UnifiedMessage, positions map[string]int, candidate provider.UnifiedMessage) {
	if pos, ok := positions[candidate.MessageKey]; ok {
		existing := (*messages)[pos]
		if candidate.Timestamp > existing.Timestamp ||
			(candidate.Timestamp == existing.Timestamp && candidate.TotalTokens() >= existing.TotalTokens()) {
			(*messages)[pos] = candidate
		}
		return
	}
	positions[candidate.MessageKey] = len(*messages)
	*messages = append(*messages, candidate)
}

func deref(v *int64) int64 {
	if v == nil {
		return 0
	}
	return *v
}

func max64(a, b int64) int64 {
	if a > b {
		return a
	}
	return b
}
