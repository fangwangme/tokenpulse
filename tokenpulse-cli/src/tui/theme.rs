use ratatui::style::Color;
use tokenpulse_core::config::ThemePreference;
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
        if let Some(mode) = detect_terminal_background_via_osc11() {
            return mode;
        }

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

    pub fn from_preference(preference: ThemePreference) -> Self {
        match preference {
            ThemePreference::Auto => Self::auto(),
            ThemePreference::Dark => Self::new(ThemeMode::Dark),
            ThemePreference::Light => Self::new(ThemeMode::Light),
        }
    }

    pub fn new(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::dark(),
            ThemeMode::Light => Self::light(),
        }
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
                Color::Rgb(184, 236, 255),
                Color::Rgb(128, 224, 255),
                Color::Rgb(32, 190, 255),
                Color::Rgb(0, 148, 209),
                Color::Rgb(0, 92, 134),
            ],
            cost_heatmap: [
                Color::Rgb(155, 233, 168),
                Color::Rgb(64, 196, 99),
                Color::Rgb(48, 161, 78),
                Color::Rgb(33, 110, 57),
                Color::Rgb(14, 68, 41),
            ],
            count_heatmap: [
                Color::Rgb(184, 236, 255),
                Color::Rgb(128, 224, 255),
                Color::Rgb(32, 190, 255),
                Color::Rgb(0, 148, 209),
                Color::Rgb(0, 92, 134),
            ],
            cache_heatmap: [
                Color::Rgb(13, 148, 136),
                Color::Rgb(15, 118, 110),
                Color::Rgb(17, 94, 89),
                Color::Rgb(19, 78, 74),
                Color::Rgb(20, 64, 60),
            ],
            heatmap_bg: Color::Rgb(255, 255, 255),
            heatmap_border: Color::Rgb(71, 85, 105),
            empty_heatmap: Color::Rgb(235, 237, 240),
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
                Color::Rgb(184, 236, 255),
                Color::Rgb(128, 224, 255),
                Color::Rgb(32, 190, 255),
                Color::Rgb(0, 148, 209),
                Color::Rgb(0, 92, 134),
            ],
            cost_heatmap: [
                Color::Rgb(155, 233, 168),
                Color::Rgb(64, 196, 99),
                Color::Rgb(48, 161, 78),
                Color::Rgb(33, 110, 57),
                Color::Rgb(14, 68, 41),
            ],
            count_heatmap: [
                Color::Rgb(184, 236, 255),
                Color::Rgb(128, 224, 255),
                Color::Rgb(32, 190, 255),
                Color::Rgb(0, 148, 209),
                Color::Rgb(0, 92, 134),
            ],
            cache_heatmap: [
                Color::Rgb(13, 148, 136),
                Color::Rgb(15, 118, 110),
                Color::Rgb(17, 94, 89),
                Color::Rgb(19, 78, 74),
                Color::Rgb(20, 64, 60),
            ],
            heatmap_bg: Color::Rgb(255, 255, 255),
            heatmap_border: Color::Rgb(15, 23, 42),
            empty_heatmap: Color::Rgb(235, 237, 240),
            selected_bg: Color::Rgb(203, 213, 225),
            today_bg: Color::Rgb(196, 181, 253),
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

pub fn theme_status_label(preference: ThemePreference, resolved: ThemeMode) -> String {
    match preference {
        ThemePreference::Auto => format!("auto -> {}", resolved.label()),
        ThemePreference::Dark => ThemePreference::Dark.label().to_string(),
        ThemePreference::Light => ThemePreference::Light.label().to_string(),
    }
}

fn parse_ansi_color_index(value: &str) -> Option<u8> {
    value.parse::<u8>().ok()
}

fn detect_system_theme() -> Option<ThemeMode> {
    ThemeMode::detect_from_macos()
}

