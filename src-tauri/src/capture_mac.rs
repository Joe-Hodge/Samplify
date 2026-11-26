// macOS-only system-audio capture via ScreenCaptureKit.
//
// Preserves Samplify's defining property: the rolling buffer is always
// capturing whatever the system is playing, with zero ongoing user-managed
// routing. ScreenCaptureKit follows system audio regardless of which output
// device is currently active (speakers, headphones, AirPods) — the exact
// property WASAPI loopback preserves on Windows.
//
// What we do NOT add here: a device picker, output-device selector, or manual
// routing config UI. SCStream captures default system audio; that's the
// whole point — a picker would reintroduce the routing fragility BlackHole+
// cpal suffers from.
//
// !!! UNVERIFIED ON Mac — written from docs.rs API references, not compiled.
// Needs first compile on a macOS 14+ machine (`xcode-select --install` first).
// All "TODO(MAC-VERIFY):" items must be hit during owner Mac bring-up and
// either confirmed or fixed; STATUS.md tracks the live-test pass.

#![cfg(target_os = "macos")]

use anyhow::{anyhow, Result};
use screencapturekit::prelude::*;
use std::sync::{Arc, Mutex, OnceLock};

use crate::capture::{CaptureAuth, RingBuffer};

/// The configured capture format (system-audio SCStream delivers 48 kHz
/// stereo F32 once `with_captures_audio(true).with_sample_rate(48_000)`
/// `.with_channel_count(2)` is set on `SCStreamConfiguration`).
pub fn device_format() -> Result<(u32, u16)> {
    Ok((48_000, 2))
}

/// Authorization state probe. **Does NOT trigger the TCC prompt** — only
/// reads whatever the last `start()` set. Use `start()` first; this is for
/// the frontend to poll whether capture is currently authorized.
pub fn authorization() -> CaptureAuth {
    *auth_state().lock().unwrap()
}

fn auth_state() -> &'static Mutex<CaptureAuth> {
    static STATE: OnceLock<Mutex<CaptureAuth>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(CaptureAuth::NotDetermined))
}

fn set_auth_state(s: CaptureAuth) {
    *auth_state().lock().unwrap() = s;
}

/// Start an audio-only SCStream on the main display, feeding interleaved f32
/// samples into the shared `RingBuffer`.
///
/// The first `SCShareableContent::get()` call triggers macOS's "Screen
/// Recording" TCC prompt (system-audio capture is bundled under Screen
/// Recording permission — this is a known oddity, not that we capture video:
/// `SCStreamOutputType::Audio` is the only output type we register). If the
/// user grants permission, the stream starts and pushes audio into the ring
/// buffer. If denied, this returns Err and surfaces the failure so the
/// frontend can show the "Open System Settings" banner.
///
/// TCC behavior note: macOS does NOT re-prompt after a denial. The user must
/// open System Settings → Privacy & Security → Screen Recording, toggle
/// Samplify on, then **restart Samplify** for the engine to pick up capture.
/// Our "not authorized" banner in the library empty-state reflects this.
pub fn start(shared: Arc<Mutex<RingBuffer>>) -> Result<CaptureEngine> {
    let content = match SCShareableContent::get() {
        Ok(c) => {
            set_auth_state(CaptureAuth::Authorized);
            c
        }
        Err(e) => {
            log::error!("SCShareableContent::get failed (TCC denied?): {e}");
            set_auth_state(CaptureAuth::Denied);
            return Err(anyhow!("Screen Recording permission required: {e}"));
        }
    };

    let display = content
        .displays()
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no displays found"))?;

    let filter = SCContentFilter::create()
        .with_display(&display)
        .with_excluding_windows(&[])
        .build();

    // Audio-only: no width/height, no pixel format. We DO set
    // captures_audio + rate + channel count so SCStream delivers 48 kHz
    // stereo f32 samples regardless of the actual output device format.
    let config = SCStreamConfiguration::new()
        .with_captures_audio(true)
        .with_sample_rate(48_000)
        .with_channel_count(2);

    let mut stream = SCStream::new(&filter, &config);

    // Closure handler fires on Apple's dispatch queue (NOT our Tokio runtime).
    // Keep it cheap and return — never block the dispatch thread.
    // Audio arrives DE-INTERLEAVED: one AudioBuffer per channel in the list.
    // We interleave L/R frames before pushing, matching the RingBuffer's
    // interleaved-f32 contract that the rest of the app already assumes
    // (see the `get_range_is_frame_aligned_for_stereo` regression test in
    // capture.rs — an odd offset in a stereo clip swaps L/R for the whole
    // clip, so the interleaving here must produce exact L,R,L,R frames).
    stream.add_output_handler(
        move |sample: CMSampleBuffer, of_type: SCStreamOutputType| {
            if of_type != SCStreamOutputType::Audio {
                return;
            }
            let Some(list) = sample.audio_buffer_list() else { return };

            let n = list.num_buffers();
            if n == 0 {
                return;
            }

            // Pull each channel's data slice as f32 (LE).
            // TODO(MAC-VERIFY): the exact `AudioBufferRef::data()` return
            // shape (slice type / element type) needs confirming against the
            // 8.0 source on first Mac compile. The intent: per-channel Vec<f32>,
            // one frame per element.
            let mut channels: Vec<Vec<f32>> = Vec::with_capacity(n);
            for i in 0..n {
                let Some(buf_ref) = list.buffer(i) else { continue };
                let bytes = buf_ref.data();
                let samples: Vec<f32> = bytes
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                    .collect();
                channels.push(samples);
            }

            let channels_count = channels.len();
            if channels_count == 0 {
                return;
            }

            // If we got a single interleaved buffer already (some SC configs
            // deliver this), push it as-is.
            if channels_count == 1 {
                if let Ok(mut rb) = shared.lock() {
                    rb.push(&channels[0]);
                }
                return;
            }

            // De-interleaved stereo (the common case): interleave L/R.
            let frame_count = channels.iter().map(|c| c.len()).min().unwrap_or(0);
            let mut interleaved: Vec<f32> = Vec::with_capacity(frame_count * channels_count);
            for f in 0..frame_count {
                for c in 0..channels_count {
                    interleaved.push(channels[c][f]);
                }
            }
            if let Ok(mut rb) = shared.lock() {
                rb.push(&interleaved);
            }
        },
        SCStreamOutputType::Audio,
    );

    // TODO(MAC-VERIFY): SCStream may dispatch stream-stop errors via a
    // delegate (SCStreamDelegateTrait::did_stop_with_error) rather than
    // returning from start_capture. For now we only handle the synchronous
    // Result — add an ErrorHandler delegate if start_capture surfaces errors
    // via delegate. See crate's ErrorHandler::new closure pattern (the lib.rs
    // README mentions `SCStream::new_with_delegate`).
    stream
        .start_capture()
        .map_err(|e| anyhow!("SCStream::start_capture failed: {e:?}"))?;

    Ok(CaptureEngine {
        _stream: Stream(stream),
        sample_rate: 48_000,
        channels: 2,
    })
}

/// Keeps the ScreenCaptureKit stream and its delegate alive in Tauri's managed
/// state. The audio callback shares only the separately synchronized PCM ring.
pub struct CaptureEngine {
    _stream: Stream,
    pub sample_rate: u32,
    pub channels: u16,
}

struct Stream(SCStream);
unsafe impl Send for Stream {}
unsafe impl Sync for Stream {}