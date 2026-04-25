use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::HashMap;
use unicode_width::UnicodeWidthStr;

/// Sub-cell precision bar characters (⅛ increments)
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

#[derive(Debug, Clone, Copy)]
pub enum ValueFormat {
    Currency,
    CompactNumber,
}

pub struct StackedBarChart<'a> {
    data: &'a [(f64, HashMap<&'a str, f64>)],
    colors: HashMap<&'a str, Color>,
    max_value: f64,
    value_format: ValueFormat,
}

impl<'a> StackedBarChart<'a> {
    pub fn new(data: &'a [(f64, HashMap<&'a str, f64>)]) -> Self {
        let max_value = data
            .iter()
            .map(|(_, vals)| vals.values().sum())
            .fold(0.0, f64::max);

        Self {
            data,
            colors: HashMap::new(),
            max_value: max_value.max(1.0),
            value_format: ValueFormat::Currency,
        }
    }

    pub fn color(mut self, provider: &'a str, color: Color) -> Self {
        self.colors.insert(provider, color);
        self
    }

    pub fn value_format(mut self, value_format: ValueFormat) -> Self {
        self.value_format = value_format;
        self
    }
}

impl<'a> Widget for StackedBarChart<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.data.is_empty() || area.height < 3 || area.width < 10 {
            return;
        }

        // Reserve space for Y-axis labels (e.g. "$12.34")
        let y_axis_width = 7u16;
        let chart_x = area.x + y_axis_width;
        let chart_width = area.width.saturating_sub(y_axis_width + 1);
        let chart_height = area.height.saturating_sub(1) as usize; // leave 1 row for x-axis

        if chart_width == 0 || chart_height == 0 {
            return;
        }

        let desired_bar_width = (chart_width as usize / self.data.len().max(1)).clamp(1, 3);
        let max_bars = (chart_width as usize / desired_bar_width).max(1);
        let bars = aggregated_bars(self.data, max_bars);
        let bar_width = desired_bar_width.min((chart_width as usize / bars.len().max(1)).max(1));

        // Render Y-axis labels (4 evenly spaced ticks)
        let num_ticks = chart_height.min(4).max(2);
        for tick in 0..num_ticks {
            let value = self.max_value * (num_ticks - tick) as f64 / num_ticks as f64;
            let row = area.y + (tick * chart_height / num_ticks) as u16;
            let label = format_y_label(value, self.value_format);
            let label_w = UnicodeWidthStr::width(label.as_str()).min(y_axis_width as usize);
            let pad = y_axis_width as usize - label_w;
            buf.set_string(
                area.x + pad as u16,
                row,
                &label,
                Style::default().fg(Color::DarkGray),
            );
        }

        // Render bars
        for (bar_idx, values) in bars.iter().enumerate() {
            let bar_x = chart_x + (bar_idx * bar_width) as u16;
            if bar_x + bar_width as u16 > chart_x + chart_width {
                break;
            }

            let total: f64 = values.values().sum();
            if total <= 0.0 {
                continue;
            }

            let height_eighths =
                (total / self.max_value * (chart_height * 8) as f64).round() as usize;
            let full_rows = height_eighths / 8;
            let partial = height_eighths % 8;

            let mut segments: Vec<_> = values.iter().collect();
            segments.sort_by(|left, right| {
                stack_order(left.0)
                    .cmp(&stack_order(right.0))
                    .then_with(|| left.0.cmp(right.0))
            });

            let segment_units = allocate_segment_units(&segments, total, height_eighths);
            let mut allocated = 0usize;
            let mut segment_ranges = Vec::new();
            for ((name, _), units) in segments.iter().zip(segment_units) {
                let start = allocated;
                allocated += units.min(height_eighths.saturating_sub(allocated));
                let end = allocated;
                let color = self.colors.get(*name).copied().unwrap_or(Color::White);
                segment_ranges.push((start, end, color));
            }

            for row in 0..full_rows {
                let y = area.y + (chart_height - 1 - row) as u16;
                render_band(
                    buf,
                    bar_x,
                    y,
                    bar_width,
                    '█',
                    &segment_ranges,
                    row * 8,
                    row * 8 + 8,
                );
            }

            if partial > 0 && full_rows < chart_height {
                let y = area.y + (chart_height - 1 - full_rows) as u16;
                render_band(
                    buf,
                    bar_x,
                    y,
                    bar_width,
                    BLOCKS[partial - 1],
                    &segment_ranges,
                    full_rows * 8,
                    full_rows * 8 + partial,
                );
            }
        }
    }
}

