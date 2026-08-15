#!/usr/bin/env python3
"""Regenerates `quota-restored.wav`, the built-in quota recovery chime.

The asset is committed to the repo and embedded into the binary with
`include_bytes!`, so this script only needs to run when the sound changes.

Design notes: three additively synthesised bell partial stacks (A5 -> E6 -> A6)
with a fast attack and exponential decay. Loudness is normalised to
-23.9 dBFS RMS, which is roughly 11 dB hotter than macOS `Ping.aiff` and
matches the loudness of the notification sounds shipped by comparable
terminal tools — quiet system sounds are easy to miss over headphones.

Usage: python3 generate_chime.py
"""

import array
import math
import os
import wave

SAMPLE_RATE = 48_000
DURATION_SECS = 1.08
TARGET_RMS_DBFS = -23.9
FADE_OUT_SECS = 0.04

# (frequency, duration, start offset, amplitude)
PARTIALS = [
    (880.0, 0.62, 0.00, 1.00),  # A5
    (1318.5, 0.86, 0.16, 0.95),  # E6
    (1760.0, 0.70, 0.30, 0.45),  # A6 shimmer
]

# Relative amplitude of each harmonic above the fundamental. The slightly
# detuned third partial (3.01x) is what gives the tone its bell character.
HARMONICS = [(1.0, 1.00), (2.0, 0.42), (3.01, 0.18), (4.2, 0.08)]


def render() -> list[float]:
    total = int(DURATION_SECS * SAMPLE_RATE)
    buf = [0.0] * total

    for freq, dur, start, amp in PARTIALS:
        offset = int(start * SAMPLE_RATE)
        for i in range(int(dur * SAMPLE_RATE)):
            idx = offset + i
            if idx >= total:
                break
            t = i / SAMPLE_RATE
            # Exponential decay with a short attack ramp so the onset is
            # percussive but click-free.
            envelope = math.exp(-t * 4.2) * (1.0 - math.exp(-t * 400.0))
            sample = sum(
                math.sin(2.0 * math.pi * freq * mult * t) * level
                for mult, level in HARMONICS
            )
            buf[idx] += sample * envelope * amp

    # Fade the tail to zero so the file never ends on a discontinuity.
    fade = int(FADE_OUT_SECS * SAMPLE_RATE)
    for i in range(fade):
        buf[total - fade + i] *= 1.0 - (i / fade)

    return buf


def normalise(buf: list[float]) -> list[float]:
    peak = max(abs(x) for x in buf)
    buf = [x / peak for x in buf]
    rms = math.sqrt(sum(x * x for x in buf) / len(buf))
    # Take the smaller of "hit the target RMS" and "stay below full scale" so
    # boosting loudness can never clip.
    gain = min(10 ** (TARGET_RMS_DBFS / 20) / rms, 1.0 / max(abs(x) for x in buf))
    return [max(-1.0, min(1.0, x * gain)) for x in buf]


def main() -> None:
    buf = normalise(render())
    samples = array.array("h", (int(x * 32767) for x in buf))

    out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "quota-restored.wav")
    with wave.open(out, "wb") as w:
        w.setnchannels(1)
        w.setsampwidth(2)
        w.setframerate(SAMPLE_RATE)
        w.writeframes(samples.tobytes())

    rms = math.sqrt(sum(float(x) * x for x in samples) / len(samples)) / 32768
    peak = max(abs(x) for x in samples) / 32768
    print(
        f"wrote {out} "
        f"({len(samples) / SAMPLE_RATE:.2f}s, peak {peak:.2f}, "
        f"RMS {20 * math.log10(rms):.1f} dBFS, {os.path.getsize(out)} bytes)"
    )


if __name__ == "__main__":
    main()
