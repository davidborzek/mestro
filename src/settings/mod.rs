//! The device-owned settings layer (see docs/configuration.md): a
//! sparse TOML tree merged over the read-only config file.
//!
//! ONE model: the overlay is not a mirrored struct — it is a
//! partial config tree. config::load deep-merges it over the
//! base tree (tables recurse, scalars and arrays replace) and
//! deserializes the result into the regular Config. New config
//! keys work in both layers without a second struct, and the
//! overlay can't drift: it only ever holds the keys the UI
//! actually changed.
//!
//! Robustness: if the merged tree fails to parse, the base
//! config alone decides — a corrupt overlay warns and drops,
//! the device still boots.

use std::cell::RefCell;
use std::io::Write as _;
use std::rc::Rc;

mod schema;
mod ui;
pub use schema::schema;
pub use ui::{
    apply_effective, sync_connection, sync_initial, sync_overlay_state, sync_theme_ui, sync_vu,
    wire, wire_vu, NpState, VuState,
};

/// the app-side editor handle: parses dotted keys into the
/// overlay tree, re-derives the effective config and debounces
/// persistence (typing must not fsync per keystroke).
///
/// The rewire dispatch lives with the caller (main) — it owns
/// the UI handles; set() hands back the effective config.
pub struct Handle {
    overlay: RefCell<Overlay>,
    base_tree: toml::Value,
    /// the resolved config.toml (None = built-in defaults only) —
    /// the settings page names what its edits override
    base_path: Option<std::path::PathBuf>,
    save: Rc<slint::Timer>,
}

impl Handle {
    /// Persist pending edits before replacing the process. Debounced
    /// timers do not get a chance to fire after a restart request.
    pub fn flush(&self) -> std::io::Result<()> {
        self.overlay.borrow().save()?;
        self.save.stop();
        Ok(())
    }

    /// does this key deviate from the config (is in the overlay)?
    pub fn is_overridden(&self, key: &str) -> bool {
        let path: Vec<&str> = key.split('.').collect();
        self.overlay.borrow().contains(&path)
    }
    /// any overlay leaf at all? Empty parent tables (left behind
    /// by per-key resets) don't count
    pub fn has_overrides(&self) -> bool {
        fn leaves(v: &toml::Value) -> bool {
            match v {
                toml::Value::Table(t) => t.values().any(leaves),
                _ => true,
            }
        }
        self.overlay.borrow().tree.as_ref().is_some_and(leaves)
    }
    /// the base config file the settings override, for display
    pub fn base_path(&self) -> Option<&std::path::Path> {
        self.base_path.as_deref()
    }
    /// the effective config tree (base ⊕ overlay) as plain data —
    /// the web endpoint serializes this
    pub fn merged(&self) -> toml::Value {
        match &self.overlay.borrow().tree {
            Some(tree) => {
                let mut merged = self.base_tree.clone();
                merge(&mut merged, tree);
                merged
            }
            None => self.base_tree.clone(),
        }
    }
    /// dotted paths of every overlay leaf (deviates from config)
    pub fn override_keys(&self) -> Vec<String> {
        fn walk(prefix: &str, value: &toml::Value, out: &mut Vec<String>) {
            match value {
                toml::Value::Table(t) => {
                    for (key, val) in t {
                        let path = if prefix.is_empty() {
                            key.clone()
                        } else {
                            format!("{prefix}.{key}")
                        };
                        walk(&path, val, out);
                    }
                }
                _ => out.push(prefix.to_string()),
            }
        }
        let mut out = Vec::new();
        if let Some(tree) = &self.overlay.borrow().tree {
            walk("", tree, &mut out);
        }
        out.sort();
        out
    }
    /// drop every overlay key at once: config.toml takes over
    /// again. The pending debounce is cancelled first so a stale
    /// snapshot cannot resurrect the dropped keys
    pub fn reset_all(&self) -> Result<crate::config::Config, String> {
        self.save.stop();
        {
            let mut overlay = self.overlay.borrow_mut();
            overlay.tree = None;
            if let Err(e) = std::fs::remove_file(&overlay.path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    tracing::warn!("settings: reset-all persist: {e}");
                }
            }
        }
        let mut cfg = lenient_config(self.base_tree.clone())?;
        cfg.validate();
        Ok(cfg)
    }
    /// the base tree comes from config::load's merge input —
    /// the handle re-merges after every change
    pub fn new(
        overlay: Overlay,
        base_tree: toml::Value,
        base_path: Option<std::path::PathBuf>,
    ) -> Self {
        Self {
            overlay: RefCell::new(overlay),
            base_tree,
            base_path,
            // debounce: single-shot, restarted on every set
            save: Rc::new(slint::Timer::default()),
        }
    }

    /// the effective config — the merged tree WITH the defaults
    /// folded in — for readers that need every field's current
    /// value, not just what the files set (the web's schema and
    /// `effective` state)
    pub fn effective(&self) -> Result<crate::config::Config, String> {
        let mut cfg = lenient_config(self.merged())?;
        cfg.validate();
        Ok(cfg)
    }

    /// set one key (dotted path, TOML-syntax value) and return
    /// the effective config: merge → parse (schema + types,
    /// this is the validation) → validate() (range clamps).
    /// Persistence debounces; a failed parse rolls the change
    /// back out of the overlay
    /// set one key (dotted path, typed value) and return the
    /// effective config: merge → parse (schema + types, this is
    /// the validation) → validate() (range clamps). Persistence
    /// debounces; a failed parse rolls the change back out of
    /// the overlay
    pub fn set(&self, key: &str, value: toml::Value) -> Result<crate::config::Config, String> {
        let path: Vec<&str> = key.split('.').collect();
        if path.len() < 2 || path.iter().any(|p| p.is_empty()) {
            return Err(format!(
                "setting key {key:?}: expected a dotted path like now-playing.font.title-size"
            ));
        }
        let outcome = {
            let mut overlay = self.overlay.borrow_mut();
            let prev = overlay.tree.clone();
            overlay.set(&path, value);
            let merged = match &overlay.tree {
                Some(tree) => {
                    let mut merged = self.base_tree.clone();
                    merge(&mut merged, tree);
                    merged
                }
                None => self.base_tree.clone(),
            };
            // lenient for keys that were already broken (the config
            // file's own problems must not block every edit), strict
            // for the key being set: a rejected edit rolls back
            match crate::config::lenient::parse::<crate::config::Config>(merged) {
                Ok((mut cfg, problems)) => {
                    if let Some(p) = problems
                        .iter()
                        .find(|p| p.key == key || key.starts_with(&format!("{}.", p.key)))
                    {
                        overlay.tree = prev;
                        tracing::warn!("settings: refusing {key} ({})", p.reason);
                        Err(format!("invalid value for {key}: {}", p.reason))
                    } else {
                        cfg.validate();
                        Ok(cfg)
                    }
                }
                Err(e) => {
                    overlay.tree = prev;
                    tracing::warn!("settings: refusing {key} ({e})");
                    Err(format!("invalid value for {key}: {e}"))
                }
            }
        };
        if outcome.is_ok() {
            // debounced persist — a snapshot, not the RefCell
            let snapshot = self.overlay.borrow().clone();
            self.save.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(500),
                move || {
                    if let Err(e) = snapshot.save() {
                        tracing::warn!("settings: persist failed: {e}");
                    }
                },
            );
        }
        outcome
    }

    /// remove one key — the config default returns. Shared
    /// merge/validate/debounce path with set()
    pub fn reset(&self, key: &str) -> Result<crate::config::Config, String> {
        let path: Vec<&str> = key.split('.').collect();
        if path.len() < 2 {
            return Err(format!("setting key {key:?}: expected a dotted path"));
        }
        {
            let mut overlay = self.overlay.borrow_mut();
            overlay.remove(&path);
        }
        let merged = match &self.overlay.borrow().tree {
            Some(tree) => {
                let mut merged = self.base_tree.clone();
                merge(&mut merged, tree);
                merged
            }
            None => self.base_tree.clone(),
        };
        let mut cfg = lenient_config(merged)?;
        cfg.validate();
        let snapshot = self.overlay.borrow().clone();
        self.save.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(500),
            move || {
                if let Err(e) = snapshot.save() {
                    tracing::warn!("settings: persist failed: {e}");
                }
            },
        );
        Ok(cfg)
    }
}

