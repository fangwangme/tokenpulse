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
    x_labels: &'a [String],
    bar_width: usize,
}

impl<'a> StackedBarChart<'a> {
    pub fn new(data: &'a [(f64, HashMap<&'a str, f64>)], bar_width: usize) -> Self {
        let max_value = data
            .iter()
            .map(|(_, vals)| vals.values().sum())
            .fold(0.0, f64::max);

        Self {
            data,
            colors: HashMap::new(),
            max_value: max_value.max(1.0),
            value_format: ValueFormat::Currency,
            x_labels: &[],
            bar_width: bar_width.max(1),
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

    /// Date labels for the X axis, one per data point (oldest first). A few
    /// evenly spaced entries — always including the first and last — are drawn
    /// along the bottom row to help locate dates.
    pub fn x_labels(mut self, labels: &'a [String]) -> Self {
        self.x_labels = labels;
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

        let bar_width = self.bar_width;
        let bars_len = self.data.len();
        if bars_len == 0 {
            return;
        }

        let total_bars_width = bars_len * bar_width;
        let chart_offset = if (chart_width as usize) > total_bars_width {
            ((chart_width as usize - total_bars_width) / 2) as u16
        } else {
            0u16
        };
        let start_x = chart_x + chart_offset;

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
        for (bar_idx, values) in self.data.iter().enumerate() {
            let bar_x = start_x + (bar_idx * bar_width) as u16;
            if bar_x + bar_width as u16 > chart_x + chart_width {
                break;
            }

            let total = values.0;
            if total <= 0.0 {
                continue;
            }

            let height_eighths =
                (total / self.max_value * (chart_height * 8) as f64).round() as usize;
            let full_rows = height_eighths / 8;
            let partial = height_eighths % 8;

            let mut segments: Vec<_> = values.1.iter().collect();
            segments.sort_by(|left, right| {
                right
                    .1
                    .partial_cmp(left.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
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

        // X-axis: evenly spaced date ticks aligned to the actual bar columns.
        if !self.x_labels.is_empty() {
            let axis_y = area.y + chart_height as u16;
            render_x_axis(
                buf,
                start_x,
                total_bars_width as u16,
                bar_width as u16,
                bars_len,
                axis_y,
                self.x_labels,
            );
        }
    }
}

/// Draw evenly spaced date ticks aligned to the bar columns. The first and last
/// bars always get a label (oldest / newest date); intermediate ticks are added
/// when the width allows. Each label is centered on the column it refers to, so
/// the date always lines up with its bar — including when days are aggregated
/// into buckets and the bars occupy less than the full chart width.
fn render_x_axis(
    buf: &mut Buffer,
    chart_x: u16,
    total_bars_width: u16,
    bar_width: u16,
    bars_len: usize,
    y: u16,
    labels: &[String],
) {
    let label_w = 5u16; // "MM-DD"
    let n = labels.len();
    if n == 0 || bars_len == 0 || bar_width == 0 || total_bars_width == 0 {
        return;
    }

    if total_bars_width < label_w {
        return;
    }

    // Pick a tick count whose even spacing leaves room between labels.
    let max_ticks = (1 + total_bars_width.saturating_sub(1) / (label_w + 3)).clamp(2, 6) as usize;
    let ticks = max_ticks.min(bars_len);

    let mut last_end: i64 = i64::MIN;
    for tick in 0..ticks {
        let bar_idx = if ticks <= 1 {
            0
        } else {
            (tick as f64 * (bars_len - 1) as f64 / (ticks - 1) as f64).round() as usize
        };

        // Date for this bar
        let data_idx = if bars_len >= n {
            bar_idx.min(n - 1)
        } else if bar_idx + 1 == bars_len {
            n - 1
        } else {
            (bar_idx * n / bars_len).min(n - 1)
        };

        let label = &labels[data_idx];
        let lw = UnicodeWidthStr::width(label.as_str()) as u16;
        let bar_center = chart_x as f64 + (bar_idx as f64 + 0.5) * bar_width as f64;
        let max_start = (chart_x + total_bars_width).saturating_sub(lw);
        let start =
            ((bar_center - lw as f64 / 2.0).round() as i64).clamp(chart_x as i64, max_start as i64);
        if start <= last_end {
            continue; // never overwrite the previous label
        }
        buf.set_string(start as u16, y, label, Style::default().fg(Color::DarkGray));
        last_end = start + lw as i64;
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
                format!("${}", format_int_commas(value.round() as i64))
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

fn format_int_commas(value: i64) -> String {
    let raw = value.to_string();
    let digits = raw.strip_prefix('-').unwrap_or(&raw);
    let mut formatted_rev = String::with_capacity(raw.len() + raw.len() / 3);

    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted_rev.push(',');
        }
        formatted_rev.push(ch);
    }

    let formatted: String = formatted_rev.chars().rev().collect();
    if raw.starts_with('-') {
        format!("-{}", formatted)
    } else {
        formatted
    }
}

fn format_compact(value: i64) -> String {
    let abs = value.abs();
    if abs >= 1_000_000_000 {
        format!("{:.4}B", value as f64 / 1_000_000_000.0)
    } else if abs >= 1_000_000 {
        format!("{:.4}M", value as f64 / 1_000_000.0)
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

    #[test]
    fn currency_y_labels_use_thousands_separators() {
        assert_eq!(format_y_label(3_000.0, ValueFormat::Currency), "$3,000");
    }
}
