#[derive(Clone)]
pub struct Sample {
    pub track_ms: i64,
    pub is_playing: bool,
    pub received_at_ms: i64,
    pub spotify_id: String,
}

#[derive(Default)]
pub struct Clock {
    last: Option<Sample>,
}

impl Clock {
    pub fn update(&mut self, s: Sample) {
        // Newest wins. (Phase 2 Spicetify push and Phase 1 polling both call this;
        // callers pass monotonically increasing received_at_ms.)
        match &self.last {
            Some(prev) if prev.received_at_ms > s.received_at_ms => {}
            _ => self.last = Some(s),
        }
    }

    pub fn position(&self, now_ms: i64) -> Option<(String, i64)> {
        let s = self.last.as_ref()?;
        let ms = if s.is_playing { s.track_ms + (now_ms - s.received_at_ms).max(0) } else { s.track_ms };
        Some((s.spotify_id.clone(), ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolates_forward_while_playing() {
        let mut c = Clock::default();
        c.update(Sample { track_ms: 10_000, is_playing: true, received_at_ms: 1_000, spotify_id: "s".into() });
        let (_, ms) = c.position(1_500).unwrap();
        assert_eq!(ms, 10_500); // +500ms elapsed
    }

    #[test]
    fn holds_position_while_paused() {
        let mut c = Clock::default();
        c.update(Sample { track_ms: 42_000, is_playing: false, received_at_ms: 1_000, spotify_id: "s".into() });
        let (_, ms) = c.position(9_000).unwrap();
        assert_eq!(ms, 42_000);
    }

    #[test]
    fn none_before_any_sample() {
        let c = Clock::default();
        assert!(c.position(1000).is_none());
    }

    #[test]
    fn newer_sample_replaces_older() {
        let mut c = Clock::default();
        c.update(Sample { track_ms: 10_000, is_playing: true, received_at_ms: 1_000, spotify_id: "a".into() });
        c.update(Sample { track_ms: 0, is_playing: true, received_at_ms: 2_000, spotify_id: "b".into() });
        let (id, ms) = c.position(2_250).unwrap();
        assert_eq!(id, "b");
        assert_eq!(ms, 250);
    }
}
