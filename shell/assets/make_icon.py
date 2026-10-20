#!/usr/bin/env python3
"""Browser Relay app icon: two variants, 256px masters."""
from PIL import Image, ImageDraw

S = 256
PURPLE_TOP = (154, 127, 245)
PURPLE_BOT = (106, 75, 216)
WHITE = (255, 255, 255)
INK = (20, 20, 22)


def rounded_bg(draw, radius, top, bot):
    for y in range(S):
        t = y / (S - 1)
        c = tuple(int(top[i] + (bot[i] - top[i]) * t) for i in range(3))
        draw.line([(0, y), (S, y)], fill=c + (255,))
    # round the corners by punching transparent
    mask = Image.new("L", (S, S), 0)
    md = ImageDraw.Draw(mask)
    md.rounded_rectangle([0, 0, S - 1, S - 1], radius=radius, fill=255)
    return mask


def variant_a():
    """Purple tile + white browser window + dark phone with play."""
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    mask = rounded_bg(d, 58, PURPLE_TOP, PURPLE_BOT)
    # browser window: white outline
    d.rounded_rectangle([44, 66, 182, 172], radius=18, outline=WHITE, width=20)
    d.line([(64, 100), (162, 100)], fill=WHITE, width=14, joint="curve")  # address bar
    # phone overlapping bottom-right
    d.rounded_rectangle([128, 116, 214, 212], radius=22, fill=INK + (255,))
    d.rounded_rectangle([128, 116, 214, 212], radius=22, outline=WHITE, width=9)
    # play triangle on the phone screen
    cx, cy = 171, 164
    d.polygon([(cx - 13, cy - 17), (cx - 13, cy + 17), (cx + 17, cy)], fill=(139, 114, 240, 255))
    img.putalpha(mask)
    return img


def variant_b():
    """Purple tile + white relay link (two interlocking rings)."""
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    mask = rounded_bg(d, 58, PURPLE_TOP, PURPLE_BOT)
    # two interlocked rings
    d.ellipse([52, 78, 152, 178], outline=WHITE, width=24)
    d.ellipse([104, 78, 204, 178], outline=WHITE, width=24)
    img.putalpha(mask)
    return img


def save_all(img, name):
    img.save(f"/home/hatch/workspace/browser-relay/shell/assets/icon-{name}-256.png")
    small = img.resize((64, 64), Image.LANCZOS)
    with open(f"/home/hatch/workspace/browser-relay/shell/assets/icon-{name}.rgba", "wb") as f:
        f.write(small.tobytes())
    # legibility preview at tray-ish sizes
    prev = Image.new("RGBA", (16 * 5 + 40, 64), (22, 22, 22, 255))
    for i, sz in enumerate([48, 32, 16]):
        r = img.resize((sz, sz), Image.LANCZOS)
        prev.alpha_composite(r, (20 + i * (48 + 20), (64 - sz) // 2))
    prev.save(f"/home/hatch/workspace/browser-relay/shell/assets/preview-{name}.png")


if __name__ == "__main__":
    for name, fn in [("a", variant_a), ("b", variant_b)]:
        save_all(fn(), name)
    print("done")
