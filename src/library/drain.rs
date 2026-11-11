//! The slint-thread drain: keyed provider replies, cover pixels and
//! late subject images flow into the UI here.
//!
//! The handlers borrow the shared state, mutate the UI and return a
//! cover url to prefetch — the fetch itself happens outside the
//! borrow (it re-enters the RefCell).

use super::convert::{lib_entry, to_image};
use super::drill::prefetch_cover;
use super::state::{LibMsg, LibUi};
use crate::{LibEntry, Mestro};
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn spawn(
    ui_weak: slint::Weak<Mestro>,
    lib: Rc<RefCell<LibUi>>,
    lib_tx: super::drill::LibTx,
    mut lib_rx: tokio::sync::mpsc::UnboundedReceiver<LibMsg>,
) {
    slint::spawn_local(async move {
        while let Some(msg) = lib_rx.recv().await {
            let Some(ui) = ui_weak.upgrade() else {
                continue;
            };
            // handlers return a cover url to prefetch outside the
            // state borrow
            let prefetch = {
                let mut l = lib.borrow_mut();
                match msg {
                    // the artist's own image, fetched alongside the
                    // albums — applies when its level is still shown
                    LibMsg::SubjectImage { key, url } => subject_image(&ui, &mut l, key, url),
                    LibMsg::Cover { url, data } => {
                        cover(&ui, &mut l, url, data);
                        None
                    }
                    LibMsg::Update { key, entries } => update(&ui, &mut l, key, entries),
                }
            };
            if let Some(url) = prefetch {
                prefetch_cover(&lib, &lib_tx, &url);
            }
        }
    })
    .unwrap();
}

/// late subject image (artist headers) — returns the url to fetch
/// when it isn't cached yet
fn subject_image(
    ui: &Mestro,
    l: &mut std::cell::RefMut<'_, LibUi>,
    key: String,
    url: String,
) -> Option<String> {
    // the artists/get reply can beat the albums reply — the image
    // then arms the PENDING level instead of the current one and
    // update() prefetches it once the level lands
    if l.current_key != key {
        if let Some((pk, _, subject)) = l.pending.as_mut() {
            if *pk == key {
                if let Some(s) = subject {
                    tracing::debug!("library: early subject image for {key}");
                    s.image_url = url.clone().into();
                }
            }
        }
        return None;
    }
    let s = l.current_subject.as_mut()?;
    s.image_url = url.clone().into();
    ui.set_lib_subject(s.clone());
    if let Some(data) = l.covers.get(&url) {
        ui.set_lib_cover(to_image(data));
        None
    } else {
        ui.set_lib_cover(slint::Image::default());
        Some(url)
    }
}

/// cover pixels: cache, apply if it's the visible subject's
fn cover(
    ui: &Mestro,
    l: &mut std::cell::RefMut<'_, LibUi>,
    url: String,
    data: crate::player::cover::CoverData,
) {
    let img = to_image(&data);
    l.covers.insert(url.clone(), data);
    if l.current_subject
        .as_ref()
        .is_some_and(|s| s.image_url == url)
    {
        ui.set_lib_cover(img);
    }
}

/// a browse/drill reply — returns the subject cover url to fetch
/// when it isn't cached yet
fn update(
    ui: &Mestro,
    l: &mut std::cell::RefMut<'_, LibUi>,
    key: String,
    entries: Vec<crate::providers::MediaEntry>,
) -> Option<String> {
    // stale answer (request abandoned mid-flight) — keep whatever
    // is pending now
    if l.pending.as_ref().is_none_or(|(k, _, _)| *k != key) {
        tracing::debug!("library: drop stale reply {key}");
        return None;
    }
    tracing::debug!("library: apply reply {key} ({} entries)", entries.len());
    let (_, title, subject) = l.pending.take().unwrap();
    l.current = entries.iter().map(lib_entry).collect();
    l.current_subject = subject.clone();
    ui.set_lib_model(slint::ModelRc::new(slint::VecModel::from(
        l.current.clone(),
    )));
    // fresh level: the two-way scroll binding still holds the
    // previous level's offset — reset
    ui.set_lib_scroll(0.0);
    ui.set_lib_title(title.into());
    ui.set_lib_loading(false);
    l.current_key = key;
    let Some(s) = subject.clone() else {
        ui.set_lib_mode("list".into());
        ui.set_lib_subject(LibEntry::default());
        ui.set_lib_meta("".into());
        ui.set_lib_cover(slint::Image::default());
        return None;
    };
    // an album row may not carry its artist reference — the
    // track list always does. Backfill the subject so the
    // context menu can always offer go-to-artist in an album
    let mut s = s;
    if s.kind == "album" && s.artist_id.is_empty() {
        if let Some(track) = l
            .current
            .iter()
            .find(|e| e.kind == "track" && !e.artist_id.is_empty())
        {
            s.artist_id = track.artist_id.clone();
            s.artist_uri = track.artist_uri.clone();
            if s.artist.is_empty() {
                s.artist = track.artist.clone();
            }
        }
        l.current_subject = Some(s.clone());
        ui.set_lib_subject(s.clone());
    }
    let mode = super::state::mode_of(&s);
    ui.set_lib_mode(mode.into());
    ui.set_lib_subject(s.clone());
    let meta = LibUi::meta_for(&s, &l.current);
    ui.set_lib_meta(meta.into());
    let url = s.image_url.to_string();
    if url.is_empty() {
        ui.set_lib_cover(slint::Image::default());
        return None;
    }
    if let Some(data) = l.covers.get(&url) {
        ui.set_lib_cover(to_image(data));
        None
    } else {
        ui.set_lib_cover(slint::Image::default());
        Some(url)
    }
}
