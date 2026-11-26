# Custom VUs

Every VU faceplate in mestro is data: a small manifest plus SVG
art, rendered by one generic engine (`crates/mestro/ui/vu/svgskin.slint`
driven by `crates/mestro/src/vu/manifest.rs`). The built-ins under
`crates/mestro/skins/` — `night`, `cream`, `round`, `wall`, `wall-light`, `arc`, `orbit`, `modern` —
take exactly the same path as your own: ship a directory with
a `manifest.toml` and the art it references, and register it.
The default cycle also includes the two analyzer views,
`spectrum` and `bars` — built-in compiled faces, not manifest
skins (see "The spectrum face" below).

```toml
[vu.skins]
amber = "/etc/mestro/amber/manifest.toml"   # a manifest skin
retro = "/etc/mestro/retro.slint"           # a full slint skin (below)
```

`[vu] cycle` entries accept built-in names and skin names
alike; swiping the faceplate cycles through them.

## The manifest

All geometry lives in one **faceplate design space** you pick
(`design-w`/`design-h`). The engine scales the whole design
uniformly to fit any window (`fit = min(window/design)`,
centered) and vertically centers the meters+info group — you
never position anything in screen pixels.

```toml
glow = 0            # the glow dial, 0..1 — feeds every glow layer
light = false       # a light plate flips the ghost-arrow ink

[faceplate]
design-w = 573
design-h = 344
plate = "#000000"   # band tone — see the plate contract
plate-art = "plate.svg"   # optional: stretched over the whole zone
                            # before the fitted faceplate (seamless
                            # plates at any aspect)

# one [[dial]] per channel; the engine animates needles by
# dial index (left = level-l, right = level-r)
[[dial]]
side = "left"       # left | right
x = 104             # dial top-left, faceplate units
art-w = 180         # the dial's art design box
art-h = 180         # — every layer SVG shares this viewBox
pivot-x = 90        # needle rotation origin, dial units
pivot-y = 90
zero-angle = -60    # needle angle at level 0
sweep = 120         # added over the full level range

# the layer stack, in file order = z-order, bottom first
[[dial.layer]]
role = "face"
source = "bg.svg"

[[dial.layer]]
role = "glow"       # engine-drawn warm wash, opacity = glow
radius = 90         # corner radius; optional inset shrinks it
inset = 0

[[dial.layer]]
role = "scale"      # ticks + traced numbers
source = "scale.svg"

[[dial.layer]]
role = "overlay"    # free art: per-side lettering, brand marks
source = "overlay-l.svg"

[[dial.layer]]
role = "label"       # live text (L/R), or use overlay art
text = "L"
y = 149
size = 12
color = "#6b5c42"

[[dial.layer]]
role = "needle"     # the ONLY animated layer
source = "needle.svg"
```

Layer placement defaults to the full dial box; `x`/`y`/`w`/`h`
(per layer, dial units) override for free art.

## The plate contract

The faceplate scales **uniformly** and centers in the zone.
When the zone aspect differs from the design aspect (the nav
rail narrows the zone, kiosk windows, future layout changes)
letterbox bands appear — they take the `plate` color, so set
`plate` to the **bg SVG edge tone** and the bands blend.

The seamless option is `plate-art`: one SVG the engine
**stretches over the whole zone** before the fitted faceplate
(wall-light ships it for the window-centered plate radial).
No bands at all, any aspect — preferred whenever the plate is
more than a flat color. The stock dark skins ship tone-matched
`plate` values instead (their bands are invisible).

## The needle contract

Draw the needle **pointing up from the pivot** in dial design
space. The engine rotates it around `(pivot-x, pivot-y)` by

```
angle = zero-angle + level × sweep      (level 0..1)
```

`level` spans the classic VU scale **−20 … +3 dB**, smoothed
to ~300ms ballistics at ~30fps — like a physical meter. The
angle mapping is yours: the round tachos sweep −60°…+60°
(`zero-angle = -60, sweep = 120`), the straight-scale walls
−42°…+42° (`zero-angle = -42, sweep = 84`), matching where
their ticks sit.

Rotating a raster resamples with bilinear filtering — keep
needle art thin and high-contrast, and expect a barely softer
edge at steep angles.

## The VU scale

The printed scale is yours to draw, but the values map linearly:
`angle(v) = (v + 20) × sweep / 23 + zero-angle`. A classic VU
prints −20, −10, −7, −5, −3, 0 and +3 (0 VU sits ~74% into the
arc — headroom above it is the point). Numbers that must hit
their tick exactly belong on the tick's angle, centered.

## SVG rules (the sharp edges)

- **`<text>` never renders.** slint parses SVGs with an empty
  fontdb, so any text element silently disappears. Convert
  type to outlines: render high-res (e.g. 220pt), trace with
  potrace, embed the paths, position with a `translate() +
  scale()` group. The built-in scales are generated this way.
- **Rasterization is at physical device resolution** — that is
  the whole point. Ticks and traced numbers stay crisp at any
  window size or DPI; rotated geometry you would otherwise draw
  in slint (anti-aliasing mush at fractional scale factors)
  belongs in the SVG.
