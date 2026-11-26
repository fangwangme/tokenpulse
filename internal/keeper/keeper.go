// Package keeper drives agent CLIs with tiny prompts so rate-limit windows
// start when the user wants them to.
package keeper

import (
	"bytes"
	"database/sql"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"time"

	"github.com/fangwangme/tokenpulse/internal/config"
	"github.com/fangwangme/tokenpulse/internal/provider"
)

// Agents the Keeper drives, in card order.
var Agents = []string{"claude", "codex", "antigravity"}

type TriggerType string

const (
	TriggerDaily  TriggerType = "daily"
	TriggerWeekly TriggerType = "weekly"
	TriggerManual TriggerType = "manual"
)

func (t TriggerType) Label() string {
	switch t {
	case TriggerDaily:
		return "5h Daily"
	case TriggerWeekly:
		return "Weekly Sync"
	default:
		return "Manual Ping"
	}
}

func (t TriggerType) String() string { return string(t) }

func TriggerFromString(value string) TriggerType {
	switch value {
	case "daily":
		return TriggerDaily
	case "weekly":
		return TriggerWeekly
	default:
		return TriggerManual
	}
}

type ExecutionRecord struct {
	Agent           string      `json:"agent"`
	Trigger         TriggerType `json:"trigger"`
	Model           string      `json:"model"`
	Prompt          string      `json:"prompt"`
	CommandExecuted string      `json:"-"`
	Timestamp       time.Time   `json:"timestamp"`
	DurationMs      int64       `json:"durationMs"`
	Success         bool        `json:"success"`
	ExitCode        *int        `json:"exitCode,omitempty"`
	OutputSnippet   string      `json:"outputSnippet"`
}

const (
	outputSnippetMaxChars = 300
	pingTimeout           = 120 * time.Second
	pingKillGrace         = 5 * time.Second
	dailyCatchUpMinutes   = 120
	unstartedMargin       = 4 * time.Hour
	weeklySuccessCooldown = 5 * time.Hour
	weeklyFailureCooldown = time.Hour
)

// FormatCommand substitutes {model} and {prompt}, escaping the values.
func FormatCommand(template, model, prompt string) string {
	return strings.ReplaceAll(strings.ReplaceAll(template, "{model}", escapeShellValue(model)), "{prompt}", escapeShellValue(prompt))
}

func escapeShellValue(value string) string {
	var b strings.Builder
	for _, r := range value {
		if r < 0x20 || r == 0x7f {
			continue
		}
		switch r {
		case '\\', '"', '$', '`':
			b.WriteByte('\\')
		}
		b.WriteRune(r)
	}
	return b.String()
}

// SanitizeOutputSnippet collapses CLI output into one safe line.
func SanitizeOutputSnippet(raw string) string {
	withoutANSI := stripANSI(raw)
	var b strings.Builder
	for _, r := range withoutANSI {
		if r < 0x20 || r == 0x7f {
			b.WriteByte(' ')
		} else {
			b.WriteRune(r)
		}
	}
	collapsed := strings.Join(strings.Fields(b.String()), " ")
	runes := []rune(collapsed)
	if len(runes) > outputSnippetMaxChars {
		return string(runes[:outputSnippetMaxChars]) + "..."
	}
	return collapsed
}

func stripANSI(raw string) string {
	var out strings.Builder
	runes := []rune(raw)
	for i := 0; i < len(runes); i++ {
		if runes[i] != 0x1b {
			out.WriteRune(runes[i])
			continue
		}
		if i+1 >= len(runes) {
			break
		}
		switch runes[i+1] {
		case '[':
			i += 2
			for i < len(runes) {
				if runes[i] >= 0x40 && runes[i] <= 0x7e {
					break
				}
				i++
			}
		case ']':
			i += 2
			for i < len(runes) {
				if runes[i] == 0x07 {
					break
				}
				if runes[i] == 0x1b && i+1 < len(runes) && runes[i+1] == '\\' {
					i++
					break
				}
				i++
			}
		default:
			i++
		}
	}
	return out.String()
}

