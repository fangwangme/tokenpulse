use std::io::Write;

/// Sends terminal bell and OSC 9 notification sequence to stdout.
pub fn send_terminal_notification(provider: &str, window_label: &str, remaining_percent: f64) {
    let mut stdout = std::io::stdout();
    // BEL chime
    let _ = stdout.write_all(b"\x07");
    // OSC 9 notification
    let osc9 = format!(
        "\x1b]9;TokenPulse: {} {} quota restored ({:.0}% remaining)\x1b\\",
        provider, window_label, remaining_percent
    );
    let _ = stdout.write_all(osc9.as_bytes());
    let _ = stdout.flush();
}

/// Sends macOS native system notification center alert with sound.
///
/// Spawns a background thread so it never blocks the caller or TUI loop.
pub fn send_system_notification(provider: &str, window_label: &str, remaining_percent: f64) {
    let provider = provider.to_string();
    let window_label = window_label.to_string();
    std::thread::spawn(move || {
        #[cfg(target_os = "macos")]
        {
            let msg = format!(
                "{} {} quota restored ({:.0}% remaining)",
                provider, window_label, remaining_percent
            );
            let script = format!(
                "display notification \"{}\" with title \"TokenPulse\" subtitle \"Quota Restored\" sound name \"default\"",
                msg.replace('\\', "\\\\").replace('"', "\\\"")
            );
            let _ = std::process::Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .output();
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (provider, window_label, remaining_percent);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_send_terminal_notification_does_not_panic() {
        send_terminal_notification("claude", "5h", 100.0);
    }

    #[test]
    fn test_send_system_notification_does_not_panic() {
        send_system_notification("claude", "5h", 100.0);
    }
}
