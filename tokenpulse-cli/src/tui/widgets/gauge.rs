use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    widgets::Widget,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub struct GradientGauge<'a> {
    label: &'a str,
    percent: f64,
    expected_percent: Option<f64>,
    width: usize,
    color: Color,
    show_time: Option<&'a str>,
    label_width: Option<usize>,
}

impl<'a> GradientGauge<'a> {
    pub fn new(label: &'a str, percent: f64) -> Self {
        Self {
            label,
            percent: percent.clamp(0.0, 100.0),
            expected_percent: None,
            width: 30,
            color: Color::Green,
            show_time: None,
            label_width: None,
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

    pub fn expected_percent(mut self, pct: Option<f64>) -> Self {
        self.expected_percent = pct.map(|p| p.clamp(0.0, 100.0));
        self
    }

    /// Set fixed label column width for alignment across multiple gauges
    pub fn label_width(mut self, w: usize) -> Self {
        self.label_width = Some(w);
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

        // Use fixed label width if specified, otherwise auto-calculate
        let (label_width, bar_width) = if let Some(fixed_lw) = self.label_width {
            let lw = fixed_lw.min(available.saturating_sub(2));
            let bw = available.saturating_sub(lw + 1);
            (lw, bw)
        } else {
            let mut bw = self.width.min(available.saturating_sub(1));
            let mut lw = available.saturating_sub(bw + 1);
            let min_label_width = self.label.chars().count().min(6);

            if lw < min_label_width {
                let needed = min_label_width - lw;
                bw = bw.saturating_sub(needed);
                lw = available.saturating_sub(bw + 1);
            }

            if bw == 0 {
                bw = available.saturating_sub(1);
                lw = available.saturating_sub(bw + 1);
            }
            (lw, bw)
        };

        let filled = (self.percent / 100.0 * bar_width as f64).round() as usize;

        // Build bar with optional expected-progress marker
        let expected_pos = self
            .expected_percent
            .map(|ep| (ep / 100.0 * bar_width as f64).round() as usize);

        let bar: String = (0..bar_width)
            .map(|i| {
                if Some(i) == expected_pos {
                    '▏' // expected progress marker
                } else if i < filled.min(bar_width) {
                    '█'
                } else {
                    '░'
                }
            })
            .collect();

        let label = if label_width > 0 {
            format!("{} ", fit_display_width(self.label, label_width))
        } else {
            String::new()
        };

        let text = format!("{}{}{}", label, bar, trailing);
        let text = truncate_display_width(&text, total_width);

        // Render with color. If there's an expected marker, render it char by char
        // for different styling
        let label_chars = label.len();
        let style = Style::default().fg(self.color);
        let marker_style = Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD);

        if let Some(ep) = expected_pos {
            let mut x = area.x;
            for (i, ch) in text.chars().enumerate() {
                let s = if i >= label_chars && i < label_chars + bar_width && i - label_chars == ep
                {
                    marker_style
                } else {
                    style
                };
                let w = UnicodeWidthChar::width(ch).unwrap_or(1) as u16;
                buf.set_string(x, area.y, ch.to_string(), s);
                x += w;
            }
        } else {
            buf.set_string(area.x, area.y, text, style);
        }
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
