package tui

import (
	"sort"

	"github.com/fangwangme/tokenpulse/internal/report"
)

type modelRow struct {
	model  string
	agents string
}

// calculateModelAgentWidths ports tui-table-layout.md: the model name keeps
// its identity, the agent column is compressed first.
func calculateModelAgentWidths(rows []modelRow, available int) (int, int) {
	maxAgent := 5
	maxModel := 22
	for _, row := range rows {
		if len([]rune(row.agents)) > maxAgent {
			maxAgent = len([]rune(row.agents))
		}
		if len([]rune(row.model)) > maxModel {
			maxModel = len([]rune(row.model))
		}
	}
	idealAgent := clampInt(maxAgent, 5, 40)
	idealModel := clampInt(maxModel, 22, 45)
	if available >= idealModel+idealAgent {
		return idealModel, idealAgent
	}
	singleAgent := min(idealAgent, 11)
	switch {
	case available >= idealModel+singleAgent:
		return idealModel, available - idealModel
	case available >= 22+singleAgent:
		return available - singleAgent, singleAgent
	case available >= 22+5:
		return 22, available - 22
	case available >= 18+5:
		return available - 5, 5
	default:
		model := min(18, available)
		return model, max(available-model, 0)
	}
}

// sortedModels returns the report's models in the Models page's sort order.
func (m *model) sortedModels() []report.ModelSummary {
	models := append([]report.ModelSummary(nil), m.report.Models...)
	field := m.sortField
	ascending := m.sortAsc
	sort.SliceStable(models, func(i, j int) bool {
		left, right := models[i], models[j]
		var less, equal bool
		switch field {
		case sortCost:
			less = left.Cost < right.Cost
			equal = left.Cost == right.Cost
		case sortDate:
			less = left.LastUsed < right.LastUsed
			equal = left.LastUsed == right.LastUsed
		default:
			less = left.Tokens < right.Tokens
			equal = left.Tokens == right.Tokens
		}
		if equal {
			return left.Model < right.Model
		}
		if ascending {
			return less
		}
		return !less && !equal
	})
	return models
}

func modelShareTotal(models []report.ModelSummary, field int) float64 {
	var total float64
	for _, entry := range models {
		if field == sortTokens {
			total += float64(entry.Tokens)
		} else {
			total += entry.Cost
		}
	}
	return total
}

func modelShare(entry report.ModelSummary, total float64, field int) float64 {
	if total <= 0 {
		return 0
	}
	value := entry.Cost
	if field == sortTokens {
		value = float64(entry.Tokens)
	}
	return clampFloat(value/total*100, 0, 100)
}

func clampFloat(value, low, high float64) float64 {
	if value < low {
		return low
	}
	if value > high {
		return high
	}
	return value
}

func sortedModelRows(models []report.ModelSummary) []modelRow {
	rows := make([]modelRow, 0, len(models))
	for _, entry := range models {
		rows = append(rows, modelRow{model: entry.Model, agents: displaySourceList(entry.Agents)})
	}
	return rows
}

func normalizeScroll(offset, selected, visible, total int) int {
	if total == 0 || visible == 0 {
		return 0
	}
	offset = clampInt(offset, 0, max(total-visible, 0))
	if selected < offset {
		offset = selected
	} else if selected >= offset+visible {
		offset = selected + 1 - visible
	}
	return clampInt(offset, 0, max(total-visible, 0))
}

func tableRows(total, available int) (visible int, showHint bool) {
	if total > available && available > 1 {
		return available - 1, true
	}
	return available, false
}

func scrollHint(offset, visible, total int) string {
	if total <= visible || visible <= 0 {
		return ""
	}
	up, down := " ", " "
	if offset > 0 {
		up = "↑"
	}
	if offset+visible < total {
		down = "↓"
	}
	return up + down + " scroll " +
		formatIntCommas(int64(offset+1)) + "-" +
		formatIntCommas(int64(min(offset+visible, total))) + " / " +
		formatIntCommas(int64(total))
}
