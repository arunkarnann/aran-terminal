#!/usr/bin/env python3
"""Generate the Aran Terminal logo: a sharp white 'A' on a black macOS-style tile.

Dock-native shape: a rounded-rectangle (squircle-ish) black tile on a transparent
canvas with ~9% margin, so it sits naturally beside other macOS app icons — no hard
square edge. Rendered at 4x then downscaled with LANCZOS for crisp edges.
Output: app-icon.png (1024x1024) used by `tauri icon` to produce all assets.
"""
from PIL import Image, ImageDraw

S = 1024          # final size
SS = 4            # supersample factor
N = S * SS

TILE = (13, 13, 13, 255)     # near-black tile
WHITE = (245, 245, 245, 255)

img = Image.new("RGBA", (N, N), (0, 0, 0, 0))  # transparent canvas
d = ImageDraw.Draw(img)


def pt(x, y):
    return (x * SS, y * SS)


def poly(points, fill):
    d.polygon([pt(x, y) for x, y in points], fill=fill)


# --- rounded black tile (macOS squircle-style), transparent margin around it ---
m = 92                         # transparent margin
radius = 186                   # ~0.2237 * tile size, matches macOS icon grid
d.rounded_rectangle(
    [pt(m, m)[0], pt(m, m)[1], pt(S - m, S - m)[0], pt(S - m, S - m)[1]],
    radius=radius * SS,
    fill=TILE,
)

# --- geometric 'A', centered on the tile ---
apex = (512, 286)
bl = (300, 742)
br = (724, 742)
poly([apex, br, bl], WHITE)

# Top counter (hole) — carve back to the tile colour
poly([(512, 410), (452, 568), (572, 568)], TILE)

# Bottom notch — split the legs
poly([(512, 620), (384, 742), (640, 742)], TILE)

out = img.resize((S, S), Image.LANCZOS)
out.save("/Users/arun/github/Github2/macrosx/conductor/app-icon.png")
print("wrote app-icon.png", out.size)
