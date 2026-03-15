use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};

pub struct GradientGauge<'a> {
    label: &'a str,
    percent: f64,
    width: usize,
    color: Color,
    show_time: Option<&'a str>,
}

impl<'a> GradientGauge<'a> {
    pub fn new(label: &'a str, percent: f64) -> Self {
        Self {
            label,
            percent: percent.clamp(0.0, 100.0),
            width: 30,
            color: Color::Green,
            show_time: None,
        }
    }

    pub fn width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    pub fn color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn time(mut self, time: &'a str) -> Self {
        self.show_time = Some(time);
        self
    }
}

impl<'a> Widget for GradientGauge<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let filled = (self.percent / 100.0 * self.width as f64) as usize;

        let bar: String = std::iter::repeat('█')
            .take(filled)
            .chain(std::iter::repeat('░').take(self.width - filled))
            .collect();

        let text = if let Some(time) = self.show_time {
            format!(
                "{} {} {}% ⏳ {}",
                self.label, bar, self.percent as i32, time
            )
        } else {
            format!("{} {} {}%", self.label, bar, self.percent as i32)
        };

        let style = Style::default().fg(self.color);
        buf.set_string(area.x, area.y, text, style);
    }
}
