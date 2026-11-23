#![allow(clippy::unwrap_used, clippy::expect_used)]
// test harness helpers: a panic fails the test loudly, which is
// exactly what a test wants

//! Now playing while an external source (AirPlay) plays: the source's
//! own metadata, a small "via AirPlay", the queue-bound controls and
//! the queue button greyed. Rendered offscreen on the software
//! renderer — no window, nothing on the desktop. Opt-in (needs a text
//! font; the dev shell sets FONTCONFIG_FILE):
//!
//!   MESTRO_SNAPSHOT_DIR=/tmp/shots cargo test --test external_source_snapshot -- --ignored

use mestro::{Mestro, TransportCaps};
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::ComponentHandle;
use std::{path::PathBuf, rc::Rc};

struct Headless {
    window: Rc<MinimalSoftwareWindow>,
}

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
}

/// the 5" kiosk panel at scale 1.5 → 533x320 logical
const W: u32 = 800;
const H: u32 = 480;
const SCALE: f32 = 1.5;

fn render(window: &MinimalSoftwareWindow) -> image::RgbImage {
    slint::platform::update_timers_and_animations();
    window.request_redraw();
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(W, H);
    window.draw_if_needed(|renderer| {
        renderer.render(buffer.make_mut_slice(), W as usize);
    });
    image::RgbImage::from_raw(W, H, buffer.as_bytes().to_vec()).expect("buffer size")
}

fn cover() -> slint::Image {
    let mut px = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(64, 64);
    px.make_mut_slice().fill(slint::Rgba8Pixel {
        r: 160,
        g: 90,
        b: 60,
        a: 255,
    });
    slint::Image::from_rgba8(px)
}

fn luma(p: &image::Rgb<u8>) -> u32 {
    (p.0[0] as u32 * 299 + p.0[1] as u32 * 587 + p.0[2] as u32 * 114) / 1000
}

/// pixels in a region whose luma lies in `lo..=hi`
fn count(img: &image::RgbImage, x: u32, y: u32, w: u32, h: u32, lo: u32, hi: u32) -> u32 {
    let mut n = 0;
    for py in y..(y + h).min(img.height()) {
        for px in x..(x + w).min(img.width()) {
            if (lo..=hi).contains(&luma(img.get_pixel(px, py))) {
                n += 1;
            }
        }
    }
    n
}

#[test]
#[ignore = "visual review: MESTRO_SNAPSHOT_DIR=… cargo test --test external_source_snapshot -- --ignored"]
fn now_playing_while_airplay_plays() {
    let dir = PathBuf::from(std::env::var("MESTRO_SNAPSHOT_DIR").expect("set MESTRO_SNAPSHOT_DIR"));
    std::fs::create_dir_all(&dir).unwrap();

    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    window.set_size(slint::PhysicalSize::new(W, H));
    window.dispatch_event(slint::platform::WindowEvent::ScaleFactorChanged {
        scale_factor: SCALE,
    });
    slint::platform::set_platform(Box::new(Headless {
        window: window.clone(),
    }))
    .unwrap();

    let ui = Mestro::new().unwrap();
    ui.set_powered(true);
    ui.set_provider_connected(true);
    ui.set_has_media(true);
    ui.set_playing(true);
    ui.set_np_open(true);
    ui.set_np_inline_vu(0);
    ui.set_cover(cover());
    ui.set_track_title("Stand by Me".into());
    ui.set_track_artist("Ben E. King".into());
    ui.set_track_album("Stand by Me".into());
    ui.set_duration_seconds(179);
    ui.set_progress(0.35);
    ui.set_pos_label("1:03".into());
    ui.set_dur_label("2:59".into());
    ui.show().unwrap();

    // the queue: shuffle/previous/next/repeat live, queue button on
    let queue = render(&window);
    queue.save(dir.join("np-queue.png")).unwrap();

    // AirPlay: the same screen, the source named, queue-bound things off
    ui.set_external_source("AirPlay Receiver".into());
    ui.set_can_seek(false);
    // AirPlay reports every capability false: play-pause greys out too
    ui.set_transport_caps(TransportCaps {
        play_pause: false,
        next_previous: false,
        shuffle: false,
        repeat: false,
    });
    let external = render(&window);
    external.save(dir.join("np-external-side.png")).unwrap();

    // (physical pixels, scale 1.5) "via AirPlay" paints under the album
    // line in mute grey; the queue's screen has nothing there
    let via = |img: &image::RgbImage| count(img, 330, 238, 240, 24, 90, 140);
    assert!(
        via(&external) > via(&queue) + 20,
        "no 'via' line: {} vs {}",
        via(&external),
        via(&queue)
    );

    // the round play button: accent fill on the queue, the bar colour
    // once the source reports can_play_pause = false
    let play = |img: &image::RgbImage| count(img, 460, 322, 120, 50, 140, 255);
    assert!(
        play(&external) + 200 < play(&queue),
        "play-pause did not grey out: {} vs {}",
        play(&external),
        play(&queue)
    );

    // the queue-bound glyphs (shuffle, previous | next, repeat) drop from
    // text-dim to text-mute
    let glyphs = |img: &image::RgbImage| {
        count(img, 340, 325, 120, 45, 140, 255) + count(img, 575, 325, 130, 45, 140, 255)
    };
    let bright = glyphs;
    assert!(
        bright(&external) < bright(&queue),
        "the transport row did not dim: {} vs {}",
        bright(&external),
        bright(&queue)
    );

    // a source that skips and seeks (Spotify Connect): those two come
    // back, the rest stays grey
    ui.set_transport_caps(TransportCaps {
        play_pause: true,
        next_previous: true,
        shuffle: false,
        repeat: false,
    });
    ui.set_can_seek(true);
    render(&window)
        .save(dir.join("np-external-spotify-like.png"))
        .unwrap();
    ui.set_transport_caps(TransportCaps {
        play_pause: false,
        next_previous: false,
        shuffle: false,
        repeat: false,
    });
    ui.set_can_seek(false);

    // fullbleed shares the greyed row
    ui.set_np_layout(1);
    render(&window)
        .save(dir.join("np-external-fullbleed.png"))
        .unwrap();

    // placeholder art: a track without artwork shows the note
    ui.set_np_layout(0);
    ui.set_cover_missing(true);
    render(&window)
        .save(dir.join("np-external-no-cover.png"))
        .unwrap();

    // the source is known but nothing plays yet: the progress tick
    // (main.rs) clears the bar once no duration is known
    ui.set_has_media(false);
    ui.set_cover_missing(false);
    ui.set_duration_seconds(0);
    ui.set_progress(0.0);
    ui.set_pos_label("".into());
    ui.set_dur_label("".into());
    render(&window)
        .save(dir.join("np-external-waiting.png"))
        .unwrap();
}
