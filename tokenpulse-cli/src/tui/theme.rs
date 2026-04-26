use ratatui::style::Color;
use tokenpulse_core::usage::detect_provider_from_model;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    pub fn detect() -> Self {
        if let Ok(value) = std::env::var("TOKENPULSE_THEME") {
            return match value.to_ascii_lowercase().as_str() {
                "light" => Self::Light,
                "dark" => Self::Dark,
                _ => Self::detect_from_terminal(),
            };
        }

        Self::detect_from_terminal()
    }

    fn detect_from_terminal() -> Self {
        if let Ok(value) = std::env::var("COLORFGBG") {
            if let Some(bg) = value
                .split(';')
                .next_back()
                .and_then(parse_ansi_color_index)
            {
                return if bg <= 6 || bg == 8 {
                    Self::Dark
                } else {
                    Self::Light
                };
            }
        }

        if let Ok(value) = std::env::var("TERM_BACKGROUND") {
            return match value.to_ascii_lowercase().as_str() {
                "light" => Self::Light,
                "dark" => Self::Dark,
                _ => Self::detect_from_system(),
            };
        }

        Self::detect_from_system()
    }

    fn detect_from_system() -> Self {
        detect_system_theme().unwrap_or(Self::Dark)
    }

    #[cfg(target_os = "macos")]
    fn detect_from_macos() -> Option<Self> {
        let output = std::process::Command::new("defaults")
            .args(["read", "-g", "AppleInterfaceStyle"])
            .output()
            .ok()?;

        if output.status.success() {
            let value = String::from_utf8_lossy(&output.stdout);
            if value.trim().eq_ignore_ascii_case("dark") {
                return Some(Self::Dark);
            }
        }

        Some(Self::Light)
    }

    #[cfg(not(target_os = "macos"))]
    fn detect_from_macos() -> Option<Self> {
        None
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Light => Self::Dark,
            Self::Dark => Self::Light,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

pub struct Theme {
    pub mode: ThemeMode,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub border: Color,
    pub accent: Color,
    pub accent_soft: Color,
    pub on_accent: Color,

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
    pub cache_heatmap: [Color; 5],
    pub heatmap_bg: Color,
    pub heatmap_border: Color,
    pub empty_heatmap: Color,
    pub selected_bg: Color,
    pub today_bg: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::new(ThemeMode::Dark)
    }
}

impl Theme {
    pub fn auto() -> Self {
        Self::new(ThemeMode::detect())
    }

    pub fn new(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
    }

    pub fn toggled(&self) -> Self {
        Self::new(self.mode.toggle())
    }

