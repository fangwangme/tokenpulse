use ratatui::style::Color;

#[allow(dead_code)]
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
    pub copilot: Color,

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
            copilot: Color::Rgb(139, 92, 246),

            gauge_low: Color::Green,
            gauge_mid: Color::Yellow,
            gauge_high: Color::Red,
            token_heatmap: [
                Color::Rgb(22, 27, 34),
                Color::Rgb(155, 233, 168),
                Color::Rgb(64, 196, 99),
                Color::Rgb(48, 161, 78),
                Color::Rgb(33, 110, 57),
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
            "copilot" => self.copilot,
            _ => self.fg,
        }
    }

    pub fn model_color(&self, model: &str) -> Color {
        let model_lower = model.to_lowercase();

        if model_lower.contains("claude")
            || model_lower.contains("sonnet")
            || model_lower.contains("opus")
            || model_lower.contains("haiku")
        {
            Color::Rgb(218, 119, 86) // Anthropic coral
        } else if model_lower.contains("gpt")
            || model_lower.starts_with("o1")
            || model_lower.starts_with("o3")
            || model_lower.starts_with("o4")
        {
            Color::Rgb(16, 185, 129) // OpenAI green
        } else if model_lower.contains("gemini") {
            Color::Rgb(59, 130, 246) // Google blue
        } else if model_lower.contains("deepseek") {
            Color::Rgb(6, 182, 212) // DeepSeek cyan
        } else if model_lower.contains("grok") {
            Color::Rgb(234, 179, 8) // xAI yellow
        } else if model_lower.contains("llama") || model_lower.contains("meta") {
            Color::Rgb(99, 102, 241) // Meta indigo
        } else if model_lower.contains("nvidia") || model_lower.contains("nemotron") {
            Color::Rgb(118, 185, 0) // Nvidia green
        } else if model_lower.contains("mistral") || model_lower.contains("codestral") {
            Color::Rgb(255, 115, 29) // Mistral orange
        } else if model_lower.contains("qwen") {
            Color::Rgb(89, 64, 255) // Qwen purple
        } else {
            Color::Rgb(200, 200, 200) // Default light gray
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_color_anthropic_variants() {
        let t = Theme::default();
        let coral = Color::Rgb(218, 119, 86);
        assert_eq!(t.model_color("claude-sonnet-4-20250514"), coral);
        assert_eq!(t.model_color("claude-3.5-haiku"), coral);
        assert_eq!(t.model_color("claude-opus-4"), coral);
        assert_eq!(t.model_color("sonnet-4"), coral);
    }

    #[test]
    fn model_color_openai_variants() {
        let t = Theme::default();
        let green = Color::Rgb(16, 185, 129);
        assert_eq!(t.model_color("gpt-4.1"), green);
        assert_eq!(t.model_color("o3-mini"), green);
        assert_eq!(t.model_color("o4-mini"), green);
        assert_eq!(t.model_color("o1-preview"), green);
    }

    #[test]
    fn model_color_google() {
        let t = Theme::default();
        assert_eq!(t.model_color("gemini-2.5-pro"), Color::Rgb(59, 130, 246));
    }

    #[test]
    fn model_color_deepseek() {
        let t = Theme::default();
        assert_eq!(t.model_color("deepseek-v3-0324"), Color::Rgb(6, 182, 212));
    }

    #[test]
    fn model_color_nvidia() {
        let t = Theme::default();
        assert_eq!(
            t.model_color("nvidia/nemotron-70b"),
            Color::Rgb(118, 185, 0)
        );
    }

    #[test]
    fn model_color_unknown_is_gray() {
        let t = Theme::default();
        assert_eq!(
            t.model_color("some-unknown-model"),
            Color::Rgb(200, 200, 200)
        );
    }

    #[test]
    fn provider_color_mapping() {
        let t = Theme::default();
        assert_eq!(t.provider_color("claude"), t.claude);
        assert_eq!(t.provider_color("codex"), t.codex);
        assert_eq!(t.provider_color("copilot"), t.copilot);
        assert_eq!(t.provider_color("gemini"), t.gemini);
        assert_eq!(t.provider_color("unknown"), t.fg);
    }

    #[test]
    fn gauge_color_thresholds() {
        let t = Theme::default();
        assert_eq!(t.gauge_color(10.0), t.gauge_low);
        assert_eq!(t.gauge_color(60.0), t.gauge_mid);
        assert_eq!(t.gauge_color(90.0), t.gauge_high);
    }
}
