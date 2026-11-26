// Frame-aligned interleaved PCM storage shared by the native capture backends.

use std::collections::VecDeque;

// Platform-specific capture backends declared at crate root in `lib.rs`
// (so their files live at `src/capture_win.rs` / `src/capture_mac.rs`).
#[cfg(target_os = "macos")]
use crate::capture_mac;

pub struct RingBuffer {
    buf: VecDeque<f32>,
    capacity_samples: usize,
    pub sample_rate: u32,
    pub channels: u16,
    last_packet: Option<std::time::Instant>,
}

impl RingBuffer {
    pub fn new(sample_rate: u32, channels: u16, seconds: f64) -> Self {
        let capacity_samples = (sample_rate as f64 * channels as f64 * seconds).ceil() as usize;
        Self { buf: VecDeque::new(), capacity_samples, sample_rate, channels, last_packet: None }
    }

    pub fn set_capacity_seconds(&mut self, seconds: f64) {
        self.capacity_samples = (self.sample_rate as f64 * self.channels as f64 * seconds).ceil() as usize;
        while self.buf.len() > self.capacity_samples {
            self.buf.pop_front();
        }
    }

    pub fn snapshot(&self) -> Vec<f32> {
        self.buf.iter().copied().collect()
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.last_packet = None;
    }

    #[cfg(windows)]
    pub fn push_timed(&mut self, pcm: &[f32], end: std::time::Instant) {
        if self.last_packet.is_some_and(|prev| end.saturating_duration_since(prev).as_millis() > 350) {
            self.clear();
        }
        self.push(pcm);
        self.last_packet = Some(end);
    }

    #[cfg(windows)]
    pub fn fresh_system_snapshot(&self) -> Vec<f32> {
        if self.last_packet.is_some_and(|t| t.elapsed().as_millis() <= 350) { self.snapshot() } else { Vec::new() }
    }

    pub fn len(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    pub fn push(&mut self, samples: &[f32]) {
        self.buf.extend(samples.iter().copied());
        while self.buf.len() > self.capacity_samples { self.buf.pop_front(); }
    }

    pub fn snapshot_last(&self, seconds: f64) -> Vec<f32> {
        let want = (self.sample_rate as f64 * self.channels as f64 * seconds).ceil() as usize;
        let take = want.min(self.buf.len());
        let start = self.buf.len() - take;
        self.buf.iter().skip(start).copied().collect()
    }

    pub fn trim_to_recent_ms(samples: Vec<f32>, sample_rate: u32, channels: u16, allowed_ms: i64) -> Vec<f32> {
        // Keep only the trailing `allowed_ms` of audio, frame-aligned (an odd
        // raw-sample start in a stereo buffer swaps L/R for the whole clip).
        let ch = channels.max(1) as usize;
        let allowed_frames = ((allowed_ms.max(0) as f64 / 1000.0) * sample_rate as f64).round() as usize;
        let keep = allowed_frames * ch;
        if keep >= samples.len() {
            return samples;
        }
        let mut start = samples.len() - keep;
        start -= start % ch; // realign if the buffer length itself wasn't frame-aligned
        samples[start..].to_vec()
    }

    pub fn get_range(&self, now_ms: i64, start_ms: i64, end_ms: i64) -> Vec<f32> {
        let sample_rate = self.sample_rate as f64;
        let ch = self.channels.max(1) as usize;

        let start_ago = (now_ms - start_ms).max(0) as f64 / 1000.0;
        let end_ago = (now_ms - end_ms).max(0) as f64 / 1000.0;

        // Offsets must be frame-aligned: an odd raw-sample offset in a stereo
        // buffer would swap L/R for the whole slice.
        let start_offset = (start_ago * sample_rate).round() as usize * ch;
        let end_offset = (end_ago * sample_rate).round() as usize * ch;
        
        let len = self.buf.len();
        let start_idx = len.saturating_sub(start_offset);
        let end_idx = len.saturating_sub(end_offset);
        
        let start_idx = start_idx.min(len);
        let end_idx = end_idx.clamp(start_idx, len);
        
        self.buf.iter().skip(start_idx).take(end_idx - start_idx).copied().collect()
    }

    pub fn len_frames(&self) -> usize {
        self.buf.len() / self.channels.max(1) as usize
    }
}

/// Authorization state of the system-audio capture backend.
///
/// On Windows, WASAPI loopback needs no permission and the engine always reports
/// `Authorized` once started (and we treat `NotDetermined`/`Denied`/`Restricted`
/// as Windows-impossible — they only emerge from the macOS backend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureAuth {
    Authorized,
    NotDetermined,
    Denied,
    Restricted,
}