    fn dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            bg: Color::Rgb(9, 12, 18),
            fg: Color::Rgb(232, 238, 247),
            dim: Color::Rgb(144, 158, 181),
            border: Color::Rgb(58, 76, 99),
            accent: Color::Rgb(45, 212, 191),
            accent_soft: Color::Rgb(125, 211, 252),
            on_accent: Color::Rgb(4, 18, 24),

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
                Color::Rgb(199, 240, 255),
                Color::Rgb(125, 211, 252),
                Color::Rgb(56, 189, 248),
                Color::Rgb(2, 132, 199),
                Color::Rgb(12, 74, 110),
            ],
            cost_heatmap: [
                Color::Rgb(192, 132, 252),
                Color::Rgb(168, 85, 247),
                Color::Rgb(147, 51, 234),
                Color::Rgb(109, 40, 217),
                Color::Rgb(76, 29, 149),
            ],
            count_heatmap: [
                Color::Rgb(252, 211, 77),
                Color::Rgb(245, 158, 11),
                Color::Rgb(217, 119, 6),
                Color::Rgb(180, 83, 9),
                Color::Rgb(120, 53, 15),
            ],
            cache_heatmap: [
                Color::Rgb(240, 217, 161),
                Color::Rgb(216, 180, 105),
                Color::Rgb(180, 137, 63),
                Color::Rgb(134, 95, 43),
                Color::Rgb(87, 60, 35),
            ],
            heatmap_bg: Color::Rgb(20, 28, 40),
            heatmap_border: Color::Rgb(71, 85, 105),
            empty_heatmap: Color::Rgb(24, 31, 42),
            selected_bg: Color::Rgb(51, 65, 85),
            today_bg: Color::Rgb(22, 78, 99),
        }
    }

    fn light() -> Self {
        Self {
            mode: ThemeMode::Light,
            bg: Color::Rgb(248, 250, 252),
            fg: Color::Rgb(15, 23, 42),
            dim: Color::Rgb(71, 85, 105),
            border: Color::Rgb(148, 163, 184),
            accent: Color::Rgb(13, 148, 136),
            accent_soft: Color::Rgb(3, 105, 161),
            on_accent: Color::Rgb(255, 255, 255),

            claude: Color::Rgb(194, 65, 12),
            codex: Color::Rgb(4, 120, 87),
            opencode: Color::Rgb(67, 56, 202),
            gemini: Color::Rgb(29, 78, 216),
            pi: Color::Rgb(190, 24, 93),
            antigravity: Color::Rgb(126, 34, 206),
            copilot: Color::Rgb(77, 124, 15),

            gauge_low: Color::Rgb(4, 120, 87),
            gauge_mid: Color::Rgb(180, 83, 9),
            gauge_high: Color::Rgb(185, 28, 28),
            token_heatmap: [
                Color::Rgb(199, 240, 255),
                Color::Rgb(125, 211, 252),
                Color::Rgb(56, 189, 248),
                Color::Rgb(2, 132, 199),
                Color::Rgb(12, 74, 110),
            ],
            cost_heatmap: [
                Color::Rgb(221, 214, 254),
                Color::Rgb(196, 181, 253),
                Color::Rgb(167, 139, 250),
                Color::Rgb(124, 58, 237),
                Color::Rgb(76, 29, 149),
            ],
            count_heatmap: [
                Color::Rgb(254, 240, 138),
                Color::Rgb(253, 186, 116),
                Color::Rgb(251, 146, 60),
                Color::Rgb(217, 119, 6),
                Color::Rgb(124, 45, 18),
            ],
            cache_heatmap: [
                Color::Rgb(253, 230, 138),
                Color::Rgb(245, 158, 11),
                Color::Rgb(180, 83, 9),
                Color::Rgb(146, 64, 14),
                Color::Rgb(120, 53, 15),
            ],
            heatmap_bg: Color::Rgb(248, 250, 252),
            heatmap_border: Color::Rgb(15, 23, 42),
            empty_heatmap: Color::Rgb(226, 232, 240),
            selected_bg: Color::Rgb(203, 213, 225),
            today_bg: Color::Rgb(220, 252, 231),
        }
    }

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
            _ => Color::Rgb(168, 85, 247),
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

fn parse_ansi_color_index(value: &str) -> Option<u8> {
    value.parse::<u8>().ok()
}

fn detect_system_theme() -> Option<ThemeMode> {
    ThemeMode::detect_from_macos()
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
            Color::Rgb(168, 85, 247)
        );
    }

    #[test]
    fn model_color_nvidia() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("nvidia/nemotron-70b", None),
            Color::Rgb(168, 85, 247)
        );
    }

    #[test]
    fn model_color_unknown_is_white() {
        let t = Theme::default();
        assert_eq!(
            t.model_color_for("some-unknown-model", None),
            Color::Rgb(168, 85, 247)
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
        assert_eq!(t.company_color("other"), Color::Rgb(168, 85, 247));
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

    #[test]
    fn dark_theme_uses_dark_heatmap_surfaces() {
        let t = Theme::new(ThemeMode::Dark);
        assert_eq!(t.heatmap_bg, Color::Rgb(20, 28, 40));
        assert_eq!(t.empty_heatmap, Color::Rgb(24, 31, 42));
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
