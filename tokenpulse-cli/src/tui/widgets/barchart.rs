use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use std::collections::HashMap;

pub struct StackedBarChart<'a> {
    data: &'a [(f64, HashMap<&'a str, f64>)],
    colors: HashMap<&'a str, Color>,
    max_value: f64,
    width: usize,
    height: usize,
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
            width: 50,
            height: 10,
        }
    }

    pub fn color(mut self, provider: &'a str, color: Color) -> Self {
        self.colors.insert(provider, color);
        self
    }

    pub fn width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    pub fn height(mut self, height: usize) -> Self {
        self.height = height;
        self
    }
}

impl<'a> Widget for StackedBarChart<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if self.data.is_empty() || area.height < 2 {
            return;
        }

        let chart_height = self.height.min(area.height as usize);

        for (bar_idx, (_label, values)) in self.data.iter().enumerate() {
            let bar_x = area.x + bar_idx as u16;
            if bar_x >= area.x + self.width as u16 {
                break;
            }

            let total: f64 = values.values().sum();
            let bar_height = (total / self.max_value * chart_height as f64) as usize;

            let mut current_y = area.y + (chart_height - bar_height) as u16;

            for (provider, value) in values {
                let segment_height = (*value / total * bar_height as f64) as usize;
                let color = self.colors.get(provider).copied().unwrap_or(Color::White);

                for _ in 0..segment_height.max(1) {
                    if current_y < area.y + chart_height as u16 {
                        buf.set_string(bar_x, current_y, "█", Style::default().fg(color));
                        current_y += 1;
                    }
                }
            }
        }
    }
}
