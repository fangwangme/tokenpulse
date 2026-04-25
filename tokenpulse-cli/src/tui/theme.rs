use ratatui::style::Color;
use tokenpulse_core::usage::detect_provider_from_model;

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub accent_soft: Color,

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
            bg: Color::Rgb(9, 12, 18),
            fg: Color::Rgb(229, 236, 246),
            dim: Color::Rgb(112, 127, 151),
            border: Color::Rgb(42, 59, 77),
            accent: Color::Rgb(45, 212, 191),
            accent_soft: Color::Rgb(125, 211, 252),

            claude: Color::Rgb(251, 146, 60),
            codex: Color::Rgb(52, 211, 153),
            opencode: Color::Rgb(129, 140, 248),
            gemini: Color::Rgb(96, 165, 250),
            pi: Color::Rgb(244, 114, 182),
            antigravity: Color::Rgb(192, 132, 252),
            copilot: Color::Rgb(163, 230, 53),

            gauge_low: Color::Rgb(52, 211, 153),
            gauge_mid: Color::Rgb(250, 204, 21),
            gauge_high: Color::Rgb(248, 113, 113),
            token_heatmap: [
                Color::Rgb(22, 78, 99),
                Color::Rgb(14, 116, 144),
                Color::Rgb(8, 145, 178),
                Color::Rgb(6, 182, 212),
                Color::Rgb(103, 232, 249),
            ],
            cost_heatmap: [
                Color::Rgb(76, 29, 149),
                Color::Rgb(109, 40, 217),
                Color::Rgb(147, 51, 234),
                Color::Rgb(192, 132, 252),
                Color::Rgb(216, 180, 254),
            ],
            count_heatmap: [
                Color::Rgb(120, 53, 15),
                Color::Rgb(180, 83, 9),
                Color::Rgb(217, 119, 6),
                Color::Rgb(245, 158, 11),
                Color::Rgb(252, 211, 77),
            ],
            empty_heatmap: Color::Rgb(24, 31, 42),
        }
    }
}

impl Theme {
    pub fn provider_color(&self, source: &str) -> Color {
        match source {
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

    pub fn company_key_for(&self, model: &str, provider_id: Option<&str>) -> &'static str {
        match detect_provider_from_model(model).as_str() {
            "openai" => "openai",
            "google" => "google",
            "anthropic" => "anthropic",
            _ => provider_id.map(normalize_company_key).unwrap_or("other"),
        }
    }

    pub fn company_color(&self, company: &str) -> Color {
        match company {
            "openai" => Color::Rgb(52, 211, 153),
            "google" => Color::Rgb(96, 165, 250),
            "anthropic" => Color::Rgb(251, 146, 60),
            _ => Color::Rgb(203, 213, 225),
        }
    }

    pub fn model_color_for(&self, model: &str, provider_id: Option<&str>) -> Color {
        self.company_color(self.company_key_for(model, provider_id))
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
        let coral = Color::Rgb(251, 146, 60);
        assert_eq!(t.model_color_for("claude-sonnet-4-20250514", None), coral);
        assert_eq!(t.model_color_for("claude-3.5-haiku", None), coral);
        assert_eq!(t.model_color_for("claude-opus-4", None), coral);
        assert_eq!(t.model_color_for("sonnet-4", None), coral);
    }

    #[test]
    fn model_color_openai_variants() {
        let t = Theme::default();
        let green = Color::Rgb(52, 211, 153);
        assert_eq!(t.model_color_for("gpt-4.1", None), green);
        assert_eq!(t.model_color_for("o3-mini", None), green);
        assert_eq!(t.model_color_for("o4-mini", None), green);
        assert_eq!(t.model_color_for("o1-preview", None), green);
    }

    #[test]
    fn model_color_google() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("gemini-2.5-pro", None),
            Color::Rgb(96, 165, 250)
        );
    }

    #[test]
    fn model_color_deepseek() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("deepseek-v3-0324", None),
            Color::Rgb(203, 213, 225)
        );
    }

    #[test]
    fn model_color_nvidia() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("nvidia/nemotron-70b", None),
            Color::Rgb(203, 213, 225)
        );
    }

    #[test]
    fn model_color_unknown_is_white() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("some-unknown-model", None),
            Color::Rgb(203, 213, 225)
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
    fn company_color_mapping() {
        let t = Theme::default();
        assert_eq!(t.company_color("openai"), Color::Rgb(52, 211, 153));
        assert_eq!(t.company_color("anthropic"), Color::Rgb(251, 146, 60));
        assert_eq!(t.company_color("other"), Color::Rgb(203, 213, 225));
    }

    #[test]
    fn company_key_prefers_model_family_over_provider() {
        let t = Theme::default();
        assert_eq!(t.company_key_for("gpt-4.1", Some("github")), "openai");
        assert_eq!(
            t.company_key_for("claude-sonnet-4", Some("openrouter")),
            "anthropic"
        );
    }

    #[test]
    fn gauge_color_thresholds() {
        let t = Theme::default();
        assert_eq!(t.gauge_color(10.0), t.gauge_low);
        assert_eq!(t.gauge_color(60.0), t.gauge_mid);
        assert_eq!(t.gauge_color(90.0), t.gauge_high);
    }
}

fn normalize_company_key(provider: &str) -> &'static str {
    let provider = provider.trim().to_ascii_lowercase();
    match provider.as_str() {
        value if value.contains("openai") => "openai",
        value if value.contains("google") => "google",
        value if value.contains("anthropic") => "anthropic",
        _ => "other",
    }
}
