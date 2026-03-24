use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

const SPARK_CHARS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub struct TrendSparkline<'a> {
    values: &'a [f64],
    color: Color,
    empty: Color,
    min_label: Option<String>,
    max_label: Option<String>,
}

impl<'a> TrendSparkline<'a> {
    pub fn new(values: &'a [f64]) -> Self {
        Self {
            values,
            color: Color::Cyan,
            empty: Color::DarkGray,
            min_label: None,
            max_label: None,
        }
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn empty(mut self, color: Color) -> Self {
        self.empty = color;
        self
    }

    pub fn labels(mut self, min_label: String, max_label: String) -> Self {
        self.min_label = Some(min_label);
        self.max_label = Some(max_label);
        self
    }

    fn downsample(&self, width: usize) -> Vec<f64> {
        if self.values.is_empty() || width == 0 {
            return vec![];
        }

        if self.values.len() <= width {
            return self.values.to_vec();
        }

        let mut sampled = Vec::with_capacity(width);
        for idx in 0..width {
            let start = idx * self.values.len() / width;
            let end = ((idx + 1) * self.values.len() / width).max(start + 1);
            let slice = &self.values[start..end.min(self.values.len())];
            let avg = slice.iter().sum::<f64>() / slice.len().max(1) as f64;
            sampled.push(avg);
        }

        sampled
    }
}

impl<'a> Widget for TrendSparkline<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        if self.values.is_empty() {
            buf.set_string(area.x, area.y, "No data", Style::default().fg(self.empty));
            return;
        }

        let sampled = self.downsample(area.width as usize);
        let max_value = sampled.iter().copied().fold(0.0, f64::max).max(1.0);
        let min_value = sampled
            .iter()
            .copied()
            .fold(f64::MAX, f64::min)
            .min(max_value);

        let mut line = String::with_capacity(sampled.len());
        for value in sampled {
            let ratio = (value / max_value).clamp(0.0, 1.0);
            let idx = (ratio * (SPARK_CHARS.len() as f64 - 1.0)).round() as usize;
            line.push(SPARK_CHARS[idx.min(SPARK_CHARS.len() - 1)]);
        }

        buf.set_string(area.x, area.y, line, Style::default().fg(self.color));

        if area.height > 1 {
            let min_label = self
                .min_label
                .unwrap_or_else(|| format!("{:.2}", min_value));
            let max_label = self
                .max_label
                .unwrap_or_else(|| format!("{:.2}", max_value));
            let right_x = area.x + area.width.saturating_sub(max_label.chars().count() as u16);
            buf.set_string(
                area.x,
                area.y + 1,
                min_label,
                Style::default().fg(self.empty),
            );
            buf.set_string(
                right_x,
                area.y + 1,
                max_label,
                Style::default().fg(self.empty),
            );
        }
    }
}
