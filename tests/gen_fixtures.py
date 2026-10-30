"""Generate regression fixtures for the Rust shadow-layer port from the Python reference.

Needs numpy and Pillow; the reference implementation is tests/reference/shading.py:

    uv run --with numpy --with pillow python tests/gen_fixtures.py tests/fixtures
    # real images (not committed): --original o.png --shaded s.png --cutout c.png <out_dir>

Synthetic inputs are drawn here so the committed fixtures contain no third-party images.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFilter

sys.path.insert(0, str(Path(__file__).parent / "reference"))
import shading  # noqa: E402

CASES = {
    # name: (shadow_layers kwargs, use mask, shaded at original size)
    "color_mask": ({"color": True, "smooth": 0.0}, True, False),
    "gray_nomask": ({"color": False, "smooth": 0.0}, False, False),
    "color_smooth_strong": ({"color": True, "smooth": 1.0, "strength": 1.3}, True, False),
    "color_samesize": ({"color": True, "smooth": 0.0}, True, True),
}


def synthetic(w: int, h: int, alpha: bool) -> tuple[Image.Image, Image.Image, Image.Image]:
    rng = np.random.default_rng(7)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    base = np.stack([0.55 + 0.4 * xx / w, 0.35 + 0.3 * yy / h, 0.7 - 0.3 * xx / w], -1)
    stripes = 0.08 * np.sin(xx / 3.0) * (yy > h * 0.6)  # fine detail ("hair strands")
    base = np.clip(base + stripes[..., None] + rng.normal(0, 0.01, base.shape), 0, 1)
    original = Image.fromarray((base * 255).astype(np.uint8), "RGB")
    d = ImageDraw.Draw(original)
    d.ellipse((w * 0.25, h * 0.15, w * 0.75, h * 0.55), fill=(240, 200, 210))
    d.rectangle((w * 0.3, h * 0.55, w * 0.7, h * 0.95), fill=(60, 50, 70))
    d.line((0, h * 0.3, w, h * 0.35), fill=(20, 20, 20), width=3)

    shade = np.ones((h, w), np.float32)
    shade -= 0.45 * ((xx - w * 0.6) ** 2 / (w * 0.2) ** 2 + (yy - h * 0.35) ** 2 / (h * 0.12) ** 2 < 1)
    shade -= 0.3 * (xx > w * 0.55) * (yy > h * 0.55)
    shade_img = Image.fromarray((np.clip(shade, 0, 1) * 255).astype(np.uint8), "L").filter(ImageFilter.GaussianBlur(4))
    tint = np.array([1.0, 0.93, 0.97], np.float32)
    s = np.asarray(original, np.float32) / 255.0 * (np.asarray(shade_img, np.float32)[..., None] / 255.0)
    s = s ** np.array([1.0, 1.02, 1.0]) * np.where(shade[..., None] < 1, tint, 1.0)
    shaded = Image.fromarray((np.clip(s * 0.97, 0, 1) * 255).astype(np.uint8), "RGB")
    # The model re-draws details slightly differently: shift the stripes a little.
    shaded = Image.fromarray(np.roll(np.asarray(shaded), 1, axis=1))

    cutout = Image.new("L", (w, h), 0)
    ImageDraw.Draw(cutout).ellipse((w * 0.15, h * 0.08, w * 0.85, h * 0.98), fill=255)
    if alpha:
        a = Image.new("L", (w, h), 255)
        ImageDraw.Draw(a).rectangle((0, 0, w * 0.1, h), fill=0)
        ImageDraw.Draw(a).rectangle((w * 0.1, 0, w * 0.15, h), fill=128)
        original = original.copy()
        original.putalpha(a)
    return original, shaded, cutout


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("out", type=Path)
    ap.add_argument("--original", type=Path)
    ap.add_argument("--shaded", type=Path)
    ap.add_argument("--cutout", type=Path, help="background removal result (RGBA) or mask (L)")
    args = ap.parse_args()
    out: Path = args.out
    out.mkdir(parents=True, exist_ok=True)

    if args.original:
        original, shaded_full = Image.open(args.original), Image.open(args.shaded)
        cutout = Image.open(args.cutout)
        mask_src = cutout.getchannel("A") if cutout.mode == "RGBA" else cutout.convert("L")
        shaded_small = shaded_full
    else:
        original, shaded_full, mask_src = synthetic(320, 480, alpha=True)
        shaded_small = shaded_full.resize((192, 288), Image.Resampling.LANCZOS)
    original.save(out / "original.png")
    shaded_small.save(out / "shaded.png")
    shaded_full.resize(original.size, Image.Resampling.LANCZOS).save(out / "shaded_full.png")
    mask_src.save(out / "mask_src.png")
    mask = mask_src.filter(ImageFilter.GaussianBlur(1.5))
    mask.save(out / "mask_blurred.png")

    stats = {}
    for name, (kwargs, use_mask, same_size) in CASES.items():
        shaded = Image.open(out / ("shaded_full.png" if same_size else "shaded.png"))
        layers = shading.shadow_layers(Image.open(out / "original.png"), shaded, mask=mask if use_mask else None, **kwargs)
        layers.multiply.save(out / f"{name}_multiply.png")
        layers.overlay.getchannel("A").save(out / f"{name}_overlay_alpha.png")
        layers.preview.save(out / f"{name}_preview.png")
        stats[name] = {"kwargs": kwargs, "mask": use_mask, "same_size": same_size, **layers.stats}
    (out / "cases.json").write_text(json.dumps(stats, indent=2))
    print(f"wrote {len(CASES)} cases to {out}")


if __name__ == "__main__":
    main()
