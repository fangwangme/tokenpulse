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

    #[allow(dead_code)]
    pub fn range(mut self, start: NaiveDate, end: NaiveDate) -> Self {
        self.range = Some((start, end));
        self
    }

    pub fn range_opt(mut self, range: Option<(NaiveDate, NaiveDate)>) -> Self {
        self.range = range;
        self
    }

    fn value_to_color(&self, value: f64, thresholds: &[f64; 4]) -> Color {
        if value <= 0.0 {
            return self.empty;
        }

        if value < thresholds[0] {
            self.palette[0]
        } else if value < thresholds[1] {
            self.palette[1]
        } else if value < thresholds[2] {
            self.palette[2]
        } else if value < thresholds[3] {
            self.palette[3]
        } else {
            self.palette[4]
        }
    }
}

fn compute_quantiles(cell_values: &BTreeMap<(usize, usize), f64>) -> [f64; 4] {
    let mut sorted: Vec<f64> = cell_values.values().copied().filter(|&v| v > 0.0).collect();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.is_empty() {
        return [0.0; 4];
    }
    let p = |pct: f64| -> f64 {
        let idx = (pct / 100.0 * (sorted.len() - 1) as f64).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    };
    [p(20.0), p(40.0), p(60.0), p(80.0)]
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

        // Adaptive cell width: double-char + gap when space allows
        let cell_stride = if grid_width >= total_weeks * 3 {
            3
        } else if grid_width >= total_weeks * 2 {
            2
        } else {
            1
        };
        let display_cols = (grid_width / cell_stride).min(total_weeks).max(1);

        let mut cell_values: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        let mut month_labels: BTreeMap<usize, String> = BTreeMap::new();
        let mut selected_cell = None;

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

            let key = (display_col.min(display_cols.saturating_sub(1)), row);
            cell_values
                .entry(key)
                .and_modify(|existing| *existing = existing.max(value))
                .or_insert(value);
            if self.selected == Some(cursor) {
                selected_cell = Some(key);
            }

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

        let thresholds = compute_quantiles(&cell_values);

        let weekday_labels = ["S", "M", "T", "W", "T", "F", "S"];
        for (row, label) in weekday_labels.iter().enumerate() {
            let y = grid_y + row as u16;
            if y < area.y + area.height {
                buf.set_string(area.x, y, *label, Style::default().fg(Color::Gray));
            }
        }

        for (col, label) in &month_labels {
            let x = grid_x + (*col as u16) * (cell_stride as u16);
            if x < area.x + area.width {
                buf.set_string(x, area.y, label, Style::default().fg(Color::Gray));
            }
        }

        let (sym_filled, sym_empty, sym_selected) = if cell_stride >= 2 {
            ("██", "··", "◆◆")
        } else {
            ("■", "·", "◆")
        };

        for col in 0..display_cols {
            for row in 0..7 {
                let x = grid_x + (col * cell_stride) as u16;
                let y = grid_y + row as u16;
                if x + cell_stride as u16 > area.x + area.width || y >= area.y + area.height {
                    continue;
                }

                let value = cell_values.get(&(col, row)).copied().unwrap_or(0.0);
                let color = self.value_to_color(value, &thresholds);
                let is_selected = selected_cell == Some((col, row));
                let symbol = if is_selected {
                    sym_selected
                } else if value <= 0.0 {
                    sym_empty
                } else {
                    sym_filled
                };
                let style = if is_selected {
                    Style::default().fg(self.selected_color).bg(color)
                } else {
                    Style::default().fg(color)
                };
                buf.set_string(x, y, symbol, style);
            }
        }

        let max_value = cell_values.values().copied().fold(0.0f64, f64::max);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_heatmap_keeps_selected_day_highlight() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 7).unwrap();
        let end = start + Duration::days(69);
        let selected = start + Duration::days(63);
        let points = vec![(selected, 42.0)];
        let area = Rect::new(0, 0, 12, 10);
        let mut buf = Buffer::empty(area);

        YearHeatmap::new(&points, HeatmapMetric::TotalTokens)
            .selected(Some(selected))
            .range_opt(Some((start, end)))
            .render(area, &mut buf);

        let selected_x = area.x + 4 + 7;
        let selected_y = area.y + 1;
        assert_eq!(buf[(selected_x, selected_y)].symbol(), "◆");
    }
}