fn allocate_segment_units(
    segments: &[(&&str, &f64)],
    total: f64,
    height_eighths: usize,
) -> Vec<usize> {
    if total <= 0.0 || height_eighths == 0 || segments.is_empty() {
        return vec![0; segments.len()];
    }

    let mut allocated = Vec::with_capacity(segments.len());
    let mut remainder_rank = Vec::with_capacity(segments.len());
    let mut used = 0usize;

    for (idx, (_, value)) in segments.iter().enumerate() {
        let exact = (**value / total) * height_eighths as f64;
        let whole = exact.floor().max(0.0) as usize;
        allocated.push(whole);
        used += whole;
        remainder_rank.push((idx, exact - whole as f64));
    }

    remainder_rank.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });

    for (idx, _) in remainder_rank
        .into_iter()
        .take(height_eighths.saturating_sub(used))
    {
        allocated[idx] += 1;
    }

    allocated
}

fn stack_order(name: &str) -> usize {
    match name {
        "openai" => 0,
        "other" => 1,
        "google" => 2,
        "anthropic" => 3,
        _ => 4,
    }
}

fn aggregated_bars<'a>(
    data: &'a [(f64, HashMap<&'a str, f64>)],
    max_columns: usize,
) -> Vec<HashMap<&'a str, f64>> {
    if data.is_empty() || max_columns == 0 {
        return Vec::new();
    }

    if data.len() <= max_columns {
        return data.iter().map(|(_, values)| values.clone()).collect();
    }

    let bucket_count = max_columns.min(data.len());
    let mut buckets = Vec::with_capacity(bucket_count);

    for bucket_idx in 0..bucket_count {
        let start = bucket_idx * data.len() / bucket_count;
        let end = ((bucket_idx + 1) * data.len() / bucket_count).max(start + 1);
        let mut merged = HashMap::new();

        for (_, values) in &data[start..end.min(data.len())] {
            for (name, value) in values {
                *merged.entry(*name).or_insert(0.0) += *value;
            }
        }

        buckets.push(merged);
    }

    buckets
}

fn render_band(
    buf: &mut Buffer,
    bar_x: u16,
    y: u16,
    bar_width: usize,
    fill_char: char,
    ranges: &[(usize, usize, Color)],
    start: usize,
    end: usize,
) {
    let color = dominant_band_color(ranges, start, end).unwrap_or(Color::White);
    let band: String = std::iter::repeat(fill_char).take(bar_width).collect();
    buf.set_string(bar_x, y, &band, Style::default().fg(color));
}

fn dominant_band_color(
    ranges: &[(usize, usize, Color)],
    start: usize,
    end: usize,
) -> Option<Color> {
    ranges
        .iter()
        .filter_map(|(seg_start, seg_end, color)| {
            let overlap = overlap_len(start, end, *seg_start, *seg_end);
            (overlap > 0).then_some((overlap, *color))
        })
        .max_by_key(|(overlap, _)| *overlap)
        .map(|(_, color)| color)
}

fn overlap_len(start: usize, end: usize, seg_start: usize, seg_end: usize) -> usize {
    let overlap_start = start.max(seg_start);
    let overlap_end = end.min(seg_end);
    overlap_end.saturating_sub(overlap_start)
}

fn format_y_label(value: f64, value_format: ValueFormat) -> String {
    match value_format {
        ValueFormat::Currency => {
            if value >= 100.0 {
                format!("${:.0}", value)
            } else if value >= 10.0 {
                format!("${:.1}", value)
            } else if value >= 0.01 {
                format!("${:.2}", value)
            } else {
                "$0".to_string()
            }
        }
        ValueFormat::CompactNumber => format_compact(value.round() as i64),
    }
}

fn format_compact(value: i64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000 {
        format!("{:.1}B", value as f64 / 1_000_000_000.0)
    } else if abs >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if abs >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_units_preserve_large_token_ratios() {
        let openai = 27_885_943.0;
        let anthropic = 7_092_425.0;
        let google = 2_611_670.0;
        let other = 2_013_138.0;
        let segments = vec![
            (&"anthropic", &anthropic),
            (&"google", &google),
            (&"openai", &openai),
            (&"other", &other),
        ];

        let units = allocate_segment_units(&segments, openai + anthropic + google + other, 80);

        assert_eq!(units.iter().sum::<usize>(), 80);
        assert_eq!(units[0], 14);
        let rendered_ratio = units[2] as f64 / units[0] as f64;
        let actual_ratio = openai / anthropic;
        assert!((rendered_ratio - actual_ratio).abs() < 0.15);
    }
}
