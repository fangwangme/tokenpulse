//! Quota recovery notifications: sound, terminal escape sequences, and native
//! system banners.
//!
//! Every level that shows the user something also makes a sound. Audibility is
//! the whole point of the feature, and the two mechanisms that look obvious —
//! the terminal bell and AppleScript's `sound name` — are both silently
//! swallowed in common setups (Ghostty ships `bell-features = no-audio` by
//! default; `sound name` needs a file that actually exists under
//! `Library/Sounds`). So sound always goes through `afplay`, which depends on
//! neither the terminal's bell settings nor Notification Center permissions.

use crate::config::NotificationLevel;
use std::io::Write;
use std::path::PathBuf;
use tracing::{debug, warn};

/// The built-in recovery chime, embedded so there is no runtime asset
/// dependency. Regenerate with `tokenpulse-core/assets/generate_chime.py`.
const CHIME_WAV: &[u8] = include_bytes!("../../assets/quota-restored.wav");
const CHIME_FILE_NAME: &str = "tokenpulse-quota-restored.wav";

/// Config value selecting the built-in chime.
pub const SOUND_CHIME: &str = "chime";
/// Config value muting sound while leaving the visuals intact.
pub const SOUND_NONE: &str = "none";

/// A rate window that just transitioned from exhausted back to available.
#[derive(Debug, Clone)]
pub struct QuotaRecovery {
    pub provider: String,
    pub window_label: String,
    pub remaining_percent: f64,
}

/// Announces one or more quota recoveries at the configured level.
///
/// Takes the whole batch rather than a single event so that several windows
/// resetting at once produce one sound and one banner instead of a burst.
pub fn notify_quota_restored(level: NotificationLevel, sound: &str, events: &[QuotaRecovery]) {
    if level == NotificationLevel::Off || events.is_empty() {
        return;
    }

    let message = summarize(events);
    debug!(
        "quota recovery notification: level={} sound={} message={}",
        level.label(),
        sound,
        message
    );

    // Sound is level-independent: if the user gets a notification, they hear it.
    play_alert_sound(sound);

    if matches!(
        level,
        NotificationLevel::Terminal | NotificationLevel::System
    ) {
        send_terminal_notification(&message);
    }
    if level == NotificationLevel::System {
        send_system_notification(&message);
    }
}

fn summarize(events: &[QuotaRecovery]) -> String {
    match events {
        [] => String::new(),
        [one] => format!(
            "{} {} quota restored ({:.0}% remaining)",
            one.provider, one.window_label, one.remaining_percent
        ),
        many => format!("{} quota windows restored", many.len()),
    }
}

/// Plays the alert sound on a background thread.
///
/// `sound` is either [`SOUND_CHIME`], [`SOUND_NONE`], or the base name of a
/// sound installed under `/System/Library/Sounds` (e.g. `Hero`).
pub fn play_alert_sound(sound: &str) {
    let sound = sound.trim().to_string();
    if sound.is_empty() || sound.eq_ignore_ascii_case(SOUND_NONE) {
        return;
    }

    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            let Some(path) = resolve_sound_path(&sound) else {
                return;
            };
            match std::process::Command::new("afplay").arg(&path).output() {
                Ok(out) if !out.status.success() => warn!(
                    "afplay {} failed ({}): {}",
                    path.display(),
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                Err(e) => warn!("failed to spawn afplay: {}", e),
                _ => {}
            }
        }
        // Elsewhere the terminal bell is the only portable option. It is
        // audible only if the terminal is configured to ring it.
        #[cfg(not(target_os = "macos"))]
        {
            let _ = sound;
            let mut stdout = std::io::stdout();
            let _ = stdout.write_all(b"\x07");
            let _ = stdout.flush();
        }
    });
}

/// Resolves a sound name to a playable path, falling back to the built-in
/// chime when a configured system sound does not exist.
#[cfg(target_os = "macos")]
fn resolve_sound_path(sound: &str) -> Option<PathBuf> {
    if !sound.eq_ignore_ascii_case(SOUND_CHIME) {
        let system = PathBuf::from(format!("/System/Library/Sounds/{sound}.aiff"));
        if system.is_file() {
            return Some(system);
        }
        warn!(
            "unknown notification sound '{}', using built-in chime",
            sound
        );
    }
    match materialize_chime() {
        Ok(path) => Some(path),
        Err(e) => {
            warn!("failed to write built-in chime: {}", e);
            None
        }
    }
}

