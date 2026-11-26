# Notification

Quota-recovery notifications fire when a window that was exhausted returns
below 100%. One batch of recoveries produces one sound and one banner.

Levels: `off` (nothing), `in_app` (sound plus the TUI toast and animation),
`terminal` (adds the terminal bell and an OSC 9 notification), `system` (adds a
macOS Notification Center banner). Every level except `off` plays the sound.

Text: one recovery is `<provider> <window> quota restored (<n>% remaining)`
with the percentage rounded; several are `<n> quota windows restored`.

Sound is `chime` (the embedded `quota-restored.wav`), `none`, or the base name
of a file under `/System/Library/Sounds`. On macOS it is played with `afplay`
(the configured system sound, else the built-in chime); elsewhere the terminal
bell is written to stdout. The terminal notification writes BEL and
`ESC ] 9 ; TokenPulse: <text> ESC \`. The system banner runs `osascript
display notification` with no sound of its own.
