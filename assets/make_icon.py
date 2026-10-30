"""Generate assets/icon.png (256x256 RGBA) and assets/icon.ico.

Run: uv run --with pillow python assets/make_icon.py
Flat design: light rounded square with a thin border, a blue sphere whose right half is a darker shade.
"""
from pathlib import Path

from PIL import Image, ImageDraw

BG = (0xF0, 0xF0, 0xF0, 255)
BORDER = (0xA8, 0xA8, 0xA8, 255)
LIGHT = (0x00, 0x53, 0x7D, 255)
DARK = (0x00, 0x34, 0x4F, 255)
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
SS = 4  # supersampling factor


def render(size: int) -> Image.Image:
    n = size * SS
    u = n / 32.0  # one design unit (the icon is designed on a 32 grid)
    radius = 2 * u
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    d = ImageDraw.Draw(img)
    d.rounded_rectangle([0, 0, n - 1, n - 1], radius=radius, fill=BORDER)
    d.rounded_rectangle([u, u, n - 1 - u, n - 1 - u], radius=max(radius - u, 0), fill=BG)

    diameter = n * 0.62
    c = n / 2
    box = [c - diameter / 2, c - diameter / 2, c + diameter / 2, c + diameter / 2]
    d.ellipse(box, fill=LIGHT)
    # Shadow side: the right half of the circle, as a flat overlay.
    mask = Image.new("L", (n, n), 0)
    md = ImageDraw.Draw(mask)
    md.ellipse(box, fill=255)
    md.rectangle([0, 0, round(c) - 1, n], fill=0)
    img.paste(Image.new("RGBA", (n, n), DARK), (0, 0), mask)
    return img.resize((size, size), Image.LANCZOS)


def main() -> None:
    out = Path(__file__).resolve().parent
    png = render(256)
    png.save(out / "icon.png")
    # Pillow resamples the base image itself, so hand it each size as an explicit append.
    imgs = [render(s) for s in ICO_SIZES]
    imgs[-1].save(out / "icon.ico", format="ICO", sizes=[(s, s) for s in ICO_SIZES],
                  append_images=imgs[:-1])


if __name__ == "__main__":
    main()
