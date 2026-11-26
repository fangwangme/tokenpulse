#!/usr/bin/env python3
"""Render tmux capture-pane ANSI output as a standalone HTML page."""

from __future__ import annotations

import argparse
import html
import re

PALETTE = [
    "#000000", "#cd0000", "#00cd00", "#cdcd00", "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
    "#7f7f7f", "#ff0000", "#00ff00", "#ffff00", "#5c5cff", "#ff00ff", "#00ffff", "#ffffff",
]


def xterm_color(index: int) -> str:
    if index < 16:
        return PALETTE[index]
    if index < 232:
        index -= 16
        red, green, blue = index // 36, (index % 36) // 6, index % 6
        return f"#{red * 51:02x}{green * 51:02x}{blue * 51:02x}"
    grey = 8 + (index - 232) * 10
    return f"#{grey:02x}{grey:02x}{grey:02x}"


def sgr(params: list[int], style: dict[str, str | bool | None]) -> None:
    if not params:
        params = [0]
    index = 0
    while index < len(params):
        code = params[index]
        if code == 0:
            style.update(fg=None, bg=None, bold=False, faint=False, italic=False, underline=False, reverse=False)
        elif code == 1:
            style["bold"] = True
        elif code == 2:
            style["faint"] = True
        elif code == 3:
            style["italic"] = True
        elif code == 4:
            style["underline"] = True
        elif code == 7:
            style["reverse"] = True
        elif code == 22:
            style["bold"] = False
            style["faint"] = False
        elif code == 23:
            style["italic"] = False
        elif code == 24:
            style["underline"] = False
        elif code == 27:
            style["reverse"] = False
        elif 30 <= code <= 37:
            style["fg"] = PALETTE[code - 30]
        elif code == 39:
            style["fg"] = None
        elif 40 <= code <= 47:
            style["bg"] = PALETTE[code - 40]
        elif code == 49:
            style["bg"] = None
        elif 90 <= code <= 97:
            style["fg"] = PALETTE[code - 90 + 8]
        elif 100 <= code <= 107:
            style["bg"] = PALETTE[code - 100 + 8]
        elif code in (38, 48):
            target = "fg" if code == 38 else "bg"
            if index + 1 < len(params) and params[index + 1] == 5 and index + 2 < len(params):
                style[target] = xterm_color(params[index + 2])
                index += 2
            elif index + 1 < len(params) and params[index + 1] == 2 and index + 4 < len(params):
                red, green, blue = params[index + 2:index + 5]
                style[target] = f"#{red:02x}{green:02x}{blue:02x}"
                index += 4
        index += 1


def css(style: dict[str, str | bool | None]) -> str:
    fg, bg = style["fg"], style["bg"]
    if style["reverse"]:
        fg, bg = bg, fg
    parts = [
        f"color:{fg or '#d7d7d7'}",
        f"background:{bg or 'transparent'}",
    ]
    if style["bold"]:
        parts.append("font-weight:700")
    if style["faint"]:
        parts.append("opacity:.7")
    if style["italic"]:
        parts.append("font-style:italic")
    if style["underline"]:
        parts.append("text-decoration:underline")
    return ";".join(parts)


def render(text: str) -> str:
    style: dict[str, str | bool | None] = {
        "fg": None, "bg": None, "bold": False, "faint": False,
        "italic": False, "underline": False, "reverse": False,
    }
    out: list[str] = []
    current = ""
    position = 0
    for match in re.finditer(r"\x1b\[([0-9;?]*)([A-Za-z])", text):
        chunk = text[position:match.start()]
        if chunk:
            value = html.escape(chunk)
            next_style = css(style)
            if next_style != current:
                if current:
                    out.append("</span>")
                out.append(f'<span style="{next_style}">')
                current = next_style
            out.append(value)
        if match.group(2) == "m" and not match.group(1).startswith("?"):
            params = [int(value) for value in match.group(1).split(";") if value]
            sgr(params, style)
        position = match.end()
    chunk = text[position:]
    if chunk:
        value = html.escape(chunk)
        next_style = css(style)
        if next_style != current:
            if current:
                out.append("</span>")
            out.append(f'<span style="{next_style}">')
            current = next_style
        out.append(value)
    if current:
        out.append("</span>")
    return "".join(out)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("input")
    parser.add_argument("output")
    parser.add_argument("--title", default="TokenPulse")
    args = parser.parse_args()
    with open(args.input, encoding="utf-8") as handle:
        body = render(handle.read())
    page = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>{html.escape(args.title)}</title>
<style>
html,body {{ margin:0; background:#0b0d14; }}
.terminal {{
  display:inline-block; padding:28px 30px; background:#0f1014; color:#d7d7d7;
  border:1px solid #24262e; border-radius:14px;
  font:15px/1.45 "JetBrains Mono","SFMono-Regular","Noto Sans Mono",monospace;
  font-variant-ligatures:none; white-space:pre; tab-size:4;
}}
</style></head><body><div class="terminal">{body}</div></body></html>"""
    with open(args.output, "w", encoding="utf-8") as handle:
        handle.write(page)


if __name__ == "__main__":
    main()
