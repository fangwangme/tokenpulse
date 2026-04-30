use chrono::{Datelike, Duration, Local, NaiveDate, Weekday};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Widget},
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
    background: Color,
    border: Option<Color>,
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
            background: Color::Rgb(248, 250, 252),
            border: None,
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

    pub fn background(mut self, color: Color) -> Self {
        self.background = color;
        self
    }

    pub fn border(mut self, color: Option<Color>) -> Self {
        self.border = color;
        self
    }

    pub fn selected(mut self, selected: Option<NaiveDate>) -> Self {
        self.selected = selected;
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

    fn thresholds(&self, cell_values: &BTreeMap<(usize, usize), f64>) -> [f64; 4] {
        compute_max_relative_thresholds(cell_values)
    }
}

fn compute_max_relative_thresholds(cell_values: &BTreeMap<(usize, usize), f64>) -> [f64; 4] {
    let max_value = cell_values
        .values()
        .copied()
        .filter(|&v| v > 0.0)
        .fold(0.0f64, f64::max);
    if max_value <= 0.0 {
        return [0.0; 4];
    }

    [
        max_value * 0.20,
        max_value * 0.40,
        max_value * 0.60,
        max_value * 0.80,
    ]
}

fn readable_foreground(background: Color, fallback: Color) -> Color {
    let Color::Rgb(r, g, b) = background else {
        return fallback;
    };

    let luminance = relative_luminance(r, g, b);
    let black_contrast = (luminance + 0.05) / 0.05;
    let white_contrast = 1.05 / (luminance + 0.05);
    if black_contrast >= white_contrast {
        Color::Black
    } else {
        Color::White
    }
}

fn relative_luminance(r: u8, g: u8, b: u8) -> f64 {
    fn channel(value: u8) -> f64 {
        let value = f64::from(value) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }

    0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
}

#[derive(Debug, Clone, Copy)]
struct HeatmapLayout {
    window_start: NaiveDate,
    start: NaiveDate,
    end: NaiveDate,
    first_week_idx: usize,
    display_cols: usize,
    cell_width: usize,
    grid_width: usize,
    grid_x: u16,
    grid_y: u16,
}

fn compute_layout(area: Rect, range: Option<(NaiveDate, NaiveDate)>) -> Option<HeatmapLayout> {
    if area.width < 30 || area.height < 8 {
        return None;
    }

    let (window_start, end) = range.unwrap_or_else(|| {
        let end = Local::now().date_naive();
        (end - Duration::days(364), end)
    });
    let mut start = window_start;
    while start.weekday() != Weekday::Sun {
        start -= Duration::days(1);
    }

    let total_days = (end - start).num_days().max(0) as usize + 1;
    let total_weeks = ((total_days + 6) / 7).max(1);
    let grid_x = area.x + 4;
    let grid_y = area.y + 1;
    let grid_width = area.width.saturating_sub(4) as usize;

    if grid_width == 0 {
        return None;
    }

    let cell_width = if grid_width >= total_weeks * 2 { 2 } else { 1 };
    let display_cols = (grid_width / cell_width).min(total_weeks).max(1);
    let first_week_idx = total_weeks.saturating_sub(display_cols);
    let rendered_width = display_cols * cell_width;

    Some(HeatmapLayout {
        window_start,
        start,
        end,
        first_week_idx,
        display_cols,
        cell_width,
        grid_width: rendered_width,
        grid_x,
        grid_y,
    })
}

fn display_col_x(layout: &HeatmapLayout, display_col: usize) -> u16 {
    layout.grid_x + (display_col * layout.cell_width) as u16
}

