use chrono::{Datelike, Duration, NaiveDate, Utc, Weekday};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeatmapMetric {
    TotalTokens,
    Cost,
    InputTokens,
    OutputTokens,
    CacheTokens,
    Messages,
    Sessions,
}

impl HeatmapMetric {
    pub fn label(self) -> &'static str {
        match self {
            HeatmapMetric::TotalTokens => "Total Tokens",
            HeatmapMetric::Cost => "Cost",
            HeatmapMetric::InputTokens => "Input Tokens",
            HeatmapMetric::OutputTokens => "Output Tokens",
            HeatmapMetric::CacheTokens => "Cache Tokens",
            HeatmapMetric::Messages => "Messages",
            HeatmapMetric::Sessions => "Sessions",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            HeatmapMetric::TotalTokens => "tokens",
            HeatmapMetric::Cost => "cost",
            HeatmapMetric::InputTokens => "input",
            HeatmapMetric::OutputTokens => "output",
            HeatmapMetric::CacheTokens => "cache",
            HeatmapMetric::Messages => "messages",
            HeatmapMetric::Sessions => "sessions",
        }
    }
}

pub struct YearHeatmap<'a> {
    points: &'a [(NaiveDate, f64)],
    metric: HeatmapMetric,
    palette: [Color; 5],
    empty: Color,
    selected: Option<NaiveDate>,
    range: Option<(NaiveDate, NaiveDate)>,
    selected_color: Color,
}

impl<'a> YearHeatmap<'a> {
    pub fn new(points: &'a [(NaiveDate, f64)], metric: HeatmapMetric) -> Self {
        Self {
            points,
            metric,
            palette: [
                Color::Rgb(18, 78, 65),
                Color::Rgb(25, 107, 85),
                Color::Rgb(33, 138, 104),
                Color::Rgb(54, 181, 135),
                Color::Rgb(102, 237, 173),
            ],
            empty: Color::Rgb(35, 39, 48),
            selected: None,
            range: None,
            selected_color: Color::White,
        }
    }

    pub fn palette(mut self, palette: [Color; 5]) -> Self {
        self.palette = palette;
        self
    }

    pub fn empty(mut self, color: Color) -> Self {
        self.empty = color;
        self
    }

    pub fn selected(mut self, selected: Option<NaiveDate>) -> Self {
        self.selected = selected;
        self
    }

    pub fn range(mut self, start: NaiveDate, end: NaiveDate) -> Self {
        self.range = Some((start, end));
        self
    }

    pub fn range_opt(mut self, range: Option<(NaiveDate, NaiveDate)>) -> Self {
        self.range = range;
        self
    }

    fn value_to_color(&self, value: f64, max_value: f64) -> Color {
        if value <= 0.0 || max_value <= 0.0 {
            return self.empty;
        }

        let ratio = (value / max_value).clamp(0.0, 1.0);
        if ratio < 0.20 {
            self.palette[0]
        } else if ratio < 0.40 {
            self.palette[1]
        } else if ratio < 0.60 {
            self.palette[2]
        } else if ratio < 0.80 {
            self.palette[3]
        } else {
            self.palette[4]
        }
    }
}

impl<'a> Widget for YearHeatmap<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width < 12 || area.height < 8 {
            return;
        }

        let (mut start, end) = self.range.unwrap_or_else(|| {
            let end = Utc::now().date_naive();
            (end - Duration::days(364), end)
        });
        while start.weekday() != Weekday::Sun {
            start -= Duration::days(1);
        }

        let total_days = (end - start).num_days().max(0) as usize + 1;
        let total_weeks = ((total_days + 6) / 7).max(1);
        let grid_x = area.x + 4;
        let grid_y = area.y + 1;
        let grid_width = area.width.saturating_sub(4) as usize;

        if grid_width == 0 {
            return;
        }

        let display_cols = grid_width.min(total_weeks).max(1);
        let mut cell_values: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        let mut month_labels: BTreeMap<usize, String> = BTreeMap::new();
        let mut max_value = 0.0f64;

        let values: BTreeMap<NaiveDate, f64> = self.points.iter().copied().collect();
        let mut cursor = start;
        let mut day_idx = 0usize;
        let mut last_month = None;

        while cursor <= end {
            let week_idx = day_idx / 7;
            let display_col = if total_weeks <= display_cols {
                week_idx
            } else {
                week_idx * display_cols / total_weeks
            };
            let row = cursor.weekday().num_days_from_sunday() as usize;

            let value = values.get(&cursor).copied().unwrap_or(0.0);
            max_value = max_value.max(value);

            let key = (display_col.min(display_cols.saturating_sub(1)), row);
            cell_values
                .entry(key)
                .and_modify(|existing| *existing = existing.max(value))
                .or_insert(value);

            let month = cursor.month();
            if last_month != Some(month) {
                month_labels
                    .entry(display_col)
                    .or_insert_with(|| format!("{}", cursor.format("%b")));
                last_month = Some(month);
            }

            cursor += Duration::days(1);
            day_idx += 1;
        }

        let weekday_labels = ["S", "M", "T", "W", "T", "F", "S"];
        for (row, label) in weekday_labels.iter().enumerate() {
            let y = grid_y + row as u16;
            if y < area.y + area.height {
                buf.set_string(area.x, y, *label, Style::default().fg(Color::Gray));
            }
        }

        for (col, label) in month_labels {
            let x = grid_x + col as u16;
            if x < area.x + area.width {
                buf.set_string(x, area.y, label, Style::default().fg(Color::Gray));
            }
        }

        for col in 0..display_cols {
            for row in 0..7 {
                let x = grid_x + col as u16;
                let y = grid_y + row as u16;
                if x >= area.x + area.width || y >= area.y + area.height {
                    continue;
                }

                let value = cell_values.get(&(col, row)).copied().unwrap_or(0.0);
                let color = self.value_to_color(value, max_value);
                let date = start + Duration::days((col * 7 + row) as i64);
                let is_selected = self.selected == Some(date);
                let symbol = if is_selected {
                    "◆"
                } else if value <= 0.0 {
                    "·"
                } else {
                    "■"
                };
                let style = if is_selected {
                    Style::default().fg(self.selected_color).bg(color)
                } else {
                    Style::default().fg(color)
                };
                buf.set_string(x, y, symbol, style);
            }
        }

        let metric_text = format!(
            "{}  {} → {}  max {:.2}",
            self.metric.label(),
            start.format("%Y-%m-%d"),
            end.format("%Y-%m-%d"),
            max_value
        );
        let footer_y = area.y + area.height.saturating_sub(1);
        if footer_y > area.y + 7 {
            buf.set_string(
                area.x,
                footer_y,
                metric_text,
                Style::default().fg(Color::Gray),
            );
        }
    }
}
