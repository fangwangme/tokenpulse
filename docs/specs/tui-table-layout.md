# TUI Table Layout

This specification defines column width sizing and space allocation rules for tabular views in TokenPulse (specifically the Model tables in Overview and Models views).

## Model and Agent Column Width Allocation

The table row contains `#`, `Model`, `Agent`, followed by metric columns (`Tokens`, `Cost`, `Input`, `Output`, `Cache R`, `Cache W`, `%`, `Msgs`, `Last`, `Trend`) in the Models view (Overview displays `Model`, `Agent`, `Tokens`, `Cost`, `%`).

Metric columns have fixed or breakpoint-controlled minimum widths. After subtracting fixed column widths and spacers from the available row width, the remainder is `available_for_both` (allocated between `Model` and `Agent`).

### 1. Max Content Bounding (Upper Limit)

The `Agent` column displays comma-separated agent names formatted via `format_source_list`.
- `max_agent_content`: the maximum character length of the formatted agent string among the rows to be displayed, with a minimum of 5 (`"Agent"`.len()) and a maximum ceiling of 44.
- `max_model_content`: the maximum character length of model names among the rows to be displayed, clamped between 22 and 45.

When screen width is generous (`available_for_both >= max_model_content + max_agent_content`):
- `agent_width` MUST NOT expand beyond `max_agent_content`. No runaway blank padding spaces are placed after the agent text.
- `model_width` is set to `max_model_content`.
- The following metric columns immediately follow `Agent` with standard single-space column separators. Any extra terminal width remains as clean background space at the right edge of the table.

### 2. Space-Constrained Compression Hierarchy

When `available_for_both < max_model_content + max_agent_content`:
- **Priority**: `Model` column identification is critical to distinguish rows. The `Agent` column is compressed first.
- **First-Agent Preservation**: While `available_for_both` allows, `agent_width` is compressed from `max_agent_content` down towards 11 (the length of the longest single agent name, e.g. `Claude Code` / `Antigravity`), ensuring at least the primary/first agent remains visible.
- **Extreme Compression**: If space is further constrained, `agent_width` shrinks towards a floor of 5, and `model_width` shrinks towards a floor of 18. Both fields and column headers are safely truncated with `…` (or empty when 0-width) without panic, line break, or header shifting.
