use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
        if area.width == 0 || area.height == 0 {
            return;
        }

        let percent_text = format!("{:>3}%", self.percent.round() as i32);
        let trailing = if let Some(time) = self.show_time {
            format!(" {} {}", percent_text, truncate_display_width(time, 12))
        } else {
            format!(" {}", percent_text)
        };
        let trailing_width = UnicodeWidthStr::width(trailing.as_str());
        let total_width = area.width as usize;

        if trailing_width >= total_width {
            let text = truncate_display_width(trailing.trim(), total_width);
            buf.set_string(area.x, area.y, text, Style::default().fg(self.color));
            return;
        }

        let available = total_width.saturating_sub(trailing_width);
        let mut bar_width = self.width.min(available.saturating_sub(1));
        let mut label_width = available.saturating_sub(bar_width + 1);
        let min_label_width = self.label.chars().count().min(6);

        if label_width < min_label_width {
            let needed = min_label_width - label_width;
            bar_width = bar_width.saturating_sub(needed);
            label_width = available.saturating_sub(bar_width + 1);
        }

        if bar_width == 0 {
            bar_width = available.saturating_sub(1);
            label_width = available.saturating_sub(bar_width + 1);
        }

        let filled = (self.percent / 100.0 * bar_width as f64).round() as usize;
        let bar: String = std::iter::repeat('█')
            .take(filled.min(bar_width))
            .chain(std::iter::repeat('░').take(bar_width.saturating_sub(filled.min(bar_width))))
            .collect();

        let label = if label_width > 0 {
            format!("{} ", fit_display_width(self.label, label_width))
        } else {
            String::new()
        };

        let text = format!("{}{}{}", label, bar, trailing);
        let text = truncate_display_width(&text, total_width);

        let style = Style::default().fg(self.color);
        buf.set_string(area.x, area.y, text, style);
    }
}

fn fit_display_width(text: &str, width: usize) -> String {
    let truncated = truncate_display_width(text, width);
    let used = UnicodeWidthStr::width(truncated.as_str());
    if used >= width {
        truncated
    } else {
        format!("{}{}", truncated, " ".repeat(width - used))
    }
}

fn truncate_display_width(text: &str, width: usize) -> String {
    let mut out = String::new();
    let mut used = 0;

    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if used + ch_width > width {
            break;
        }
        out.push(ch);
        used += ch_width;
    }

    out
}