/// parse a merged tree leniently: keys the schema rejects are
/// dropped (they were already reported by config::load)
fn lenient_config(tree: toml::Value) -> Result<crate::config::Config, String> {
    crate::config::lenient::parse::<crate::config::Config>(tree)
        .map(|(cfg, _)| cfg)
        .map_err(|e| format!("config: {e}"))
}

/// the sparse overlay tree + its persistence path
#[derive(Debug, Default, Clone)]
pub struct Overlay {
    /// None = no file (nothing changed on this device yet)
    pub tree: Option<toml::Value>,
    pub path: std::path::PathBuf,
}

impl Overlay {
    /// missing file = empty overlay (clean default, not an
    /// error); unreadable/malformed file warns and drops — the
    /// file is device-owned, its schema is the config's
    pub fn load() -> Self {
        let path = state_dir().join("settings.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<toml::Value>(&text) {
                Ok(tree) => Self {
                    tree: Some(tree),
                    path,
                },
                Err(e) => {
                    tracing::warn!("settings: ignoring unreadable {}: {e}", path.display());
                    Self { tree: None, path }
                }
            },
            Err(_) => Self { tree: None, path },
        }
    }

    /// persist (atomic: tmp + rename). The caller debounces — no
    /// fsync per keystroke
    pub fn save(&self) -> std::io::Result<()> {
        let Some(tree) = &self.tree else {
            return Ok(());
        };
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string_pretty(tree)
            .map_err(|e| std::io::Error::other(format!("settings serialize: {e}")))?;
        let tmp = self.path.with_extension("toml.tmp");
        std::fs::File::create(&tmp)?.write_all(text.as_bytes())?;
        std::fs::rename(&tmp, &self.path)
    }

    /// set one key (dotted path through the tables), creating
    /// the tables on the way — the overlay stays sparse: only
    /// what the UI changed
    pub fn set(&mut self, path: &[&str], value: toml::Value) {
        let tree = self
            .tree
            .get_or_insert_with(|| toml::Value::Table(Default::default()));
        let mut cur = tree;
        for key in &path[..path.len() - 1] {
            let table = cur
                .as_table_mut()
                .expect("overlay path traverses non-table");
            cur = table
                .entry(key.to_string())
                .or_insert_with(|| toml::Value::Table(Default::default()));
        }
        let last = path[path.len() - 1];
        cur.as_table_mut()
            .expect("overlay path traverses non-table")
            .insert(last.to_string(), value);
    }

    /// is this key in the overlay (deviates from config)?
    pub fn contains(&self, path: &[&str]) -> bool {
        let Some(tree) = self.tree.as_ref() else {
            return false;
        };
        let mut cur = tree;
        for key in &path[..path.len() - 1] {
            match cur.get(*key) {
                Some(v) => cur = v,
                None => return false,
            }
        }
        cur.get(path[path.len() - 1]).is_some()
    }

    /// remove one key (dotted path) — the config default
    /// returns. A missing key is a no-op
    pub fn remove(&mut self, path: &[&str]) {
        let Some(tree) = self.tree.as_mut() else {
            return;
        };
        let Some(table) = tree.as_table_mut() else {
            return;
        };
        let mut cur = table;
        for key in &path[..path.len() - 1] {
            let Some(next) = cur.get_mut(*key) else {
                return;
            };
            let Some(next) = next.as_table_mut() else {
                return;
            };
            cur = next;
        }
        cur.remove(path[path.len() - 1]);
    }
}