#[cfg(unix)]
fn detect_terminal_background_via_osc11() -> Option<ThemeMode> {
    use std::io::Write;
    use std::time::{Duration, Instant};

    let stdin_fd = libc::STDIN_FILENO;
    let stdout_fd = libc::STDOUT_FILENO;
    if unsafe { libc::isatty(stdin_fd) } != 1 || unsafe { libc::isatty(stdout_fd) } != 1 {
        return None;
    }

    let flags = unsafe { libc::fcntl(stdin_fd, libc::F_GETFL) };
    if flags < 0 {
        return None;
    }

    if unsafe { libc::fcntl(stdin_fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return None;
    }
    let _guard = NonBlockingStdinGuard {
        fd: stdin_fd,
        flags,
    };

    let mut stdout = std::io::stdout();
    stdout.write_all(b"\x1b]11;?\x07").ok()?;
    stdout.flush().ok()?;

    let started = Instant::now();
    let timeout = Duration::from_millis(120);
    let mut response = Vec::with_capacity(128);
    let mut buffer = [0_u8; 64];

    while started.elapsed() < timeout {
        let read = unsafe {
            libc::read(
                stdin_fd,
                buffer.as_mut_ptr().cast::<libc::c_void>(),
                buffer.len(),
            )
        };

        if read > 0 {
            response.extend_from_slice(&buffer[..read as usize]);
            if let Some(mode) = parse_osc11_theme_response(&response) {
                return Some(mode);
            }
            if response.len() > 512 {
                break;
            }
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    parse_osc11_theme_response(&response)
}

#[cfg(unix)]
struct NonBlockingStdinGuard {
    fd: libc::c_int,
    flags: libc::c_int,
}

#[cfg(unix)]
impl Drop for NonBlockingStdinGuard {
    fn drop(&mut self) {
        unsafe {
            libc::fcntl(self.fd, libc::F_SETFL, self.flags);
        }
    }
}

#[cfg(not(unix))]
fn detect_terminal_background_via_osc11() -> Option<ThemeMode> {
    None
}

fn parse_osc11_theme_response(response: &[u8]) -> Option<ThemeMode> {
    let text = String::from_utf8_lossy(response);
    let marker = "]11;";
    let start = text.find(marker)? + marker.len();
    let payload = &text[start..];
    let end = payload
        .find(|ch| ch == '\x07' || ch == '\x1b')
        .unwrap_or(payload.len());
    theme_from_osc_color(&payload[..end])
}

fn theme_from_osc_color(value: &str) -> Option<ThemeMode> {
    let rgb = value
        .strip_prefix("rgb:")
        .or_else(|| value.strip_prefix("rgba:"))?;
    let mut parts = rgb.split('/');
    let r = parse_osc_color_component(parts.next()?)?;
    let g = parse_osc_color_component(parts.next()?)?;
    let b = parse_osc_color_component(parts.next()?)?;
    let luminance = 0.2126 * f64::from(r) + 0.7152 * f64::from(g) + 0.0722 * f64::from(b);
    if luminance < 128.0 {
        Some(ThemeMode::Dark)
    } else {
        Some(ThemeMode::Light)
    }
}

fn parse_osc_color_component(value: &str) -> Option<u8> {
    let digits: String = value
        .chars()
        .take_while(|ch| ch.is_ascii_hexdigit())
        .take(4)
        .collect();
    if digits.is_empty() {
        return None;
    }

    let raw = u32::from_str_radix(&digits, 16).ok()?;
    let max = (1_u32 << (digits.len() * 4)) - 1;
    Some(((raw * 255 + max / 2) / max) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(color: Color) -> (u8, u8, u8) {
        let Color::Rgb(r, g, b) = color else {
            panic!("expected RGB color");
        };
        (r, g, b)
    }

    fn relative_luminance(color: Color) -> f64 {
        fn channel(value: u8) -> f64 {
            let value = f64::from(value) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        }

        let (r, g, b) = rgb(color);
        0.2126 * channel(r) + 0.7152 * channel(g) + 0.0722 * channel(b)
    }

    fn contrast_ratio(a: Color, b: Color) -> f64 {
        let a = relative_luminance(a);
        let b = relative_luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn heatmap_palettes(theme: &Theme) -> [&[Color; 5]; 4] {
        [
            &theme.token_heatmap,
            &theme.cost_heatmap,
            &theme.count_heatmap,
            &theme.cache_heatmap,
        ]
    }

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
    fn dark_theme_uses_light_heatmap_surface_for_readable_levels() {
        let t = Theme::new(ThemeMode::Dark);
        assert_eq!(t.heatmap_bg, Color::Rgb(255, 255, 255));
        assert_eq!(t.empty_heatmap, Color::Rgb(235, 237, 240));
    }

    #[test]
    fn heatmap_metric_palettes_keep_expected_hues() {
        for theme in [Theme::new(ThemeMode::Dark), Theme::new(ThemeMode::Light)] {
            for color in theme.token_heatmap.into_iter().chain(theme.count_heatmap) {
                let (r, g, b) = rgb(color);
                assert!(
                    b > g && g > r,
                    "{} token/count heatmap should stay Kaggle blue: {:?}",
                    theme.mode.label(),
                    color
                );
            }
            for color in theme.cost_heatmap {
                let (r, g, b) = rgb(color);
                assert!(
                    g > r && g > b,
                    "{} cost heatmap should stay green: {:?}",
                    theme.mode.label(),
                    color
                );
            }
        }
    }

    #[test]
    fn heatmap_palettes_match_between_light_and_dark_themes() {
        let dark = Theme::new(ThemeMode::Dark);
        let light = Theme::new(ThemeMode::Light);
        assert_eq!(dark.token_heatmap, light.token_heatmap);
        assert_eq!(dark.cost_heatmap, light.cost_heatmap);
        assert_eq!(dark.count_heatmap, light.count_heatmap);
        assert_eq!(dark.cache_heatmap, light.cache_heatmap);
    }

    #[test]
    fn heatmap_uses_platform_inspired_palettes() {
        let t = Theme::new(ThemeMode::Light);
        assert_eq!(t.cost_heatmap[0], Color::Rgb(155, 233, 168));
        assert_eq!(t.cost_heatmap[4], Color::Rgb(14, 68, 41));
        assert_eq!(t.token_heatmap[0], Color::Rgb(184, 236, 255));
        assert_eq!(t.token_heatmap[2], Color::Rgb(32, 190, 255));
        assert_eq!(t.count_heatmap[0], Color::Rgb(184, 236, 255));
        assert_eq!(t.count_heatmap[2], Color::Rgb(32, 190, 255));
    }

    #[test]
    fn heatmap_lowest_level_is_visible_in_light_and_dark_themes() {
        for theme in [Theme::new(ThemeMode::Dark), Theme::new(ThemeMode::Light)] {
            for palette in heatmap_palettes(&theme) {
                assert!(
                    contrast_ratio(palette[0], theme.heatmap_bg) >= 1.25,
                    "{} heatmap low level has insufficient contrast: {:?}",
                    theme.mode.label(),
                    palette[0]
                );
                assert!(
                    contrast_ratio(palette[0], theme.heatmap_bg)
                        > contrast_ratio(theme.empty_heatmap, theme.heatmap_bg),
                    "{} heatmap low level should be clearer than empty cells",
                    theme.mode.label()
                );
            }
        }
    }

    #[test]
    fn heatmap_levels_increase_contrast_in_light_and_dark_themes() {
        for theme in [Theme::new(ThemeMode::Dark), Theme::new(ThemeMode::Light)] {
            for palette in heatmap_palettes(&theme) {
                let contrasts = palette.map(|color| contrast_ratio(color, theme.heatmap_bg));
                for pair in contrasts.windows(2) {
                    assert!(
                        pair[1] > pair[0],
                        "{} heatmap levels should progress visually: {:?}",
                        theme.mode.label(),
                        contrasts
                    );
                }
            }
        }
    }

    #[test]
    fn light_theme_today_highlight_stays_visible() {
        let t = Theme::new(ThemeMode::Light);
        assert_eq!(t.today_bg, Color::Rgb(196, 181, 253));
    }

    #[test]
    fn osc11_response_detects_dark_background() {
        assert_eq!(
            parse_osc11_theme_response(b"\x1b]11;rgb:0000/0000/0000\x1b\\"),
            Some(ThemeMode::Dark)
        );
    }

    #[test]
    fn osc11_response_detects_light_background() {
        assert_eq!(
            parse_osc11_theme_response(b"\x1b]11;rgb:ffff/ffff/ffff\x07"),
            Some(ThemeMode::Light)
        );
    }

    #[test]
    fn osc_color_component_scales_short_hex_values() {
        assert_eq!(parse_osc_color_component("0"), Some(0));
        assert_eq!(parse_osc_color_component("f"), Some(255));
        assert_eq!(parse_osc_color_component("80"), Some(128));
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