func parseClock(value string) (hour, minute int, ok bool) {
	parts := strings.Split(strings.TrimSpace(value), ":")
	if len(parts) != 2 {
		return 0, 0, false
	}
	if _, err := fmt.Sscanf(parts[0], "%d", &hour); err != nil {
		return 0, 0, false
	}
	if _, err := fmt.Sscanf(parts[1], "%d", &minute); err != nil {
		return 0, 0, false
	}
	if hour < 0 || hour > 23 || minute < 0 || minute > 59 {
		return 0, 0, false
	}
	return hour, minute, true
}

// ShouldTriggerDaily says whether the 5h wakeup may fire now.
func ShouldTriggerDaily(dailyTime string, lastTriggered *time.Time, now time.Time) bool {
	hour, minute, ok := parseClock(dailyTime)
	if !ok {
		return false
	}
	if lastTriggered != nil {
		last := lastTriggered.In(now.Location())
		if !last.Before(time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location())) {
			return false
		}
	}
	target := time.Date(now.Year(), now.Month(), now.Day(), hour, minute, 0, 0, now.Location())
	elapsed := now.Sub(target)
	return elapsed >= 0 && elapsed <= dailyCatchUpMinutes*time.Minute
}

// NextDailyTrigger is the next wakeup instant.
func NextDailyTrigger(dailyTime string, lastTriggered *time.Time, now time.Time) *time.Time {
	hour, minute, ok := parseClock(dailyTime)
	if !ok {
		return nil
	}
	target := time.Date(now.Year(), now.Month(), now.Day(), hour, minute, 0, 0, now.Location())
	triggeredToday := lastTriggered != nil && !lastTriggered.In(now.Location()).Before(time.Date(now.Year(), now.Month(), now.Day(), 0, 0, 0, 0, now.Location()))
	if triggeredToday || !now.Before(target) {
		target = target.AddDate(0, 0, 1)
	}
	return &target
}

// IsWeeklyCycleWindow reports a major quota cycle, not a 5h session.
func IsWeeklyCycleWindow(window provider.RateWindow) bool {
	if window.PeriodMs != nil {
		return *window.PeriodMs >= 6*24*60*60*1000
	}
	lower := strings.ToLower(window.Label)
	return strings.Contains(lower, "7d") || strings.Contains(lower, "7 d") ||
		strings.Contains(lower, "7-day") || strings.Contains(lower, "week") ||
		strings.Contains(lower, "month") || strings.Contains(lower, "30d")
}

func weeklyWindowFor(snapshot *provider.QuotaSnapshot, group *string) *provider.RateWindow {
	for i := range snapshot.Windows {
		window := &snapshot.Windows[i]
		if !IsWeeklyCycleWindow(*window) {
			continue
		}
		if group == nil {
			if window.Family == "" {
				return window
			}
			continue
		}
		if window.Family == *group {
			return window
		}
	}
	return nil
}

func isUnstartedWeekly(window *provider.RateWindow, now time.Time) bool {
	if window.UsedPercent > 0 {
		return false
	}
	if window.ResetsAt == nil {
		return true
	}
	period := 7 * 24 * time.Hour
	if window.PeriodMs != nil {
		period = time.Duration(*window.PeriodMs) * time.Millisecond
	}
	return window.ResetsAt.Sub(now) > period-unstartedMargin
}

// ShouldTriggerWeekly is the fallback trigger after a reset.
func ShouldTriggerWeekly(resetsAt *time.Time, lastTriggered *time.Time, now time.Time) bool {
	if resetsAt == nil {
		return false
	}
	threshold := resetsAt.Add(time.Minute)
	if now.Before(threshold) {
		return false
	}
	if lastTriggered == nil {
		return true
	}
	if now.Sub(*lastTriggered) < 12*time.Hour {
		return false
	}
	return lastTriggered.Before(*resetsAt)
}

