"""Shadow layers: the layer that turns an original image into a shaded version of it.

Python reference implementation (numpy + Pillow). src/shading.rs is a port of this file and
tests/regression.rs checks the port against fixtures generated from it (tests/gen_fixtures.py).

Given the original O and a shaded version S (e.g. produced by an instruction edit "add natural shadows"), the
shadow layer L satisfies O x L ~= S, i.e. it can be put on top of the original in "multiply" mode (or, for the
grayscale layer, as a black layer with alpha 1 - L in normal mode). L is solved per pixel (least squares over the
color channels), then smoothed with a guided filter that follows the original's edges, so small misalignments and
re-drawn details of the model output do not end up in the layer. Only darkening is kept (L <= 1).
"""

from __future__ import annotations

from dataclasses import dataclass, field

import numpy as np
from PIL import Image

# Regularizes the ratio in near-black pixels (ratio of tiny numbers) towards "no shadow".
RATIO_EPS = 0.02


@dataclass
class ShadowLayers:
    multiply: Image.Image  # "L" (grayscale) or "RGB" (colored) layer for multiply blending
    overlay: Image.Image  # RGBA black layer whose alpha is the darkening, for normal blending
    preview: Image.Image  # original x layer: exactly what the layered result looks like
    stats: dict[str, float] = field(default_factory=dict)


def _rgb(image: Image.Image) -> np.ndarray:
    if image.mode in ("RGBA", "LA", "P"):
        rgba = image.convert("RGBA")
        image = Image.alpha_composite(Image.new("RGBA", rgba.size, "white"), rgba)
    return np.asarray(image.convert("RGB"), dtype=np.float32) / 255.0


def _box(a: np.ndarray, r: int) -> np.ndarray:
    """Mean over a (2r+1)^2 window, edges replicated."""
    # float64: float32 cumulative sums over megapixel images lose precision (blocky artifacts).
    padded = np.pad(a.astype(np.float64), ((r + 1, r), (r + 1, r)), mode="edge")
    c = padded.cumsum(0).cumsum(1)
    k = 2 * r + 1
    s = c[k:, k:] - c[:-k, k:] - c[k:, :-k] + c[:-k, :-k]
    return s / (k * k)


def guided_filter(guide: np.ndarray, src: np.ndarray, r: int, eps: float) -> np.ndarray:
    """He et al. guided filter (gray guide): smooths `src` while keeping the edges of `guide`."""
    if r < 1:
        return src
    mean_i, mean_p = _box(guide, r), _box(src, r)
    cov = _box(guide * src, r) - mean_i * mean_p
    var = _box(guide * guide, r) - mean_i * mean_i
    a = cov / (var + eps)
    b = mean_p - a * mean_i
    return (_box(a, r) * guide + _box(b, r)).astype(np.float32)


def guided_coefficients(guide: np.ndarray, src: np.ndarray, r: int, eps: float) -> tuple[np.ndarray, np.ndarray]:
    """Smoothed linear coefficients (A, B) of the guided filter: output = A * guide + B."""
    mean_i, mean_p = _box(guide, r), _box(src, r)
    cov = _box(guide * src, r) - mean_i * mean_p
    var = _box(guide * guide, r) - mean_i * mean_i
    a = cov / (var + eps)
    b = mean_p - a * mean_i
    return _box(a, r).astype(np.float32), _box(b, r).astype(np.float32)


def _resize(a: np.ndarray, size: tuple[int, int]) -> np.ndarray:
    """Bilinear resize of a float map to (w, h)."""
    return np.asarray(Image.fromarray(a.astype(np.float32), "F").resize(size, Image.Resampling.BILINEAR))


def _ratio(o: np.ndarray, s: np.ndarray, color: bool) -> np.ndarray:
    if color:
        return (o * s + RATIO_EPS / 3) / (o * o + RATIO_EPS / 3)
    return ((o * s).sum(-1) + RATIO_EPS) / ((o * o).sum(-1) + RATIO_EPS)


