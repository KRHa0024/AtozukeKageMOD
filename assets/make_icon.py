"""Build the application PNG and multi-resolution Windows icon.

Run: uv run --with pillow python assets/make_icon.py

The source artwork is kept separately so the generated files can be rebuilt
without resampling an already-small icon.
"""
from pathlib import Path

from PIL import Image

ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]
CANVAS_SIZE = 256
PADDING = 12
ALPHA_CUTOFF = 16


def prepared_source(path: Path) -> Image.Image:
    """Crop ImageGen edge noise and fit the artwork to a square safe area."""
    source = Image.open(path).convert("RGBA")
    alpha = source.getchannel("A")
    visible = alpha.point(lambda value: 255 if value >= ALPHA_CUTOFF else 0)
    bounds = visible.getbbox()
    if bounds is None:
        raise ValueError(f"icon source has no visible pixels: {path}")

    source = source.crop(bounds)
    alpha = source.getchannel("A").point(lambda value: value if value >= ALPHA_CUTOFF else 0)
    source.putalpha(alpha)

    available = CANVAS_SIZE - PADDING * 2
    scale = min(available / source.width, available / source.height)
    fitted_size = (max(1, round(source.width * scale)), max(1, round(source.height * scale)))
    source = source.resize(fitted_size, Image.Resampling.LANCZOS)

    canvas = Image.new("RGBA", (CANVAS_SIZE, CANVAS_SIZE), (0, 0, 0, 0))
    offset = ((CANVAS_SIZE - source.width) // 2, (CANVAS_SIZE - source.height) // 2)
    canvas.alpha_composite(source, offset)
    return canvas


def main() -> None:
    out = Path(__file__).resolve().parent
    icon = prepared_source(out / "icon_source.png")
    icon.save(out / "icon.png", optimize=True)
    icon.save(out / "icon.ico", format="ICO", sizes=[(size, size) for size in ICO_SIZES])


if __name__ == "__main__":
    main()