fn distribute_month_label_positions(
    labels: &[(usize, String)],
    layout: &HeatmapLayout,
    right_edge: u16,
) -> Vec<(u16, String)> {
    if labels.is_empty() {
        return Vec::new();
    }

    let total_label_width: u16 = labels
        .iter()
        .map(|(_, label)| label.chars().count() as u16)
        .sum();
    let available = right_edge.saturating_sub(layout.grid_x);
    let min_gap = if labels.len() > 1 {
        available
            .saturating_sub(total_label_width)
            .checked_div(labels.len().saturating_sub(1) as u16)
            .unwrap_or(0)
            .clamp(1, 4)
    } else {
        1
    };
    let mut out: Vec<(u16, String)> = labels
        .iter()
        .map(|(col, label)| (display_col_x(layout, *col), label.clone()))
        .collect();

    for idx in 1..out.len() {
        let prev_end = out[idx - 1].0 + out[idx - 1].1.chars().count() as u16;
        let min_x = prev_end + min_gap;
        if out[idx].0 < min_x {
            out[idx].0 = min_x;
        }
    }

    for idx in (0..out.len()).rev() {
        let label_width = out[idx].1.chars().count() as u16;
        if out[idx].0 + label_width > right_edge {
            out[idx].0 = right_edge.saturating_sub(label_width);
        }

        if idx > 0 {
            let min_prev_end = out[idx].0.saturating_sub(min_gap);
            let prev_width = out[idx - 1].1.chars().count() as u16;
            if out[idx - 1].0 + prev_width > min_prev_end {
                out[idx - 1].0 = min_prev_end.saturating_sub(prev_width);
            }
        }
    }

    for idx in 1..out.len() {
        let prev_end = out[idx - 1].0 + out[idx - 1].1.chars().count() as u16;
        let min_x = prev_end + min_gap;
        if out[idx].0 < min_x {
            out[idx].0 = min_x;
        }
    }

    out
}

pub fn date_at_position(
    area: Rect,
    range: Option<(NaiveDate, NaiveDate)>,
    x: u16,
    y: u16,
) -> Option<NaiveDate> {
    let layout = compute_layout(area, range)?;
    if x < layout.grid_x || y < layout.grid_y || y >= layout.grid_y + 7 {
        return None;
    }

    let col = (0..layout.display_cols).find(|display_col| {
        let col_x = display_col_x(&layout, *display_col);
        x >= col_x && x < col_x + layout.cell_width as u16
    })?;
    let row = (y - layout.grid_y) as usize;

    let week_idx = layout.first_week_idx + col;
    let date = layout.start + Duration::days((week_idx * 7 + row) as i64);
    (date >= layout.window_start && date <= layout.end).then_some(date)
}

