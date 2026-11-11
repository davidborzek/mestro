//! Cross-cutting UI: the navigation stack, the OSD layer and the
//! accent palette.

pub mod navigation;
pub mod osd;
pub mod palette;

/// the hub destinations, prebuilt in [hub] order — the UI loops
/// them the same way for cards and list
pub fn hub_entries(config: &crate::config::Config) -> Vec<crate::HubEntryData> {
    use crate::config::HubEntry;
    config
        .hub
        .order
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let (kind, icon, label) = match e {
                HubEntry::Library => (0, crate::Icon::LibraryMusic, "LIBRARY"),
                HubEntry::Search => (1, crate::Icon::Search, "SEARCH"),
                HubEntry::Settings => (2, crate::Icon::Settings, "SETTINGS"),
                HubEntry::Standby => (3, crate::Icon::PowerSettingsNew, "STANDBY"),
            };
            crate::HubEntryData {
                icon,
                label: label.into(),
                index_label: format!("{:02}", i + 1).into(),
                kind,
            }
        })
        .collect()
}