/// Report the system-audio authorization state. On Windows this is always
/// `Authorized` (WASAPI loopback is unrestricted); on macOS it reflects the
/// ScreenCaptureKit TCC permission state, which must be checked *before*
/// attempting to start an SCStream.
pub fn authorization() -> CaptureAuth {
    #[cfg(target_os = "windows")]
    { CaptureAuth::Authorized }
    #[cfg(target_os = "macos")]
    { capture_mac::authorization() }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    { CaptureAuth::Denied }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trim_to_recent_keeps_trailing_frames_aligned() {
        // 1000 Hz stereo, 5s of [L=1.0, R=-1.0] frames = 10_000 samples.
        let samples: Vec<f32> = (0..5000).flat_map(|_| [1.0f32, -1.0]).collect();
        // Ask for the last 2s -> 2000 frames -> 4000 samples, starting on L.
        let out = RingBuffer::trim_to_recent_ms(samples.clone(), 1000, 2, 2_000);
        assert_eq!(out.len(), 4_000);
        assert_eq!(out[0], 1.0); // frame-aligned: starts with an L sample
        // Asking for more than exists returns everything unchanged.
        let all = RingBuffer::trim_to_recent_ms(samples.clone(), 1000, 2, 60_000);
        assert_eq!(all.len(), samples.len());
        // Zero/negative allowance never panics and returns empty.
        assert!(RingBuffer::trim_to_recent_ms(samples, 1000, 2, 0).is_empty());
    }

    #[test]
    fn get_range_is_frame_aligned_for_stereo() {
        // 1000 Hz stereo: every frame is [L=1.0, R=-1.0]. Any misalignment
        // would make a returned slice start with -1.0 (an R sample).
        let mut rb = RingBuffer::new(1000, 2, 10.0);
        for _ in 0..5000 { rb.push(&[1.0, -1.0]); }
        // Odd-millisecond window: 5000ms of buffer, ask for 333ms..111ms ago.
        let slice = rb.get_range(5000, 4667, 4889);
        assert_eq!(slice.len() % 2, 0, "slice must contain whole frames");
        assert_eq!(slice[0], 1.0, "slice must start on an L sample");
        assert_eq!(slice[1], -1.0);
    }

    #[test]
    fn trims_to_capacity() {
        // 10 Hz, mono, 2 seconds => capacity 20 frames.
        let mut rb = RingBuffer::new(10, 1, 2.0);
        for i in 0..50 { rb.push(&[i as f32]); }
        assert_eq!(rb.len_frames(), 20);
        let snap = rb.snapshot_last(2.0);
        assert_eq!(snap.len(), 20);
        assert_eq!(snap[0], 30.0); // oldest kept = 50-20
        assert_eq!(snap[19], 49.0);
    }

    #[test]
    fn snapshot_shorter_than_buffer() {
        let mut rb = RingBuffer::new(10, 1, 5.0);
        for i in 0..40 { rb.push(&[i as f32]); }
        let snap = rb.snapshot_last(1.0); // last 10 frames
        assert_eq!(snap.len(), 10);
        assert_eq!(snap[0], 30.0);
    }

    #[test]
    fn snapshot_clamps_when_buffer_underfilled() {
        let mut rb = RingBuffer::new(10, 2, 30.0);
        rb.push(&[0.1, 0.2, 0.3, 0.4]); // 2 stereo frames
        let snap = rb.snapshot_last(30.0);
        assert_eq!(snap.len(), 4); // only what we have
    }

    #[test]
    fn test_get_range_normal_and_underfill() {
        let mut rb = RingBuffer::new(10, 1, 10.0);
        for i in 0..50 {
            rb.push(&[i as f32]);
        }
        let range = rb.get_range(10000, 6000, 9000);
        assert_eq!(range.len(), 30);
        assert_eq!(range[0], 10.0);
        assert_eq!(range[29], 39.0);

        let range_clamped = rb.get_range(10000, 2000, 9000);
        assert_eq!(range_clamped.len(), 40);
        assert_eq!(range_clamped[0], 0.0);
        assert_eq!(range_clamped[39], 39.0);
    }
}
