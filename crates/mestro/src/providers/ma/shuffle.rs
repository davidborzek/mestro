use crate::providers::SourceCommand;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct ShufflePlay {
    pending: Option<Pending>,
}

struct Pending {
    id: u64,
    queue: String,
    uri: String,
    at: Instant,
}

impl ShufflePlay {
    pub fn start(&mut self, id: u64, queue: String, uri: String) {
        self.pending = Some(Pending {
            id,
            queue,
            uri,
            at: Instant::now(),
        });
    }
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub fn reply(&mut self, id: u64, queue: &str, failed: bool) -> Option<SourceCommand> {
        if self.pending.as_ref()?.id != id {
            return None;
        }
        let pending = self.pending.take()?;
        if failed || pending.queue != queue || pending.at.elapsed() > Duration::from_secs(5) {
            return None;
        }
        Some(SourceCommand::PlayMediaShuffled { uri: pending.uri })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn play_waits_for_its_shuffle_reply_and_is_consumed_once() {
        let mut play = ShufflePlay::default();
        play.start(10, "queue".into(), "library://album/1".into());
        assert!(play.reply(9, "queue", false).is_none());
        assert!(
            matches!(play.reply(10, "queue", false), Some(SourceCommand::PlayMediaShuffled { uri }) if uri == "library://album/1")
        );
        assert!(play.reply(10, "queue", false).is_none());
    }
    #[test]
    fn failed_expired_superseded_or_other_queue_requests_never_start_playback() {
        let mut play = ShufflePlay::default();
        play.start(1, "queue".into(), "album".into());
        assert!(play.reply(1, "queue", true).is_none());
        play.start(2, "queue".into(), "album".into());
        assert!(play.reply(2, "other", false).is_none());
        play.start(3, "queue".into(), "album".into());
        play.pending.as_mut().unwrap().at = Instant::now() - Duration::from_secs(6);
        assert!(play.reply(3, "queue", false).is_none());
        play.start(4, "queue".into(), "album".into());
        play.cancel();
        assert!(play.reply(4, "queue", false).is_none());
        play.start(5, "queue".into(), "album".into());
        play.start(6, "queue".into(), "playlist".into());
        assert!(play.reply(5, "queue", false).is_none());
        assert!(
            matches!(play.reply(6, "queue", false), Some(SourceCommand::PlayMediaShuffled { uri }) if uri == "playlist")
        );
    }
}