// NextWeeklyTrigger is the expected reset time while it has not fired.
func NextWeeklyTrigger(resetsAt *time.Time, lastTriggered *time.Time, now time.Time) *time.Time {
	if resetsAt == nil {
		return nil
	}
	if lastTriggered != nil && !lastTriggered.Before(*resetsAt) && !now.Before(*resetsAt) {
		return nil
	}
	return resetsAt
}

type WeeklyTarget struct {
	Key   string `json:"key"`
	Group string `json:"group,omitempty"`
}

func (t WeeklyTarget) IsPrimary(agent string) bool { return t.Key == agent }

// WeeklyGroups lists distinct families among weekly windows, in snapshot order.
func WeeklyGroups(snapshot *provider.QuotaSnapshot) []string {
	if snapshot == nil {
		return nil
	}
	var groups []string
	for _, window := range snapshot.Windows {
		if !IsWeeklyCycleWindow(window) || window.Family == "" {
			continue
		}
		found := false
		for _, group := range groups {
			if group == window.Family {
				found = true
			}
		}
		if !found {
			groups = append(groups, window.Family)
		}
	}
	return groups
}

// WeeklyTargets builds an agent's targets.
func WeeklyTargets(agent string, snapshot *provider.QuotaSnapshot, allGroups bool) []WeeklyTarget {
	var groups []string
	if snapshot != nil && agent == "antigravity" {
		groups = WeeklyGroups(snapshot)
	}
	if len(groups) == 0 {
		return []WeeklyTarget{{Key: agent}}
	}
	count := 1
	if allGroups {
		count = len(groups)
	}
	var targets []WeeklyTarget
	for i, group := range groups {
		if i >= count {
			break
		}
		key := agent
		if i > 0 {
			key = agent + ":" + group
		}
		targets = append(targets, WeeklyTarget{Key: key, Group: group})
	}
	return targets
}

// IdleWeeklyTargets returns the targets whose window may be pinged now.
func IdleWeeklyTargets(agent string, snapshot *provider.QuotaSnapshot, allGroups bool,
	weeklyTriggered map[string]time.Time, weeklyFailed map[string]bool, now time.Time) []WeeklyTarget {
	var out []WeeklyTarget
	for _, target := range WeeklyTargets(agent, snapshot, allGroups) {
		last, ok := weeklyTriggered[target.Key]
		if ok {
			cooldown := weeklySuccessCooldown
			if weeklyFailed[target.Key] {
				cooldown = weeklyFailureCooldown
			}
			if now.Sub(last) < cooldown {
				continue
			}
		}
		group := target.Group
		var groupPtr *string
		if group != "" {
			groupPtr = &group
		}
		window := weeklyWindowFor(snapshot, groupPtr)
		if window == nil || !isUnstartedWeekly(window, now) {
			continue
		}
		out = append(out, target)
	}
	return out
}

func findSnapshotForAgent(snapshots []provider.QuotaSnapshot, agent string) *provider.QuotaSnapshot {
	for i := range snapshots {
		if strings.EqualFold(snapshots[i].Provider, agent) {
			return &snapshots[i]
		}
	}
	for i := range snapshots {
		providerID := strings.ToLower(snapshots[i].Provider)
		switch agent {
		case "antigravity":
			if providerID == "google" || providerID == "antigravity" {
				return &snapshots[i]
			}
		case "claude":
			if providerID == "anthropic" || providerID == "claude" {
				return &snapshots[i]
			}
		case "codex":
			if providerID == "openai" || providerID == "codex" {
				return &snapshots[i]
			}
		}
	}
	return nil
}

// ExtractWeeklyResetTime returns the followed window's reset time.
func ExtractWeeklyResetTime(snapshot *provider.QuotaSnapshot, group *string) *time.Time {
	if snapshot == nil {
		return nil
	}
	window := weeklyWindowFor(snapshot, group)
	if window == nil {
		return nil
	}
	return window.ResetsAt
}

