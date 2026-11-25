// Package summary builds the CLI's text and JSON usage summary from ledger
// rows, preserving the 0.6.x output shape.
package summary

import (
	"sort"
	"time"
)

type Day struct {
	Date             string  `json:"date"`
	TotalTokens      int64   `json:"total_tokens"`
	TotalCostUSD     float64 `json:"total_cost_usd"`
	InputTokens      int64   `json:"input_tokens"`
	OutputTokens     int64   `json:"output_tokens"`
	CacheReadTokens  int64   `json:"cache_read_tokens"`
	CacheWriteTokens int64   `json:"cache_write_tokens"`
	ReasoningTokens  int64   `json:"reasoning_tokens"`
	MessageCount     int64   `json:"message_count"`
	SessionCount     int64   `json:"session_count"`
	IntensityTokens  uint8   `json:"intensity_tokens"`
	IntensityCost    uint8   `json:"intensity_cost"`
}

type Rollup struct {
	Label            string  `json:"label"`
	StartDate        string  `json:"start_date"`
	EndDate          string  `json:"end_date"`
	TotalTokens      int64   `json:"total_tokens"`
	TotalCostUSD     float64 `json:"total_cost_usd"`
	InputTokens      int64   `json:"input_tokens"`
	OutputTokens     int64   `json:"output_tokens"`
	CacheReadTokens  int64   `json:"cache_read_tokens"`
	CacheWriteTokens int64   `json:"cache_write_tokens"`
	ReasoningTokens  int64   `json:"reasoning_tokens"`
	MessageCount     int64   `json:"message_count"`
	SessionCount     int64   `json:"session_count"`
	ActiveDays       int64   `json:"active_days"`
}

type Provider struct {
	Provider     string  `json:"provider"`
	Cost         float64 `json:"cost"`
	Tokens       int64   `json:"tokens"`
	MessageCount int     `json:"message_count"`
	SessionCount int     `json:"session_count"`
	Percent      float64 `json:"percent"`
}

type Model struct {
	Model            string  `json:"model"`
	Provider         string  `json:"provider"`
	Source           string  `json:"source"`
	Cost             float64 `json:"cost"`
	Tokens           int64   `json:"tokens"`
	InputTokens      int64   `json:"input_tokens"`
	OutputTokens     int64   `json:"output_tokens"`
	CacheTokens      int64   `json:"cache_tokens"`
	CacheReadTokens  int64   `json:"cache_read_tokens"`
	CacheWriteTokens int64   `json:"cache_write_tokens"`
	MessageCount     int     `json:"message_count"`
	SessionCount     int     `json:"session_count"`
	Percent          float64 `json:"percent"`
}

type Summary struct {
	TotalCost      float64    `json:"total_cost"`
	TotalTokens    int64      `json:"total_tokens"`
	MessageCount   int        `json:"message_count"`
	SessionCount   int        `json:"session_count"`
	ActiveDays     int        `json:"active_days"`
	AvgDailyCost   float64    `json:"avg_daily_cost"`
	MaxDailyCost   float64    `json:"max_daily_cost"`
	AvgDailyTokens float64    `json:"avg_daily_tokens"`
	MaxDailyTokens int64      `json:"max_daily_tokens"`
	Daily          []Day      `json:"daily"`
	Weekly         []Rollup   `json:"weekly"`
	Monthly        []Rollup   `json:"monthly"`
	ByProvider     []Provider `json:"by_provider"`
	ByModel        []Model    `json:"by_model"`
}

