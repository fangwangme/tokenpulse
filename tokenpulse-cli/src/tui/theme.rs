use ratatui::style::Color;

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,

    pub claude: Color,
    pub codex: Color,
    pub opencode: Color,
    pub pi: Color,

    pub gauge_low: Color,
    pub gauge_mid: Color,
    pub gauge_high: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::Gray,
            border: Color::DarkGray,
            accent: Color::Cyan,

            claude: Color::Rgb(217, 119, 6),
            codex: Color::Rgb(16, 185, 129),
            opencode: Color::Rgb(99, 102, 241),
            pi: Color::Rgb(236, 72, 153),

            gauge_low: Color::Green,
            gauge_mid: Color::Yellow,
            gauge_high: Color::Red,
        }
    }
}

impl Theme {
    pub fn provider_color(&self, provider: &str) -> Color {
        match provider {
            "claude" => self.claude,
            "codex" => self.codex,
            "opencode" => self.opencode,
            "pi" => self.pi,
            _ => self.fg,
        }
    }

    pub fn gauge_color(&self, percent: f64) -> Color {
        if percent < 50.0 {
            self.gauge_low
        } else if percent < 75.0 {
            self.gauge_mid
        } else {
            self.gauge_high
        }
    }
}
