# Standby: clocks and backgrounds

The standby screen shows a clock while the player is off. Two
faces are built in — `digital` (default) and `analog` — and any
`.slint` file implementing the StandbySkin contract can replace
them.

```toml
[clock]
style = "big"        # "digital", "analog" or a skin name

[clock.skins]
big = "/data/skins/big-clock.slint"
```

Registered skins appear in Settings → Standby next to the
built-ins, each with a rendered preview. `MESTRO_CLOCK=<name>`
overrides the style for a single run.

## Background

Behind the clock, standby shows plain black (default), the last
played cover or a slideshow of your own pictures:

```toml
[clock.background]
source = "folder"            # black | cover | folder
dim = 0.6                    # darkening over the image, 0..1
blur = false                 # blurred instead of sharp
folder = "/data/backgrounds" # folder source: jpg/png files
interval = 10                # folder source: minutes per image, 0 = never
```

- **cover** follows the player: a new track brings its cover.
- **folder** picks a random image (never the same one twice in a
  row), decodes and scales it off the UI thread and crossfades
  over about a second. On the kiosk, drop the pictures onto the
  data partition from any PC.
- **dim** keeps the clock readable on bright pictures. Any image
  keeps the backlight busy — `black` stays the night setting.

All of it is editable in Settings → Standby.

## The contract

```slint
export component MyClock inherits Rectangle {
    in property <int>    hour;        // 0..23
    in property <int>    minute;      // 0..59
    in property <string> time-label;  // "21:42"
    in property <bool>   valid;       // false until the system time is trustworthy
    in property <string> date-label;  // "MON 09 NOV"
    in property <color>  accent;      // theme accent (follows the cover palette)
    callback power-on();              // wake the player
}
```

`valid` is false while the system time cannot be trusted: the Pi has
no RTC, so after a boot it reads an old date until NTP answers.
"Trustworthy" means the year is at least the year this binary was
built — no NTP coupling. Until then `hour`/`minute` are 0, `time-label`
is `--:--` and `date-label` is empty; a skin that draws hands or
digits itself should draw nothing (or a bare dial) while `valid` is
false. A skin that only shows `time-label` needs nothing: it shows
the placeholder. The built-in faces, the top bar clock and the home
screen's greeting follow the same rule, and the next tick after
the sync switches to the real time.

Every member is optional — declare what you use. The last
exported component in the file is the skin. Imports are not
available; a skin is one self-contained file. Custom skins compile with
Slint edition 1.19 semantics.

A complete example:

```slint
export component BigClock inherits Rectangle {
    in property <string> time-label;
    in property <string> date-label;
    in property <color> accent: #d8a657;

    VerticalLayout {
        alignment: center;
        spacing: 12px;
        Text {
            text: root.time-label;
            horizontal-alignment: center;
            font-size: 160px;
            font-weight: 100;
            color: #8c8274;
        }
        Text {
            text: root.date-label;
            horizontal-alignment: center;
            font-size: 14px;
            letter-spacing: 4px;
            color: root.accent;
        }
    }
}
```

### Pitfalls

- **Fixed sizes.** The skin fills the standby screen. Size things
  in fixed lengths: a size computed from the skin's own
  `width`/`height` feeds back into the layout and Slint reports a
  binding loop.
- **No `transform-rotation`.** The settings preview renders
  skins with Slint's software renderer, which cannot rotate
  (slint #6068) — rotated elements come out wrong or missing.
  Place dial marks and hands with `Math.sin`/`Math.cos` instead
  (the built-in analog face draws its hands as chains of
  overlapping dots).

## What mestro does for you

- **Black ground**: standby renders on `#000`; a skin can paint
  its own background but should stay dark — LCD backlights
  cannot always be switched off.
- **Tap to wake**: a tap anywhere outside a skin's own touch
  areas wakes the player. Call `power-on()` from your own
  controls if you add any.
- **Burn-in jitter**: the whole skin shifts by up to ±3px every
  minute.
- **Fallback**: a skin that fails to load or compile logs a
  warning and the digital face takes over; so does a `style`
  naming no registered skin.

## Keep it idle

Values change once a minute. Avoid `animate` and timers in
standby skins: anything that keeps changing keeps the renderer
busy all night — CPU, heat and power on a kiosk that is meant to
sleep.
