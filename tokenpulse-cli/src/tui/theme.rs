use ratatui::style::Color;

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub card: Color,

    pub claude: Color,
    pub codex: Color,
    pub opencode: Color,
    pub gemini: Color,
    pub pi: Color,
    pub antigravity: Color,

    pub gauge_low: Color,
    pub gauge_mid: Color,
    pub gauge_high: Color,
    pub token_heatmap: [Color; 5],
    pub cost_heatmap: [Color; 5],
    pub count_heatmap: [Color; 5],
    pub empty_heatmap: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::Gray,
            border: Color::DarkGray,
            accent: Color::Cyan,
            accent_soft: Color::Rgb(66, 153, 225),
            card: Color::Rgb(24, 28, 38),

            claude: Color::Rgb(217, 119, 6),
            codex: Color::Rgb(16, 185, 129),
            opencode: Color::Rgb(99, 102, 241),
            gemini: Color::Rgb(66, 133, 244),
            pi: Color::Rgb(236, 72, 153),
            antigravity: Color::Rgb(168, 85, 247),

            gauge_low: Color::Green,
            gauge_mid: Color::Yellow,
            gauge_high: Color::Red,
            token_heatmap: [
                Color::Rgb(18, 78, 65),
                Color::Rgb(25, 107, 85),
                Color::Rgb(33, 138, 104),
                Color::Rgb(54, 181, 135),
                Color::Rgb(102, 237, 173),
            ],
            cost_heatmap: [
                Color::Rgb(71, 44, 18),
                Color::Rgb(115, 65, 21),
                Color::Rgb(168, 92, 25),
                Color::Rgb(224, 131, 49),
                Color::Rgb(255, 190, 92),
            ],
            count_heatmap: [
                Color::Rgb(18, 49, 76),
                Color::Rgb(29, 76, 116),
                Color::Rgb(38, 101, 147),
                Color::Rgb(61, 145, 204),
                Color::Rgb(118, 191, 255),
            ],
            empty_heatmap: Color::Rgb(35, 39, 48),
        }
    }
}

impl Theme {
    pub fn provider_color(&self, provider: &str) -> Color {
        match provider {
            "claude" => self.claude,
            "codex" => self.codex,
            "opencode" => self.opencode,
            "gemini" => self.gemini,
            "pi" => self.pi,
            "antigravity" => self.antigravity,
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

    pub fn pace_color(&self, status: &str) -> Color {
        match status {
            "ahead" => self.gauge_low,
            "on-track" => self.gauge_mid,
            "behind" => self.gauge_high,
            _ => self.dim,
        }
    }
}
