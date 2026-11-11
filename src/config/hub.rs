//! [hub]: the hub page layout

use serde::Deserialize;

/// which hub layout to render
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HubLayout {
    /// the console-row glass cards (default)
    #[default]
    Cards,
    /// the editorial word list — full-width rows, big letterspaced
    /// words, hairline separators
    List,
}

/// one hub destination. The set is fixed — `order` only
/// permutes it
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HubEntry {
    Library,
    Search,
    Settings,
    Standby,
}

fn default_order() -> Vec<HubEntry> {
    vec![
        HubEntry::Library,
        HubEntry::Search,
        HubEntry::Settings,
        HubEntry::Standby,
    ]
}

impl HubEntry {
    /// the fixed set — `order` only permutes it (validate()
    /// re-appends missing entries)
    pub const ALL: [HubEntry; 4] = [
        HubEntry::Library,
        HubEntry::Search,
        HubEntry::Settings,
        HubEntry::Standby,
    ];

    /// the config spelling ([hub].order entries)
    pub fn wire_name(self) -> &'static str {
        match self {
            HubEntry::Library => "library",
            HubEntry::Search => "search",
            HubEntry::Settings => "settings",
            HubEntry::Standby => "standby",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "kebab-case")]
pub struct Hub {
    /// see HubLayout
    #[serde(default)]
    pub layout: HubLayout,
    /// display order of the entries; duplicates warn and dedupe,
    /// omitted entries append at the end (warn) — the hub never
    /// silently loses a destination
    #[serde(default = "default_order")]
    pub order: Vec<HubEntry>,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            layout: HubLayout::default(),
            order: default_order(),
        }
    }
}

impl Hub {
    pub fn validate(&mut self) {
        let before = self.order.len();
        let mut seen = std::collections::HashSet::new();
        self.order.retain(|e| seen.insert(*e));
        if self.order.len() != before {
            tracing::warn!("hub order has duplicates, keeping first occurrences");
        }
        // the destination set is fixed — omitted entries append
        for e in default_order() {
            if !self.order.contains(&e) {
                tracing::warn!("hub order omits {e:?}, appending it");
                self.order.push(e);
            }
        }
    }
}