/// deep-merge overlay into base: tables recurse, everything
/// else (scalars, arrays) replaces. pub(crate) — config::load
/// applies the overlay
pub(crate) fn merge(base: &mut toml::Value, overlay: &toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(b), toml::Value::Table(o)) => {
            for (k, v) in o {
                match b.entry(k.clone()) {
                    toml::map::Entry::Occupied(mut e) => merge(e.get_mut(), v),
                    toml::map::Entry::Vacant(e) => {
                        e.insert(v.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// $XDG_STATE_HOME/mestro, per the base dir spec ~/.local/state
pub(crate) fn state_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .unwrap_or_default();
            home.join(".local/state")
        })
        .join("mestro")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_scalars_replace_tables_recurse() {
        let mut base = toml::from_str::<toml::Value>(
            "[now-playing]\ntitle-size = 24\ninline-vu = \"led\"\n\n[theme]\npalette = \"amber\"\n",
        )
        .unwrap();
        let overlay = toml::from_str::<toml::Value>("[now-playing]\ntitle-size = 20\n").unwrap();
        merge(&mut base, &overlay);
        let np = base.get("now-playing").unwrap();
        assert_eq!(np.get("title-size").unwrap().as_integer(), Some(20));
        assert_eq!(np.get("inline-vu").unwrap().as_str(), Some("led"));
        assert_eq!(
            base.get("theme").unwrap().get("palette").unwrap().as_str(),
            Some("amber")
        );
    }

    #[test]
    fn merge_arrays_replace_not_concat() {
        let mut base = toml::from_str::<toml::Value>("controls = [1, 2]\n").unwrap();
        let overlay = toml::from_str::<toml::Value>("controls = [3]\n").unwrap();
        merge(&mut base, &overlay);
        let arr = base.get("controls").unwrap().as_array().unwrap();
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn overlay_stays_sparse_on_set() {
        let mut o = Overlay::default();
        o.set(&["now-playing", "title-size"], 20.into());
        let text = toml::to_string_pretty(o.tree.as_ref().unwrap()).unwrap();
        assert!(text.contains("title-size = 20"));
        assert!(!text.contains("inline-vu"));
        // a second set keeps the first
        o.set(&["theme", "palette"], "cover".into());
        let text = toml::to_string_pretty(o.tree.as_ref().unwrap()).unwrap();
        assert!(text.contains("title-size = 20"));
        assert!(text.contains("palette = \"cover\""));
    }

    #[test]
    fn set_rejects_a_bad_value_even_with_a_broken_base() {
        let base = toml::from_str::<toml::Value>("[display]\nnope = 1\n").unwrap();
        let handle = Handle::new(Overlay::default(), base, None);
        // an edit to an unrelated key works despite the base's typo
        assert!(handle.set("theme.palette", "cover".into()).is_ok());
        // a bad value for the edited key is still refused
        assert!(handle
            .set("clock.background.source", "sparkles".into())
            .is_err());
        assert!(!handle.is_overridden("clock.background.source"));
    }

    #[test]
    fn overlay_unknown_key_rejected_by_config_schema() {
        // the merged tree must parse as Config — unknown keys
        // fail the parse (deny_unknown_fields), which the
        // loader turns into warn-and-drop
        let base = toml::from_str::<toml::Value>("").unwrap();
        let mut merged = base;
        let overlay = toml::from_str::<toml::Value>("[now-playing]\nno-such-key = 1\n").unwrap();
        merge(&mut merged, &overlay);
        assert!(merged.try_into::<crate::config::Config>().is_err());
    }
}
