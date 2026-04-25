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
}

impl<'a> TrendSparkline<'a> {
    pub fn new(values: &'a [f64]) -> Self {
        Self {
            values,
            color: Color::Cyan,
            empty: Color::DarkGray,
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

        let line = sparkline_text(self.values, area.width as usize);
        buf.set_string(area.x, area.y, line, Style::default().fg(self.color));
    }
}

pub fn sparkline_text(values: &[f64], width: usize) -> String {
    if values.is_empty() || width == 0 {
        return String::new();
    }

    let sampled = downsample(values, width);
    let max_value = sampled.iter().copied().fold(0.0, f64::max).max(1.0);

    sampled
        .into_iter()
        .map(|value| {
            let ratio = (value / max_value).clamp(0.0, 1.0);
            let idx = (ratio * (SPARK_CHARS.len() as f64 - 1.0)).round() as usize;
            SPARK_CHARS[idx.min(SPARK_CHARS.len() - 1)]
        })
        .collect()
}

fn downsample(values: &[f64], width: usize) -> Vec<f64> {
    if values.len() <= width {
        return values.to_vec();
    }

    let mut sampled = Vec::with_capacity(width);
    for idx in 0..width {
        let start = idx * values.len() / width;
        let end = ((idx + 1) * values.len() / width).max(start + 1);
        let slice = &values[start..end.min(values.len())];
        let avg = slice.iter().sum::<f64>() / slice.len().max(1) as f64;
        sampled.push(avg);
    }
    sampled
}