// ── trigger state ──

type TriggerState struct {
	DailyTriggered  map[string]string    `json:"daily_triggered"`
	WeeklyTriggered map[string]time.Time `json:"weekly_triggered"`
	WeeklyFailed    map[string]bool      `json:"weekly_failed"`
}

func LoadState(path string) TriggerState {
	state := TriggerState{
		DailyTriggered: map[string]string{}, WeeklyTriggered: map[string]time.Time{}, WeeklyFailed: map[string]bool{},
	}
	content, err := os.ReadFile(path)
	if err != nil {
		return state
	}
	var raw struct {
		DailyTriggered  map[string]string    `json:"daily_triggered"`
		WeeklyTriggered map[string]time.Time `json:"weekly_triggered"`
		WeeklyFailed    []string             `json:"weekly_failed"`
	}
	if json.Unmarshal(content, &raw) != nil {
		return state
	}
	if raw.DailyTriggered != nil {
		state.DailyTriggered = raw.DailyTriggered
	}
	if raw.WeeklyTriggered != nil {
		state.WeeklyTriggered = raw.WeeklyTriggered
	}
	for _, key := range raw.WeeklyFailed {
		state.WeeklyFailed[key] = true
	}
	return state
}

func (s TriggerState) Save(path string) {
	payload := struct {
		DailyTriggered  map[string]string    `json:"daily_triggered"`
		WeeklyTriggered map[string]time.Time `json:"weekly_triggered"`
		WeeklyFailed    []string             `json:"weekly_failed"`
	}{s.DailyTriggered, s.WeeklyTriggered, nil}
	for key := range s.WeeklyFailed {
		payload.WeeklyFailed = append(payload.WeeklyFailed, key)
	}
	sort.Strings(payload.WeeklyFailed)
	data, err := json.MarshalIndent(payload, "", "  ")
	if err != nil {
		return
	}
	_ = os.MkdirAll(filepath.Dir(path), 0o755)
	_ = os.WriteFile(path, data, 0o644)
}

func (s TriggerState) DailyDate(agent string, location *time.Location) *time.Time {
	value, ok := s.DailyTriggered[agent]
	if !ok {
		return nil
	}
	parsed, err := time.ParseInLocation("2006-01-02", value, location)
	if err != nil {
		return nil
	}
	return &parsed
}

// ── execution ──

type Request struct {
	Agent   string
	Key     string
	Trigger TriggerType
	Config  config.AgentKeeperConfig
	Model   string
	Group   string
	Groups  []string
}

// Execute runs one ping and returns its record.
func (e *Engine) Execute(request Request) ExecutionRecord {
	start := time.Now()
	model := request.Model
	if !strings.Contains(request.Config.Command, "{model}") {
		model = "custom"
	} else if model == "" {
		var err error
		if request.Group != "" {
			model, err = ResolveAntigravityGroupModel(request.Group, request.Groups)
		} else {
			directory := e.DataDir
			if e.History != nil && e.History.Path != "" {
				directory = filepath.Dir(e.History.Path)
			}
			model, err = ResolveCheapestModel(request.Agent, directory)
		}
		if err != nil {
			command := FormatCommand(request.Config.Command, "unresolved", request.Config.Prompt)
			return ExecutionRecord{
				Agent: request.Agent, Trigger: request.Trigger, Model: "unresolved",
				Prompt: request.Config.Prompt, CommandExecuted: command, Timestamp: start,
				Success: false, OutputSnippet: SanitizeOutputSnippet("Model resolution failed: " + err.Error()),
			}
		}
	}
	command := FormatCommand(request.Config.Command, model, request.Config.Prompt)
	success, exitCode, snippet := runCommand(command, pingTimeout)
	record := ExecutionRecord{
		Agent: request.Agent, Trigger: request.Trigger, Model: model,
		Prompt: request.Config.Prompt, CommandExecuted: command, Timestamp: start,
		DurationMs: time.Since(start).Milliseconds(), Success: success,
		ExitCode: exitCode, OutputSnippet: snippet,
	}
	if e.History != nil {
		_ = e.History.Record(record)
	}
	return record
}