/// Writes the embedded chime to a temp file so `afplay` has a path to open.
///
/// Reuses the file across calls, and writes through a unique temp name so two
/// concurrent plays can never observe a half-written file.
#[cfg(target_os = "macos")]
fn materialize_chime() -> std::io::Result<PathBuf> {
    let path = std::env::temp_dir().join(CHIME_FILE_NAME);
    if path
        .metadata()
        .is_ok_and(|m| m.len() == CHIME_WAV.len() as u64)
    {
        return Ok(path);
    }
    let staging =
        std::env::temp_dir().join(format!("{}.{}.tmp", CHIME_FILE_NAME, std::process::id()));
    std::fs::write(&staging, CHIME_WAV)?;
    std::fs::rename(&staging, &path)?;
    Ok(path)
}

/// Rings the terminal bell and emits an OSC 9 desktop notification.
///
/// The bell is kept for terminals that turn it into a visual cue or a dock
/// badge (Ghostty's `attention` feature); it is not relied on for sound.
pub fn send_terminal_notification(message: &str) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(b"\x07");
    let _ = stdout.write_all(format!("\x1b]9;TokenPulse: {message}\x1b\\").as_bytes());
    let _ = stdout.flush();
}

/// Shows a macOS Notification Center banner.
///
/// Spawns a background thread so it never blocks the TUI loop. No `sound name`
/// is requested — sound is handled by [`play_alert_sound`] so that it works
/// even when Notification Center is muted or unauthorized.
pub fn send_system_notification(message: &str) {
    let message = message.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "display notification \"{}\" with title \"TokenPulse\" subtitle \"Quota Restored\"",
                escape_applescript(&message)
            );
            match std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output()
            {
                Ok(out) if !out.status.success() => warn!(
                    "osascript notification failed ({}): {} — check System Settings > \
                     Notifications that your terminal is allowed to post notifications",
                    out.status,
                    String::from_utf8_lossy(&out.stderr).trim()
                ),
                Err(e) => warn!("failed to spawn osascript: {}", e),
                _ => {}
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = message;
        }
    });
}

fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovery(provider: &str, window: &str, remaining: f64) -> QuotaRecovery {
        QuotaRecovery {
            provider: provider.to_string(),
            window_label: window.to_string(),
            remaining_percent: remaining,
        }
    }

    #[test]
    fn summarizes_a_single_recovery_with_provider_and_window() {
        let msg = summarize(&[recovery("claude", "5h", 42.4)]);
        assert_eq!(msg, "claude 5h quota restored (42% remaining)");
    }

    #[test]
    fn summarizes_a_batch_as_a_count() {
        let msg = summarize(&[
            recovery("claude", "5h", 42.0),
            recovery("codex", "7d", 10.0),
        ]);
        assert_eq!(msg, "2 quota windows restored");
    }

    #[test]
    fn summarizes_an_empty_batch_as_empty() {
        assert_eq!(summarize(&[]), "");
    }

    #[test]
    fn escapes_quotes_and_backslashes_for_applescript() {
        assert_eq!(escape_applescript(r#"a"b\c"#), r#"a\"b\\c"#);
    }

    #[test]
    fn embedded_chime_is_a_riff_wave_file() {
        assert_eq!(&CHIME_WAV[0..4], b"RIFF");
        assert_eq!(&CHIME_WAV[8..12], b"WAVE");
        assert!(CHIME_WAV.len() > 10_000);
    }

    #[test]
    fn off_level_emits_nothing() {
        // No sound thread, no escape sequences written to stdout.
        notify_quota_restored(
            NotificationLevel::Off,
            SOUND_CHIME,
            &[recovery("claude", "5h", 42.0)],
        );
    }

    #[test]
    fn empty_batch_emits_nothing() {
        notify_quota_restored(NotificationLevel::System, SOUND_CHIME, &[]);
    }

    #[test]
    fn muted_sound_values_are_ignored() {
        play_alert_sound(SOUND_NONE);
        play_alert_sound("  ");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn resolves_known_system_sound_and_falls_back_otherwise() {
        assert_eq!(
            resolve_sound_path("Hero"),
            Some(PathBuf::from("/System/Library/Sounds/Hero.aiff"))
        );
        let chime = std::env::temp_dir().join(CHIME_FILE_NAME);
        assert_eq!(resolve_sound_path(SOUND_CHIME), Some(chime.clone()));
        // An unknown name must not go silent — it falls back to the chime.
        assert_eq!(resolve_sound_path("NoSuchSound"), Some(chime));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn materialized_chime_matches_the_embedded_bytes() {
        let path = materialize_chime().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), CHIME_WAV);
        // Second call reuses the existing file.
        assert_eq!(materialize_chime().unwrap(), path);
    }
}