// Build assembles the summary and recomputes shares and intensity buckets.
func Build(daily []Day, providers []Provider, models []Model, messages, sessions int) Summary {
	sort.SliceStable(daily, func(i, j int) bool { return daily[i].Date < daily[j].Date })
	applyIntensity(daily)
	var totalCost float64
	var totalTokens int64
	for _, day := range daily {
		totalCost += day.TotalCostUSD
		totalTokens += day.TotalTokens
	}
	activeDays := len(daily)
	avgCost := 0.0
	avgTokens := 0.0
	if activeDays > 0 {
		avgCost = totalCost / float64(activeDays)
		avgTokens = float64(totalTokens) / float64(activeDays)
	}
	var maxCost float64
	var maxTokens int64
	for _, day := range daily {
		if day.TotalCostUSD > maxCost {
			maxCost = day.TotalCostUSD
		}
		if day.TotalTokens > maxTokens {
			maxTokens = day.TotalTokens
		}
	}
	sort.SliceStable(providers, func(i, j int) bool { return providers[i].Tokens > providers[j].Tokens })
	for i := range providers {
		providers[i].Percent = percent(providers[i].Tokens, totalTokens)
	}
	sort.SliceStable(models, func(i, j int) bool { return models[i].Tokens > models[j].Tokens })
	for i := range models {
		models[i].Percent = percent(models[i].Tokens, totalTokens)
	}
	return Summary{
		TotalCost: totalCost, TotalTokens: totalTokens, MessageCount: messages, SessionCount: sessions,
		ActiveDays: activeDays, AvgDailyCost: avgCost, MaxDailyCost: maxCost,
		AvgDailyTokens: avgTokens, MaxDailyTokens: maxTokens,
		Daily: daily, Weekly: weeklyRollups(daily), Monthly: monthlyRollups(daily),
		ByProvider: providers, ByModel: models,
	}
}

func percent(part, total int64) float64 {
	if total <= 0 {
		return 0
	}
	return float64(part) / float64(total) * 100
}

func applyIntensity(daily []Day) {
	var maxTokens int64
	var maxCost float64
	for _, day := range daily {
		if day.TotalTokens > maxTokens {
			maxTokens = day.TotalTokens
		}
		if day.TotalCostUSD > maxCost {
			maxCost = day.TotalCostUSD
		}
	}
	for i := range daily {
		daily[i].IntensityTokens = bucket(float64(daily[i].TotalTokens), float64(maxTokens))
		daily[i].IntensityCost = bucket(daily[i].TotalCostUSD, maxCost)
	}
}

func bucket(value, max float64) uint8 {
	if value <= 0 || max <= 0 {
		return 0
	}
	b := uint8(value / max * 4)
	if float64(b) < value/max*4 {
		b++
	}
	if b < 1 {
		b = 1
	}
	if b > 4 {
		b = 4
	}
	return b
}

func weeklyRollups(daily []Day) []Rollup {
	return rollups(daily, func(date time.Time) (string, string, string, bool) {
		offset := (int(date.Weekday()) + 6) % 7
		start := date.AddDate(0, 0, -offset)
		end := start.AddDate(0, 0, 6)
		return start.Format("2006-01-02") + ".." + end.Format("2006-01-02"),
			start.Format("2006-01-02"), end.Format("2006-01-02"), true
	})
}

func monthlyRollups(daily []Day) []Rollup {
	return rollups(daily, func(date time.Time) (string, string, string, bool) {
		start := time.Date(date.Year(), date.Month(), 1, 0, 0, 0, 0, time.Local)
		end := start.AddDate(0, 1, -1)
		return start.Format("2006-01"), start.Format("2006-01-02"), end.Format("2006-01-02"), true
	})
}

func rollups(daily []Day, group func(time.Time) (string, string, string, bool)) []Rollup {
	byLabel := map[string]*Rollup{}
	for _, day := range daily {
		date, err := time.ParseInLocation("2006-01-02", day.Date, time.Local)
		if err != nil {
			continue
		}
		label, start, end, ok := group(date)
		if !ok {
			continue
		}
		entry, exists := byLabel[label]
		if !exists {
			entry = &Rollup{Label: label, StartDate: start, EndDate: end}
			byLabel[label] = entry
		}
		entry.TotalTokens += day.TotalTokens
		entry.TotalCostUSD += day.TotalCostUSD
		entry.InputTokens += day.InputTokens
		entry.OutputTokens += day.OutputTokens
		entry.CacheReadTokens += day.CacheReadTokens
		entry.CacheWriteTokens += day.CacheWriteTokens
		entry.ReasoningTokens += day.ReasoningTokens
		entry.MessageCount += day.MessageCount
		entry.SessionCount += day.SessionCount
		entry.ActiveDays++
	}
	labels := make([]string, 0, len(byLabel))
	for label := range byLabel {
		labels = append(labels, label)
	}
	sort.Strings(labels)
	out := make([]Rollup, 0, len(labels))
	for _, label := range labels {
		out = append(out, *byLabel[label])
	}
	return out
}
