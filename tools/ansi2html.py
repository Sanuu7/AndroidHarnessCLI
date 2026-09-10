#!/usr/bin/env python3
"""Render an ANSI frame dump (see `harness --dump`) as a standalone HTML page.

Used to eyeball the TUI's colors in a browser when there is no terminal handy.
Not part of the shipped binary.
"""

import html
import re
import sys

SGR = re.compile(r"\x1b\[([0-9;]*)m")

CUBE = [0, 95, 135, 175, 215, 255]


def xterm256(n: int):
    if n < 16:
        base = [
            (0, 0, 0),
            (205, 0, 0),
            (0, 205, 0),
            (205, 205, 0),
            (0, 0, 238),
            (205, 0, 205),
            (0, 205, 205),
            (229, 229, 229),
            (127, 127, 127),
            (255, 0, 0),
            (0, 255, 0),
            (255, 255, 0),
            (92, 92, 255),
            (255, 0, 255),
            (0, 255, 255),
            (255, 255, 255),
        ]
        return base[n]
    if n < 232:
        n -= 16
        return (CUBE[n // 36], CUBE[(n // 6) % 6], CUBE[n % 6])
    v = 8 + (n - 232) * 10
    return (v, v, v)


def rgb_of(params, i):
    if params[i] == "2" and i + 3 < len(params) + 1:
        return tuple(int(params[i + 1 + k]) for k in range(3))
    if params[i] == "5":
        return xterm256(int(params[i + 1]))
    return None


def convert(src: str, bg: str = "#0d1117") -> str:
    out = [
        "<!doctype html><meta charset='utf-8'>",
        "<style>",
        f"body{{background:{bg};margin:0;padding:24px}}",
        "pre{font:14px/1.0 'JetBrains Mono','DejaVu Sans Mono',monospace;"
        "white-space:pre;letter-spacing:0;margin:0}",
        "span{display:inline}",
        "</style><pre>",
    ]
    fg = (226, 232, 240)
    fg_css = None
    style_mods = set()
    i = 0
    line_start = True
    while i < len(src):
        m = SGR.match(src, i)
        if m:
            params = [p for p in m.group(1).split(";") if p != ""] or ["0"]
            j = 0
            while j < len(params):
                p = params[j]
                if p == "0":
                    fg_css, style_mods = None, set()
                elif p == "1":
                    style_mods.add("b")
                elif p == "2":
                    style_mods.add("dim")
                elif p == "3":
                    style_mods.add("i")
                elif p == "4":
                    style_mods.add("u")
                elif p == "7":
                    style_mods.add("rev")
                elif p in ("38", "48") and j + 1 < len(params):
                    rgb = rgb_of(params, j + 1)
                    if rgb:
                        if p == "38":
                            fg_css = rgb
                        j += 4 if params[j + 1] == "2" else 2
                j += 1
            i = m.end()
            continue
        ch = src[i]
        if ch == "\n":
            if style_mods or fg_css:
                out.append("</span>")
                style_mods, fg_css = set(), None
            out.append("\n")
            i += 1
            continue
        css = []
        if fg_css:
            css.append(f"color:rgb({fg_css[0]},{fg_css[1]},{fg_css[2]})")
        else:
            css.append(f"color:rgb({fg[0]},{fg[1]},{fg[2]})")
        if "b" in style_mods:
            css.append("font-weight:700")
        if "dim" in style_mods:
            css.append("opacity:.6")
        if "i" in style_mods:
            css.append("font-style:italic")
        if "u" in style_mods:
            css.append("text-decoration:underline")
        if "rev" in style_mods:
            css.append("background:rgb(226,232,240);color:#0d1117")
        # Group consecutive same-style chars.
        start = i
        buf = []
        while i < len(src) and not SGR.match(src, i) and src[i] != "\n":
            buf.append(src[i])
            i += 1
        text = html.escape("".join(buf))
        out.append(f"<span style='{';'.join(css)}'>{text}</span>")
        del start
        line_start = False
    out.append("</pre>")
    return "".join(out)


if __name__ == "__main__":
    data = sys.stdin.read()
    bg = sys.argv[1] if len(sys.argv) > 1 else "#0d1117"
    sys.stdout.write(convert(data, bg))