impl<'a> Widget for YearHeatmap<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let Some(layout) = compute_layout(area, self.range) else {
            return;
        };
        let HeatmapLayout {
            window_start,
            start,
            end,
            first_week_idx,
            display_cols,
            cell_width,
            grid_width,
            grid_x,
            grid_y,
            ..
        } = layout;
        let grid_area = Rect::new(
            grid_x,
            grid_y,
            (grid_width as u16).min(area.x + area.width - grid_x),
            7.min(area.y + area.height - grid_y),
        );
        let cell_area = if let Some(border_color) = self.border {
            let framed = Rect::new(
                grid_area.x.saturating_sub(1),
                grid_area.y.saturating_sub(1),
                grid_area.width.saturating_add(2),
                grid_area.height.saturating_add(2),
            );
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .style(Style::default().bg(self.background))
                .render(framed, buf);
            grid_area
        } else {
            buf.set_style(grid_area, Style::default().bg(self.background));
            grid_area
        };

        let mut cell_values: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        let mut month_labels: Vec<(usize, String)> = Vec::new();
        let mut selected_cell = None;

        let values: BTreeMap<NaiveDate, f64> = self.points.iter().copied().collect();
        let mut cursor = start + Duration::days((first_week_idx * 7) as i64);
        let mut day_idx = first_week_idx * 7;
        let mut last_month = None;

        while cursor <= end {
            let week_idx = day_idx / 7;
            let Some(display_col) = week_idx.checked_sub(first_week_idx) else {
                cursor += Duration::days(1);
                day_idx += 1;
                continue;
            };
            if display_col >= display_cols {
                break;
            }
            let row = cursor.weekday().num_days_from_sunday() as usize;

            let value = if cursor >= window_start {
                values.get(&cursor).copied().unwrap_or(0.0)
            } else {
                0.0
            };

            let key = (display_col, row);
            cell_values.insert(key, value);
            if self.selected == Some(cursor) {
                selected_cell = Some(key);
            }

            let month = cursor.month();
            if cursor >= window_start && last_month != Some(month) {
                month_labels.push((display_col, cursor.format("%b").to_string()));
                last_month = Some(month);
            }

            cursor += Duration::days(1);
            day_idx += 1;
        }

        let thresholds = self.thresholds(&cell_values);

        let weekday_labels = ["S", "M", "T", "W", "T", "F", "S"];
        for (row, label) in weekday_labels.iter().enumerate() {
            let y = grid_y + row as u16;
            if y < area.y + area.height {
                buf.set_string(area.x, y, *label, Style::default().fg(Color::DarkGray));
            }
        }

        let label_positions =
            distribute_month_label_positions(&month_labels, &layout, area.x + area.width);
        for (x, label) in label_positions {
            let label_width = label.chars().count() as u16;
            if x + label_width <= area.x + area.width {
                buf.set_string(x, area.y, label, Style::default().fg(Color::DarkGray));
            }
        }

        let (sym_cell, sym_selected) = if cell_width >= 2 {
            ("██", "◆◆")
        } else {
            ("█", "◆")
        };

        for col in 0..display_cols {
            for row in 0..7 {
                let x = display_col_x(&layout, col);
                let y = grid_y + row as u16;
                if x + cell_width as u16 > cell_area.x + cell_area.width
                    || y >= cell_area.y + cell_area.height
                {
                    continue;
                }

                let value = cell_values.get(&(col, row)).copied().unwrap_or(0.0);
                let color = self.value_to_color(value, &thresholds);
                let is_selected = selected_cell == Some((col, row));
                let symbol = if is_selected { sym_selected } else { sym_cell };
                let style = if is_selected {
                    Style::default()
                        .fg(readable_foreground(color, self.selected_color))
                        .bg(color)
                } else {
                    Style::default().fg(color).bg(self.background)
                };
                buf.set_string(x, y, symbol, style);
            }
        }

        let max_value = cell_values.values().copied().fold(0.0f64, f64::max);
        let visible_start = (start + Duration::days((first_week_idx * 7) as i64)).max(window_start);
        let visible_end =
            (start + Duration::days(((first_week_idx + display_cols) * 7 - 1) as i64)).min(end);
        let metric_text = format!(
            "{}  {} → {}  max {:.2}",
            self.metric.label(),
            visible_start.format("%Y-%m-%d"),
            visible_end.format("%Y-%m-%d"),
            max_value
        );
        let footer_y = area.y + area.height.saturating_sub(1);
        if footer_y > area.y + 7 {
            buf.set_string(
                area.x,
                footer_y,
                metric_text,
                Style::default().fg(Color::DarkGray),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heatmap_keeps_selected_day_highlight() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 7).unwrap();
        let end = start + Duration::days(69);
        let selected = start + Duration::days(63);
        let points = vec![(selected, 42.0)];
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);

        YearHeatmap::new(&points, HeatmapMetric::TotalTokens)
            .selected(Some(selected))
            .range_opt(Some((start, end)))
            .render(area, &mut buf);

        let layout = compute_layout(area, Some((start, end))).unwrap();
        let selected_x = display_col_x(&layout, 9);
        let selected_y = area.y + 1;
        assert_eq!(buf[(selected_x, selected_y)].symbol(), "◆");
    }

    #[test]
    fn heatmap_uses_solid_cells_without_texture_patterns() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 7).unwrap();
        let end = start + Duration::days(34);
        let points = vec![(start, 1.0), (start + Duration::days(7), 10.0)];
        let area = Rect::new(0, 0, 72, 10);
        let mut buf = Buffer::empty(area);

        YearHeatmap::new(&points, HeatmapMetric::TotalTokens)
            .range_opt(Some((start, end)))
            .render(area, &mut buf);

        let layout = compute_layout(area, Some((start, end))).unwrap();
        assert_eq!(layout.cell_width, 2);
        assert_eq!(
            buf[(display_col_x(&layout, 0), layout.grid_y)].symbol(),
            "█"
        );
        assert_eq!(
            buf[(display_col_x(&layout, 1), layout.grid_y)].symbol(),
            "█"
        );
        assert_eq!(
            buf[(display_col_x(&layout, 0), layout.grid_y + 1)].symbol(),
            "█"
        );
    }

    #[test]
    fn narrow_heatmap_clips_old_weeks_without_merging_cells() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 7).unwrap();
        let end = start + Duration::days(370);
        let old_sunday = start;
        let recent_sunday = end - Duration::days(end.weekday().num_days_from_sunday() as i64);
        let points = vec![(old_sunday, 99.0), (recent_sunday, 1.0)];
        let area = Rect::new(0, 0, 30, 10);
        let mut buf = Buffer::empty(area);

        YearHeatmap::new(&points, HeatmapMetric::TotalTokens)
            .range_opt(Some((start, end)))
            .render(area, &mut buf);

        let layout = compute_layout(area, Some((start, end))).unwrap();
        assert_eq!(layout.cell_width, 1);
        assert!(layout.first_week_idx > 0);

        let first_visible_date =
            date_at_position(area, Some((start, end)), layout.grid_x, layout.grid_y);
        assert_ne!(first_visible_date, Some(old_sunday));

        let recent_col = layout.display_cols - 1;
        let recent_x = display_col_x(&layout, recent_col);
        assert_eq!(buf[(recent_x, layout.grid_y)].symbol(), "█");
    }

    #[test]
    fn narrow_heatmap_returns_no_layout() {
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

        assert_eq!(buf[(area.x + 4, area.y + 1)].symbol(), " ");
    }

    #[test]
    fn date_at_position_matches_rendered_cell() {
        let start = NaiveDate::from_ymd_opt(2024, 1, 7).unwrap();
        let end = start + Duration::days(34);
        let selected = start + Duration::days(10);
        let area = Rect::new(0, 0, 72, 10);
        let layout = compute_layout(area, Some((start, end))).unwrap();
        let x = display_col_x(&layout, 1);
        let y = area.y + 1 + 3;

        let date = date_at_position(area, Some((start, end)), x, y);

        assert_eq!(date, Some(selected));
    }

    #[test]
    fn close_month_labels_are_shifted_not_dropped() {
        let start = NaiveDate::from_ymd_opt(2025, 4, 1).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 4, 25).unwrap();
        let area = Rect::new(0, 0, 70, 10);
        let mut buf = Buffer::empty(area);

        YearHeatmap::new(&[], HeatmapMetric::TotalTokens)
            .range_opt(Some((start, end)))
            .render(area, &mut buf);

        let label_row = (0..area.width)
            .map(|x| buf[(x, area.y)].symbol())
            .collect::<String>();
        assert!(label_row.contains("Apr"));
        assert!(label_row.contains("May"));
    }

    #[test]
    fn close_month_labels_keep_even_spacing() {
        let labels = vec![
            (4, "Apr".to_string()),
            (5, "May".to_string()),
            (9, "Jun".to_string()),
        ];
        let layout = HeatmapLayout {
            window_start: NaiveDate::from_ymd_opt(2025, 4, 1).unwrap(),
            start: NaiveDate::from_ymd_opt(2025, 4, 1).unwrap(),
            end: NaiveDate::from_ymd_opt(2025, 6, 1).unwrap(),
            first_week_idx: 0,
            display_cols: 10,
            cell_width: 1,
            grid_width: 36,
            grid_x: 4,
            grid_y: 1,
        };

        let positions = distribute_month_label_positions(&labels, &layout, 40);

        assert_eq!(positions[0].1, "Apr");
        assert_eq!(positions[1].1, "May");
        assert_eq!(positions[2].1, "Jun");
        assert!(positions[1].0 >= positions[0].0 + 7);
        assert!(positions[2].0 >= positions[1].0 + 7);
    }

    #[test]
    fn max_relative_thresholds_scale_all_metrics_to_window_peak() {
        let cell_values = (0..10)
            .map(|i| ((i, 0), (i + 1) as f64))
            .collect::<BTreeMap<_, _>>();

        let thresholds = compute_max_relative_thresholds(&cell_values);
        let palette = [
            Color::Black,
            Color::Red,
            Color::Yellow,
            Color::Blue,
            Color::Green,
        ];

        assert_eq!(thresholds, [2.0, 4.0, 6.0, 8.0]);
        assert_eq!(
            YearHeatmap::new(&[], HeatmapMetric::TotalTokens)
                .palette(palette)
                .value_to_color(8.0, &thresholds),
            palette[4]
        );
        assert_eq!(
            YearHeatmap::new(&[], HeatmapMetric::Cost)
                .palette(palette)
                .value_to_color(1.0, &thresholds),
            palette[0]
        );
        assert_eq!(
            YearHeatmap::new(&[], HeatmapMetric::Cost)
                .palette(palette)
                .value_to_color(10.0, &thresholds),
            palette[4]
        );
    }

    #[test]
    fn selected_foreground_adapts_to_cell_background() {
        assert_eq!(
            readable_foreground(Color::Rgb(224, 242, 254), Color::White),
            Color::Black
        );
        assert_eq!(
            readable_foreground(Color::Rgb(8, 47, 73), Color::Black),
            Color::White
        );
    }
}