func runCommand(command string, timeout time.Duration) (bool, *int, string) {
	cmd := exec.Command("sh", "-c", command)
	cmd.Stdin = nil
	var stdout, stderr bytes.Buffer
	cmd.Stdout = &stdout
	cmd.Stderr = &stderr
	if err := cmd.Start(); err != nil {
		return false, nil, SanitizeOutputSnippet("Execution failed: " + err.Error())
	}
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	select {
	case err := <-done:
		code := cmd.ProcessState.ExitCode()
		combined := strings.TrimSpace(stdout.String())
		if strings.TrimSpace(stderr.String()) != "" {
			combined = strings.TrimSpace(combined + " " + stderr.String())
		}
		snippet := SanitizeOutputSnippet(combined)
		if snippet == "" {
			snippet = "Command executed (empty output)"
		}
		return err == nil, &code, snippet
	case <-time.After(timeout):
		_ = cmd.Process.Signal(os.Interrupt)
		select {
		case <-done:
		case <-time.After(pingKillGrace):
			_ = cmd.Process.Kill()
		}
		return false, nil, fmt.Sprintf("Execution timed out after %ds", int(timeout.Seconds()))
	}
}

// Engine schedules pings and keeps the last-fired state.
type Engine struct {
	Config    config.KeeperConfig
	StatePath string
	DataDir   string
	State     TriggerState
	History   *History

	mu    sync.Mutex
	pings map[string]bool
}

func NewEngine(cfg config.KeeperConfig, statePath string) *Engine {
	return &Engine{
		Config: cfg, StatePath: statePath, DataDir: filepath.Dir(statePath), State: LoadState(statePath),
		History: NewHistory(filepath.Join(filepath.Dir(statePath), "tokenpulse.db")),
		pings:   map[string]bool{},
	}
}

func (e *Engine) snapshotLocked() TriggerState {
	return TriggerState{
		DailyTriggered:  cloneStringMap(e.State.DailyTriggered),
		WeeklyTriggered: cloneTimeMap(e.State.WeeklyTriggered),
		WeeklyFailed:    cloneBoolMap(e.State.WeeklyFailed),
	}
}

func cloneStringMap(source map[string]string) map[string]string {
	if source == nil {
		return map[string]string{}
	}
	out := make(map[string]string, len(source))
	for key, value := range source {
		out[key] = value
	}
	return out
}

func cloneTimeMap(source map[string]time.Time) map[string]time.Time {
	if source == nil {
		return map[string]time.Time{}
	}
	out := make(map[string]time.Time, len(source))
	for key, value := range source {
		out[key] = value
	}
	return out
}

func cloneBoolMap(source map[string]bool) map[string]bool {
	if source == nil {
		return map[string]bool{}
	}
	out := make(map[string]bool, len(source))
	for key, value := range source {
		out[key] = value
	}
	return out
}

func (e *Engine) persistLocked() { e.snapshotLocked().Save(e.StatePath) }

// Persist saves a consistent snapshot of the trigger state.
func (e *Engine) Persist() {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.persistLocked()
}

// StateSnapshot returns a copy of the trigger maps for readers outside the
// engine; the engine keeps mutating the originals.
func (e *Engine) StateSnapshot() TriggerState {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.snapshotLocked()
}

// WeeklyTriggered reports one weekly target's last fire time.
func (e *Engine) WeeklyTriggered(key string) (time.Time, bool) {
	e.mu.Lock()
	defer e.mu.Unlock()
	value, ok := e.State.WeeklyTriggered[key]
	return value, ok
}

