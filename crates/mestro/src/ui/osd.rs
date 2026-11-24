//! The OSD layer: toast + volume overlay placement, show +
//! auto-hide wiring and the [osd] gating rules.
//!
//! The update channel carries toast/volume hints; [`show_gated`]
//! decides per [osd] flags whether they surface at all — base
//! flags with [osd.now-playing] / [osd.vu] context overrides,
//! depending on which overlay is open. The show callbacks own the
//! auto-hide: a generation counter per channel, so a newer show
//! cancels the older hide timer.

use crate::config::OsdAnchor;
use crate::player::state::{PlayerUpdate, Toast};
use crate::Mestro;
use slint::ComponentHandle;

/// placement anchors + tile style (also re-pushed by the
/// settings sync on live edits) and the show + auto-hide
/// callbacks (generation counter per channel — a newer show
/// cancels the older hide; durations read per show, so [osd]
/// edits apply live)
pub fn wire(ui: &Mestro) {
    let ui_weak = ui.as_weak();
    // one counter per channel: a volume show must not invalidate
    // the toast's pending hide (and vice versa)
    let toast_gen = std::rc::Rc::new(std::cell::Cell::new(0u32));
    let osd_gen = std::rc::Rc::new(std::cell::Cell::new(0u32));
    ui.on_show_toast({
        let ui_weak = ui_weak.clone();
        let toast_gen = toast_gen.clone();
        move |text| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let hide_after =
                std::time::Duration::from_millis(ui.get_osd_toast_duration().max(0.0) as u64);
            let gen = toast_gen.get() + 1;
            toast_gen.set(gen);
            ui.set_toast_text(text);
            ui.set_toast_visible(true);
            let ui_weak = ui_weak.clone();
            let toast_gen = toast_gen.clone();
            slint::Timer::single_shot(hide_after, move || {
                if toast_gen.get() == gen {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_toast_visible(false);
                    }
                }
            });
        }
    });
    ui.on_show_osd_volume({
        let ui_weak = ui_weak.clone();
        let osd_gen = osd_gen.clone();
        move |level| {
            let Some(ui) = ui_weak.upgrade() else { return };
            let hide_after =
                std::time::Duration::from_millis(ui.get_osd_volume_duration().max(0.0) as u64);
            let gen = osd_gen.get() + 1;
            osd_gen.set(gen);
            ui.set_osd_level(level);
            ui.set_osd_visible(true);
            let ui_weak = ui_weak.clone();
            let osd_gen = osd_gen.clone();
            slint::Timer::single_shot(hide_after, move || {
                if osd_gen.get() == gen {
                    if let Some(ui) = ui_weak.upgrade() {
                        ui.set_osd_visible(false);
                    }
                }
            });
        }
    });
}

/// gate a player update's toast/volume hints through [osd]:
/// base flags with [osd.now-playing] / [osd.vu] context
/// overrides, depending on which overlay is open. The gates come
/// from the UI props (the settings sync keeps them fresh) — the
/// context group already resolves `override or base`.
pub fn show_gated(ui: &Mestro, update: &PlayerUpdate) {
    let gates = if ui.get_vu_open() {
        ui.get_osd_vu_gates()
    } else if ui.get_np_open() {
        ui.get_osd_np_gates()
    } else {
        ui.get_osd_gates()
    };
    if let Some(toast) = &update.toast {
        let kind = match toast {
            Toast::Transport(_) => "transport",
            Toast::Track(_) => "track",
            Toast::Queue(_) => "queue",
        };
        let on = match kind {
            "transport" => gates.transport,
            "track" => gates.track,
            "queue" => gates.queue,
            _ => false,
        };
        if on {
            let text = match toast {
                Toast::Transport(t) | Toast::Track(t) | Toast::Queue(t) => t,
            };
            tracing::debug!("osd: toast ({kind}) '{text}'");
            ui.invoke_show_toast(text.into());
        }
    }
    // a remote volume change (not our own echo): the volume OSD,
    // gated like the toasts
    if let Some(v) = update.osd_volume {
        if gates.volume {
            tracing::debug!("osd: remote volume {v}");
            ui.invoke_show_osd_volume(v);
        }
    }
}

pub(crate) fn anchor(a: OsdAnchor) -> i32 {
    use OsdAnchor as A;
    match a {
        A::Top => 0,
        A::Bottom => 1,
        A::Left => 2,
        A::Right => 3,
        A::Center => 4,
    }
}
