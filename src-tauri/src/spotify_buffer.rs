//! A Spotify take owns one uninterrupted stretch of one song. All times here
//! are monotonic. The endpoint mix never feeds this buffer.
use crate::capture::RingBuffer;
use std::time::{Duration, Instant};

const UPDATE_TIMEOUT: Duration = Duration::from_secs(5);
const PACKET_TIMEOUT: Duration = Duration::from_millis(350);

struct Playhead {
    id: String,
    position: i64,
    playing: bool,
    received: Instant,
}

impl Playhead {
    fn at(&self, now: Instant) -> i64 {
        self.position
            + if self.playing {
                now.saturating_duration_since(self.received).as_millis() as i64
            } else {
                0
            }
    }
}

pub struct SpotifyBuffer {
    ring: RingBuffer,
    head: Option<Playhead>,
    segment_start: Instant,
    last_packet: Option<(Instant, i64)>,
    pub unavailable: Option<String>,
}

impl SpotifyBuffer {
    pub fn new(rate: u32, channels: u16, seconds: f64) -> Self {
        Self {
            ring: RingBuffer::new(rate, channels, seconds),
            head: None,
            segment_start: Instant::now(),
            last_packet: None,
            unavailable: None,
        }
    }

    pub fn set_capacity_seconds(&mut self, seconds: f64) {
        self.ring.set_capacity_seconds(seconds);
    }

    pub fn clear_audio(&mut self) {
        self.ring.clear();
        self.last_packet = None;
    }

    pub fn forget_track(&mut self) {
        self.head = None;
        self.clear_audio();
    }

    pub fn update(&mut self, id: &str, position: i64, playing: bool, now: Instant) {
        let reset = self.head.as_ref().is_none_or(|old| {
            old.id != id
                || now.duration_since(old.received) > UPDATE_TIMEOUT
                || (old.at(now) - position).abs() > 700
                || (!old.playing && playing)
        });
        if reset {
            self.clear_audio();
            self.segment_start = now;
        }
        self.head = Some(Playhead {
            id: id.to_owned(),
            position,
            playing,
            received: now,
        });
    }

    pub fn push(&mut self, pcm: &[f32], end: Instant, discontinuity: bool) {
        let Some(head) = self.head.as_ref() else {
            return;
        };
        if !head.playing
            || end < self.segment_start
            || end.saturating_duration_since(head.received) > UPDATE_TIMEOUT
        {
            return;
        }
        let position =
            head.at(end) - head.received.saturating_duration_since(end).as_millis() as i64;
        if discontinuity
            || self
                .last_packet
                .is_some_and(|(last, _)| end.saturating_duration_since(last) > PACKET_TIMEOUT)
        {
            self.clear_audio();
        }
        // A WASAPI packet may straddle the track change. Keep whole frames
        // falling after the first observation of this playback segment.
        let frames = (end.duration_since(self.segment_start).as_secs_f64()
            * f64::from(self.ring.sample_rate))
        .round() as usize;
        let keep = (frames * usize::from(self.ring.channels)).min(pcm.len());
        if keep > 0 {
            self.ring.push(&pcm[pcm.len() - keep..]);
            self.last_packet = Some((end, position.max(0)));
        }
    }

    pub fn freeze(&self, id: &str, now: Instant) -> Result<(Vec<f32>, i64), String> {
        if let Some(reason) = &self.unavailable {
            return Err(reason.clone());
        }
        let missing = "Play this track in Spotify on this PC, then capture the phrase again.";
        let head = self
            .head
            .as_ref()
            .filter(|h| h.id == id && now.duration_since(h.received) <= UPDATE_TIMEOUT)
            .ok_or(missing)?;
        let (end, position) = self.last_packet.ok_or(missing)?;
        if (head.playing && now.saturating_duration_since(end) > PACKET_TIMEOUT)
            || (head.at(now) - position).abs() > 700
        {
            return Err(missing.into());
        }
        let pcm = self.ring.snapshot();
        if !pcm.iter().any(|s| s.abs() >= 1.0 / 32768.0) {
            return Err(
                "Spotify supplied only silence. Play the track on this PC and capture again."
                    .into(),
            );
        }
        Ok((pcm, position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn new_song_at_fifty_seven_seconds_requires_its_own_packets() {
        let t = Instant::now();
        let mut b = SpotifyBuffer::new(1000, 1, 60.0);
        b.update("previous", 10_000, true, t);
        b.push(&[0.9; 100], t + Duration::from_millis(100), false);
        let switch = t + Duration::from_millis(500);
        b.update("on-video", 57_000, true, switch);
        // A queued packet from before the switch arrives after the metadata.
        b.push(&[0.9; 100], switch - Duration::from_millis(5), false);
        assert!(b.freeze("on-video", switch).is_err());
        b.push(&[0.5; 100], switch + Duration::from_millis(40), false);
        let (pcm, position) = b
            .freeze("on-video", switch + Duration::from_millis(40))
            .unwrap();
        assert_eq!((pcm, position), (vec![0.5; 40], 57_040));
    }

    #[test]
    fn fifty_seconds_on_new_song_cannot_relabel_old_audio() {
        let mut b = SpotifyBuffer::new(1000, 1, 60.0);
        let t = Instant::now();
        b.update("previous", 90_000, true, t);
        b.push(&[0.2; 100], t + Duration::from_millis(100), false);
        b.update("on-video", 0, true, t + Duration::from_secs(1));
        for n in 1..=500 {
            let now = t + Duration::from_millis(1000 + n * 100);
            if n % 5 == 0 {
                b.update("on-video", n as i64 * 100, true, now);
            }
            b.push(&[0.6; 100], now, false);
        }
        let (pcm, end) = b.freeze("on-video", t + Duration::from_secs(51)).unwrap();
        assert_eq!((pcm.len(), end), (50_000, 50_000));
        assert!(pcm.iter().all(|&s| s == 0.6));
    }

    #[test]
    fn gaps_seek_and_silence_never_return_an_older_phrase() {
        let t = Instant::now();
        let mut b = SpotifyBuffer::new(1000, 1, 60.0);
        b.update("a", 10_000, true, t);
        b.push(&[0.4; 100], t + Duration::from_millis(100), false);
        assert!(b.freeze("a", t + Duration::from_secs(2)).is_err());
        b.update("a", 57_000, true, t + Duration::from_secs(2));
        assert!(b.freeze("a", t + Duration::from_secs(2)).is_err());
        b.push(&[0.0; 100], t + Duration::from_millis(2100), false);
        assert!(b.freeze("a", t + Duration::from_millis(2100)).is_err());
        b.push(&[0.8; 100], t + Duration::from_millis(2200), true);
        assert_eq!(
            b.freeze("a", t + Duration::from_millis(2200)).unwrap().0,
            vec![0.8; 100]
        );
        assert!(b.freeze("b", t + Duration::from_millis(2200)).is_err());
    }

    #[test]
    fn pausing_keeps_the_phrase_and_ignores_later_packets() {
        let t = Instant::now();
        let mut b = SpotifyBuffer::new(1000, 1, 60.0);
        b.update("a", 1000, true, t);
        b.push(&[0.5; 100], t + Duration::from_millis(100), false);
        b.update("a", 1100, false, t + Duration::from_millis(100));
        b.push(&[0.0; 100], t + Duration::from_millis(200), false);
        b.update("a", 1100, false, t + Duration::from_secs(3));
        let (pcm, end) = b.freeze("a", t + Duration::from_secs(3)).unwrap();
        assert_eq!(pcm, vec![0.5; 100]);
        assert_eq!(end, 1100);
    }
}