// UpdateConfig applies a config.toml reload without recreating the engine.
func (e *Engine) UpdateConfig(cfg config.KeeperConfig) {
	e.mu.Lock()
	e.Config = cfg
	e.mu.Unlock()
}

func (e *Engine) InFlight(key string) bool {
	e.mu.Lock()
	defer e.mu.Unlock()
	return e.pings[key]
}

func (e *Engine) inFlightLocked(key string) bool { return e.pings[key] }

func (e *Engine) markLocked(key string, value bool) {
	if value {
		e.pings[key] = true
	} else {
		delete(e.pings, key)
	}
}

// CheckScheduled evaluates the daily wakeup and the weekly reset fallback.
func (e *Engine) CheckScheduled(snapshots []provider.QuotaSnapshot, now time.Time) []Request {
	e.mu.Lock()
	defer e.mu.Unlock()
	if !e.Config.Enabled {
		return nil
	}
	var requests []Request
	for _, agent := range Agents {
		cfg, ok := e.Config.Agents[agent]
		if !ok {
			cfg = config.DefaultKeeperAgents()[agent]
		}
		if cfg.SessionKeeperEnabled && !e.inFlightLocked(agent) {
			last := e.State.DailyDate(agent, now.Location())
			if ShouldTriggerDaily(cfg.DailyWakeupTime, last, now) {
				k := now.Format("2006-01-02")
				e.State.DailyTriggered[agent] = k
				e.markLocked(agent, true)
				e.persistLocked()
				requests = append(requests, Request{Agent: agent, Key: agent, Trigger: TriggerDaily, Config: cfg})
			}
		}
		if !cfg.WeeklyKeeperEnabled {
			continue
		}
		snapshot := findSnapshotForAgent(snapshots, agent)
		var groups []string
		if snapshot != nil {
			groups = WeeklyGroups(snapshot)
		}
		for _, target := range WeeklyTargets(agent, snapshot, cfg.WeeklyAllGroups) {
			if e.inFlightLocked(target.Key) {
				continue
			}
			var group *string
			if target.Group != "" {
				group = &target.Group
			}
			resetsAt := ExtractWeeklyResetTime(snapshot, group)
			last, ok := e.State.WeeklyTriggered[target.Key]
			var lastPtr *time.Time
			if ok {
				lastPtr = &last
			}
			if ShouldTriggerWeekly(resetsAt, lastPtr, now) {
				e.State.WeeklyTriggered[target.Key] = now
				e.markLocked(target.Key, true)
				e.persistLocked()
				requests = append(requests, Request{
					Agent: agent, Key: target.Key, Trigger: TriggerWeekly, Config: cfg,
					Group: target.Group, Groups: groups,
				})
			}
		}
	}
	return requests
}

// CheckQuota evaluates the unstarted-window rule after a quota refresh.
func (e *Engine) CheckQuota(snapshots []provider.QuotaSnapshot, now time.Time) []Request {
	e.mu.Lock()
	defer e.mu.Unlock()
	if !e.Config.Enabled {
		return nil
	}
	var requests []Request
	for _, agent := range Agents {
		cfg, ok := e.Config.Agents[agent]
		if !ok {
			cfg = config.DefaultKeeperAgents()[agent]
		}
		if !cfg.WeeklyKeeperEnabled {
			continue
		}
		snapshot := findSnapshotForAgent(snapshots, agent)
		if snapshot == nil {
			continue
		}
		groups := WeeklyGroups(snapshot)
		for _, target := range IdleWeeklyTargets(agent, snapshot, cfg.WeeklyAllGroups,
			e.State.WeeklyTriggered, e.State.WeeklyFailed, now) {
			if e.inFlightLocked(target.Key) {
				continue
			}
			e.State.WeeklyTriggered[target.Key] = now
			e.State.WeeklyFailed[target.Key] = false
			e.markLocked(target.Key, true)
			e.persistLocked()
			requests = append(requests, Request{
				Agent: agent, Key: target.Key, Trigger: TriggerWeekly, Config: cfg,
				Group: target.Group, Groups: groups,
			})
		}
	}
	return requests
}

