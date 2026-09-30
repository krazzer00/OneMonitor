"""Generates the app icon and the status-coloured tray icons.

Usage: python scripts/gen_icons.py   (requires Pillow)
"""
from pathlib import Path
from PIL import Image, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
ICONS = ROOT / "src-tauri" / "icons"
ICONS.mkdir(parents=True, exist_ok=True)

SS = 4  # supersampling factor

STATUS = {
    "ok": (52, 211, 153),
    "warn": (251, 191, 36),
    "error": (248, 113, 113),
    "idle": (148, 163, 184),
}


def lerp(a, b, t):
    return tuple(int(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def app_icon(size: int) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # vertical gradient body
    grad = Image.new("RGBA", (s, s))
    gd = ImageDraw.Draw(grad)
    top, bottom = (58, 60, 70, 255), (20, 21, 26, 255)
    for y in range(s):
        gd.line([(0, y), (s, y)], fill=lerp(top, bottom, y / s))
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).rounded_rectangle(
        [0, 0, s - 1, s - 1], radius=int(s * 0.23), fill=255
    )
    img.paste(grad, (0, 0), mask)

    d = ImageDraw.Draw(img)
    # glass highlight
    hl = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    ImageDraw.Draw(hl).rounded_rectangle(
        [0, 0, s - 1, s - 1], radius=int(s * 0.23), outline=(255, 255, 255, 60), width=max(1, s // 64)
    )
    img = Image.alpha_composite(img, hl)
    d = ImageDraw.Draw(img)

    # ring
    c = s / 2
    r = s * 0.28
    w = int(s * 0.075)
    d.ellipse([c - r, c - r, c + r, c + r], outline=(236, 238, 245, 255), width=w)
    # pulse line through the ring
    pts = [
        (s * 0.16, c),
        (s * 0.38, c),
        (s * 0.45, c - s * 0.13),
        (s * 0.53, c + s * 0.13),
        (s * 0.60, c),
        (s * 0.84, c),
    ]
    d.line(pts, fill=(52, 211, 153, 255), width=int(s * 0.055), joint="curve")
    return img.resize((size, size), Image.LANCZOS)


def tray_icon(color, size=32) -> Image.Image:
    s = size * SS
    img = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    c = s / 2
    r = s * 0.40
    w = int(s * 0.12)
    d.ellipse([c - r, c - r, c + r, c + r], outline=(240, 242, 248, 255), width=w)
    glow = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    gr = s * 0.20
    ImageDraw.Draw(glow).ellipse([c - gr, c - gr, c + gr, c + gr], fill=color + (160,))
    glow = glow.filter(ImageFilter.GaussianBlur(s * 0.05))
    img = Image.alpha_composite(img, glow)
    d = ImageDraw.Draw(img)
    dr = s * 0.16
    d.ellipse([c - dr, c - dr, c + dr, c + dr], fill=color + (255,))
    return img.resize((size, size), Image.LANCZOS)


def main():
    big = app_icon(512)
    big.save(ICONS / "icon.png")
    app_icon(32).save(ICONS / "32x32.png")
    app_icon(128).save(ICONS / "128x128.png")
    app_icon(256).save(ICONS / "128x128@2x.png")
    sizes = [16, 20, 24, 32, 40, 48, 64, 128, 256]
    app_icon(256).save(ICONS / "icon.ico", sizes=[(n, n) for n in sizes])
    for name, col in STATUS.items():
        tray_icon(col, 32).save(ICONS / f"tray-{name}.png")


if __name__ == "__main__":
    main()