def shadow_layers(
    original: Image.Image,
    shaded: Image.Image,
    *,
    color: bool = False,
    strength: float = 1.0,
    smooth: float = 0.0,
    normalize: bool = True,
    mask: Image.Image | None = None,
    radius_frac: float = 0.008,
    eps: float = 5e-3,
    detail_blur: float = 2.0,
    tint_limit: float | None = 0.05,
) -> ShadowLayers:
    """Solve the shadow layer for `original` -> `shaded`.

    The ratio is solved at the shaded image's own resolution (usually ~1MP, where it is aligned with a
    downscaled original without resampling blur) and brought to the original's resolution by guided upsampling
    (fast guided filter): the layer's edges follow the full-resolution original, so a 4K photo gets crisp shadow
    edges from a 1MP model output.

    color: per-channel layer (tinted shadows, multiply only) instead of a grayscale one.
    strength: scales the darkening (0.5 = half as dark, 1.5 = darker).
    smooth: scales the smoothing (0 = raw per-pixel ratio, only resampled).
    normalize: treat the brightest areas (97th percentile inside the mask) as unshadowed, i.e. remove an overall
        dimming / color grade of the model output so only actual shadows remain.
    mask: white = shadows allowed (e.g. the subject); elsewhere the layer stays white.
    radius_frac / eps: guided-filter radius (fraction of the long side) and edge sensitivity.
    detail_blur: blur (per mille of the long side) applied to both images before the ratio, so details the model
        re-drew slightly differently (hair strands, eyes) do not print as ghost edges; shadow shapes are larger.
        Tuned on 1-4K anime renders: 2.0 / 0.008 / 5e-3 removed ghosting while keeping shadow shapes.
    tint_limit: color mode only - max deviation of each channel from the gray layer, so shadows may be warm or cool
        but a model that re-colored something (light-blue hair turned saturated blue) does not re-color the original.
    """
    o = _rgb(original)
    h, w = o.shape[:2]
    luma = np.array([0.299, 0.587, 0.114], dtype=np.float32)
    lw, lh = shaded.size
    if lw * lh >= w * h:  # model output at least as large: work at the original size
        lw, lh = w, h
    s_lo = _rgb(shaded.resize((lw, lh), Image.Resampling.LANCZOS) if shaded.size != (lw, lh) else shaded)
    o_lo = _rgb(original.resize((lw, lh), Image.Resampling.LANCZOS)) if (lw, lh) != (w, h) else o
    guide_lo, guide = o_lo @ luma, o @ luma
    m = None
    if mask is not None:
        m = np.asarray(mask.convert("L").resize((w, h), Image.Resampling.LANCZOS), dtype=np.float32) / 255.0
    m_lo = _resize(m, (lw, lh)) if m is not None else None

    if detail_blur > 0 and smooth > 0:  # ignore fine re-drawn details (hair strands) of the model output
        sigma = smooth * detail_blur * max(lw, lh) / 1000
        r = max(1, round(sigma))
        blur = lambda a: np.stack([_box(_box(_box(a[..., c], r), r), r) for c in range(3)], -1)  # noqa: E731  ~Gaussian
        ratio = _ratio(blur(o_lo), blur(s_lo), color)
    else:
        ratio = _ratio(o_lo, s_lo, color)
    if color and tint_limit is not None:
        gray_ratio = ratio @ luma
        ratio = gray_ratio[..., None] + np.clip(ratio - gray_ratio[..., None], -tint_limit, tint_limit)
    channels = [ratio[..., c] for c in range(3)] if color else [ratio]
    brightened = float((ratio > 1.02).mean())
    if normalize:
        weight = (m_lo > 0.5) if m_lo is not None else np.ones((lh, lw), bool)
        lum = ratio @ luma if color else ratio
        lit = float(np.percentile(lum[weight], 97)) if weight.any() else 1.0
        if lit > 0.05:
            channels = [c / lit for c in channels]
    radius_lo = max(1, round(max(0.0, smooth) * radius_frac * max(lw, lh)))
    upscale = (w, h) != (lw, lh)
    layer_channels = []
    for c in channels:
        if smooth <= 0 and not upscale:
            layer_channels.append(c)
            continue
        a, b = guided_coefficients(guide_lo, c, radius_lo, eps)
        if upscale:
            a, b = _resize(a, (w, h)), _resize(b, (w, h))
        layer_channels.append(a * guide + b)
    layer = np.stack(layer_channels, axis=-1) if color else layer_channels[0]
    layer = np.clip(layer, 0.0, 1.0)
    layer = 1.0 - (1.0 - layer) * max(0.0, strength)
    if m is not None:
        layer = 1.0 - (1.0 - layer) * (m[..., None] if layer.ndim == 3 else m)
    layer = np.clip(layer, 0.0, 1.0)

    gray = layer if layer.ndim == 2 else layer @ luma
    preview = o * (layer if layer.ndim == 3 else layer[..., None])
    to8 = lambda a: (np.clip(a, 0, 1) * 255.0 + 0.5).astype(np.uint8)  # noqa: E731
    overlay = np.zeros((h, w, 4), dtype=np.uint8)
    overlay[..., 3] = to8(1.0 - gray)
    # How well the layer explains the model output (mean abs error at the model's resolution, 0-255 scale).
    preview_lo = np.asarray(Image.fromarray(to8(preview)).resize((lw, lh), Image.Resampling.LANCZOS), np.float32)
    error = float(np.abs(preview_lo / 255.0 - s_lo).mean() * 255.0)
    return ShadowLayers(
        multiply=Image.fromarray(to8(layer), "RGB" if layer.ndim == 3 else "L"),
        overlay=Image.fromarray(overlay, "RGBA"),
        preview=Image.fromarray(to8(preview), "RGB"),
        stats={
            "mean_darkening": round(float(1.0 - gray.mean()), 4),
            "brightened_fraction": round(brightened, 4),
            "reconstruction_error": round(error, 2),
            "solve_size": [lw, lh],
            "smooth_radius_px": radius_lo,
        },
    )