// Finish records a ping's outcome and clears its in-flight lock.
func (e *Engine) Finish(request Request, record ExecutionRecord) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.markLocked(request.Key, false)
	if request.Trigger == TriggerWeekly {
		e.State.WeeklyFailed[request.Key] = !record.Success
		if !record.Success {
			// A failed ping retries after the shorter cooldown.
			if last, ok := e.State.WeeklyTriggered[request.Key]; ok {
				e.State.WeeklyTriggered[request.Key] = last
			}
		}
		e.persistLocked()
	}
}

// Manual builds a manual request for one agent and optional group.
func (e *Engine) Manual(agent, group string) (Request, error) {
	e.mu.Lock()
	defer e.mu.Unlock()
	cfg, ok := e.Config.Agents[agent]
	if !ok {
		return Request{}, errors.New("agent is not driven by the Keeper")
	}
	key := agent
	if group != "" {
		if agent != "antigravity" {
			return Request{}, errors.New("only Antigravity has quota groups")
		}
		key = agent + ":" + group
	}
	if e.inFlightLocked(key) {
		return Request{}, errors.New("a ping for this key is already running")
	}
	e.markLocked(key, true)
	return Request{Agent: agent, Key: key, Trigger: TriggerManual, Config: cfg, Group: group}, nil
}

// AvailableGroups lists the Antigravity groups from a snapshot.
func AvailableGroups(snapshot *provider.QuotaSnapshot) []string {
	if snapshot == nil {
		return nil
	}
	return WeeklyGroups(snapshot)
}

// ── history ──

type History struct {
	Path string
}

func NewHistory(path string) *History { return &History{Path: path} }

func (h *History) Record(record ExecutionRecord) error {
	db, err := openHistory(h.Path)
	if err != nil {
		return err
	}
	defer db.Close()
	_, err = db.Exec(`INSERT INTO keeper_executions (
        agent, trigger_type, executed_at, model, prompt, command, duration_ms, success, exit_code, output_snippet
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
		record.Agent, record.Trigger.String(), record.Timestamp.Format(time.RFC3339Nano),
		record.Model, record.Prompt, record.CommandExecuted, record.DurationMs,
		boolInt(record.Success), record.ExitCode, record.OutputSnippet)
	return err
}

func (h *History) Recent(limit int) []ExecutionRecord {
	db, err := openHistory(h.Path)
	if err != nil {
		return nil
	}
	defer db.Close()
	rows, err := db.Query(`SELECT agent, trigger_type, executed_at, model, prompt, command,
        duration_ms, success, exit_code, output_snippet FROM keeper_executions
        ORDER BY executed_at DESC, id DESC LIMIT ?1`, limit)
	if err != nil {
		return nil
	}
	defer rows.Close()
	var records []ExecutionRecord
	for rows.Next() {
		var record ExecutionRecord
		var executed string
		var success int
		var exit sql.NullInt64
		if err := rows.Scan(&record.Agent, &record.Trigger, &executed, &record.Model, &record.Prompt,
			&record.CommandExecuted, &record.DurationMs, &success, &exit, &record.OutputSnippet); err != nil {
			continue
		}
		record.Timestamp, _ = time.Parse(time.RFC3339Nano, executed)
		record.Success = success != 0
		if exit.Valid {
			code := int(exit.Int64)
			record.ExitCode = &code
		}
		records = append(records, record)
	}
	for i, j := 0, len(records)-1; i < j; i, j = i+1, j-1 {
		records[i], records[j] = records[j], records[i]
	}
	return records
}

func boolInt(value bool) int {
	if value {
		return 1
	}
	return 0
}