- **Avoid `gradientUnits="userSpaceOnUse"` with centers pinned
  to the tile edge** (slint's SVG pipeline renders them as flat
  fill). Express such gradients in bounding-box percentages
  instead — `crates/mestro/skins/wall/bg-l.svg` vs `bg-r.svg` show a
  window-wide plate radial split across two 50:50 dial tiles,
  with `cx="100%"` / `cx="0%"` keeping the seam invisible.
- **`feGaussianBlur` filter regions clip** relative to the
  element bounding box — on thin rects (a needle stem) the halo
  gets cut off visibly. Bake soft auras as gradient shapes
  instead (see the wall needles).
- A hairline between two tiled dials rides the seam: draw it at
  the facing edge of one tile, as its own **layer above the
  glow**, so the lamp wash cannot bury it
  (`crates/mestro/skins/wall*/separator.svg`).

## The glow dial

`glow` layers are a warm lamp (amber radial, `#f0a24e →
#d9863a`) behind the print, and they are BAKED: the engine
composites the lamp into the opaque art the layer lights —
the nearest image layer below it (`round`, `cream`, `arc`,
`orbit` wash their face art), or the plate art when nothing
sits below the layer (`wall-light` lights the whole plate).
resvg rasterizes the composite, so every renderer draws the
same opaque pixels: femtovg (the device renderer) mishandles
semi-transparent blending, and no glow alpha ever reaches it.
One baked variant per glow value, cached and regenerated on
settings edits and activations. Built-ins take the value from
config (`[vu.round] glow = 0.5`, `[vu.wall-light] glow = 0.8`);
other manifest skins use the manifest default, and a
`[vu.skin-options] glow` key overrides both. Values apply live.

The layer's `radius` (dial units) shapes the lamp's clip:
a rounded box inset by `inset` (at least half the box makes it
a circle), filled edge to edge — warmest in the middle, easing
out to a dimmer rim. `radius = 0` is the plate lamp: one radial
ellipse over the whole plate art, its x/y/w/h/inset ignored,
so a plate-wide glow shows no disc edge, no seam and no unlit
bands.

One glow layer cannot bake: a `tint = "palette"` lamp with no
art below it (`modern`'s tinted wash over the bare plate). It
keeps the runtime engine gradient — femtovg renders it as a
centred circle, and the cover palette reaches it live.

## Info lines

Built-ins render an info block under the meters from
`[vu.<name>] info = ["title", "artist", "quality"]` (one field
per line, fixed hierarchy). The engine positions the block
relative to the meters and reserves the gap only when it
exists — meters-only skins bleed edge to edge. Custom manifest
skins currently receive no info fields; the block renders when
info lines arrive.

## Full-control skins

When a manifest is not enough (custom behaviors, layouts the
dial model cannot express), ship a `.slint` file implementing
the VuSkin contract — compiled at boot, rendered fullscreen
(custom skins compile with Slint edition 1.19 semantics):

```slint
export component MyVu inherits Rectangle {
    in property <float> level-l, level-r; // 0..1 across -20..+3 VU
    in property <string> title, artist, codec, quality;
    in property <bool> lossless, has-media, show-info;
    in property <int> info-count;
    callback back();   // optional: your own back affordance
}
```

Properties are optional (implement a subset); everything
unknown is ignored. `[vu.skin-options]` key-value pairs pass
through as properties, so a slint skin can expose its own
knobs through the config file. `crates/mestro/skins/minimal.slint` is a
documented example.

## Testing

Add the skin to `[vu] cycle`, swipe through the views. A broken
manifest or unreadable art logs a warning and drops the skin
from the cycle.

No squeezelite on the dev machine? `crates/mestro/tools/vu-feed.rs` writes a
modulated stereo sine in squeezelite's visualizer format, so the
needles move without real audio:

```sh
cargo run --example vu-feed
```

## The spectrum face

`spectrum` is a built-in VU view, not a manifest or slint skin:
a 32-band LED-segment matrix (the classic receiver analyzer —
discrete blocks, ghost matrix, red zone, ~1s peak caps), fed
by the shmem reader's spectrum analyzer.

It is **native-compiled on purpose** (`crates/mestro/ui/vu/spectrum.slint`):
its 32×24 segment grid touches thousands of properties per
frame, and runtime skins evaluate through the interpreter —
a runtime version of it burned a whole core. The manifest VUs
animate ~10 properties and are fine interpreted; the spectrum
is the one built-in that draws a grid.

The analyzer lives in `crates/mestro/src/vu/fft.rs`: rustfft over a
Hann-windowed 4096-frame mono tail (~93ms — shorter windows
flicker), log-spaced bands from 50Hz to 10kHz (cava-aligned),
per-band peak magnitudes mapped onto a compressed dBFS scale
(red ≈ "loud", not "clipping"), with asymmetric ballistics:
smoothed attack (~2-3 frames), fast fall, peak caps held ~1s.
The calibration is empirical, matched against cava with its
smoothing disabled.

Data path: the reader pushes `(bands, peaks)` into shared
state; the UI tick writes them into two persistent models on
the `Mestro` instance (`vu-spectrum-bands`/`vu-spectrum-peaks`),
updated in place — a fresh model per tick re-instantiates
every segment, the same trap as everywhere else.
