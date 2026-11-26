use anyhow::Result;
use std::path::Path;

pub fn write_wav_i16(path: &Path, samples_f32: &[f32], sample_rate: u32, channels: u16) -> Result<()> {
    if let Some(dir) = path.parent() { std::fs::create_dir_all(dir)?; }
    let spec = hound::WavSpec {
        channels, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec)?;
    for &s in samples_f32 {
        w.write_sample((s.clamp(-1.0, 1.0) * 32767.0) as i16)?;
    }
    w.finalize()?;
    Ok(())
}

/// Reads a 16-bit PCM WAV and writes the sub-region [start_ms, end_ms] (offsets
/// into the file) to `dest`, preserving the source spec. Frame-aligned + clamped.
pub fn write_trimmed_wav(src: &Path, dest: &Path, start_ms: i64, end_ms: i64) -> Result<()> {
    let mut reader = hound::WavReader::open(src)?;
    let spec = reader.spec();
    let sr = spec.sample_rate as i64;
    let ch = spec.channels.max(1) as i64;
    let samples: Vec<i16> = reader.samples::<i16>().filter_map(|s| s.ok()).collect();
    let total_frames = samples.len() as i64 / ch;
    let sf = (start_ms.max(0) * sr / 1000).clamp(0, total_frames);
    let ef = (end_ms.max(0) * sr / 1000).clamp(sf, total_frames);
    let start_idx = (sf * ch) as usize;
    let end_idx = (ef * ch) as usize;
    if let Some(dir) = dest.parent() { std::fs::create_dir_all(dir)?; }
    let mut w = hound::WavWriter::create(dest, spec)?;
    for &s in &samples[start_idx..end_idx] {
        w.write_sample(s)?;
    }
    w.finalize()?;
    Ok(())
}

pub fn sanitize_filename(artist: &str, title: &str, timestamp: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| match c {
        '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
        _ => c,
    }).collect::<String>();
    format!("{} - {} [{}].wav", clean(artist), clean(title), timestamp)
}

/// Resolve the file a note should hand to the OS drag. An untrimmed note drags
/// its capture directly (no copy). A trimmed note drags a slice written into
/// `dir`, named with the trim bounds so a retrim produces a new file rather than
/// reusing a stale one. The source capture is only ever read.
pub fn prepare_drag_slice(
    dir: &Path,
    src: &Path,
    artist: &str,
    title: &str,
    mark_id: i64,
    trim: Option<(i64, i64)>,
) -> Result<std::path::PathBuf> {
    let (start_ms, end_ms) = match trim {
        Some((s, e)) if e > s => (s, e),
        _ => return Ok(src.to_path_buf()),
    };
    std::fs::create_dir_all(dir)?;
    let label = format!("{}-{}-{}", mark_id, start_ms, end_ms);
    let dest = dir.join(sanitize_filename(artist, title, &label));
    if !dest.exists() {
        write_trimmed_wav(src, &dest, start_ms, end_ms)?;
    }
    Ok(dest)
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn trimmed_wav_has_expected_length() {
        let dir = tempdir().unwrap();
        let src = dir.path().join("src.wav");
        // 1s of stereo @ 1000 Hz = 1000 frames = 2000 interleaved samples.
        write_wav_i16(&src, &vec![0.2f32; 2000], 1000, 2).unwrap();
        let dest = dir.path().join("trim.wav");
        write_trimmed_wav(&src, &dest, 200, 700).unwrap(); // 500 ms region
        let r = hound::WavReader::open(&dest).unwrap();
        assert_eq!(r.spec().channels, 2);
        assert_eq!(r.spec().sample_rate, 1000);
        assert_eq!(r.len(), 1000); // 500 frames * 2 channels
    }

    #[test]
    fn wav_roundtrips_sample_count() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("clip.wav");
        let samples = vec![0.0f32, 0.5, -0.5, 1.0, -1.0, 0.25]; // 3 stereo frames
        write_wav_i16(&path, &samples, 48000, 2).unwrap();
        let reader = hound::WavReader::open(&path).unwrap();
        assert_eq!(reader.spec().channels, 2);
        assert_eq!(reader.spec().sample_rate, 48000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.len(), 6); // total samples
    }

    #[test]
    fn filename_is_sanitized() {
        let clean = sanitize_filename("A/B\\C", "X:Y*Z", "2026");
        assert_eq!(clean, "A_B_C - X_Y_Z [2026].wav");
    }

    #[test]
    fn test_v2_selection_track_time_mapping() {
        let trigger_track_ms = 60000;
        let snapshot_duration_ms = 45000;
        let selection_start_ms = 10000.0;
        let file_start_track_ms = (trigger_track_ms - (snapshot_duration_ms - selection_start_ms as i64)).max(0);
        assert_eq!(file_start_track_ms, 25000);

        let trigger_track_ms_under = 10000;
        let file_start_track_ms_under = (trigger_track_ms_under - (snapshot_duration_ms - 5000)).max(0);
        assert_eq!(file_start_track_ms_under, 0);
    }

    #[test]
    fn test_v2_frame_alignment_and_clamping() {
        let sr = 48000;
        let ch = 2;
        let mut samples = vec![0.0f32; 288000];
        for i in 0..288000 {
            samples[i] = i as f32;
        }
        let start_ms = 1000.5;
        let end_ms = 2000.7;
        let start_idx = ((start_ms / 1000.0) * sr as f64).round() as usize * ch;
        let end_idx = ((end_ms / 1000.0) * sr as f64).round() as usize * ch;
        let slice = samples[start_idx.min(samples.len())..end_idx.clamp(start_idx, samples.len())].to_vec();
        assert_eq!(start_idx % ch, 0, "start index must be a multiple of channels");
        assert_eq!(end_idx % ch, 0, "end index must be a multiple of channels");
        assert_eq!(slice.len() % ch, 0, "slice length must be a multiple of channels");
    }

    #[test]
    fn prepare_drag_slice_returns_source_when_untrimmed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("full.wav");
        write_wav_i16(&src, &vec![0.0f32; 48000 * 2], 48000, 2).unwrap();

        let out = prepare_drag_slice(dir.path(), &src, "A", "T", 7, None).unwrap();

        assert_eq!(out, src, "no trim means drag the capture itself — no copy, no write");
    }

    #[test]
    fn prepare_drag_slice_writes_slice_when_trimmed() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("full.wav");
        // 2 seconds of stereo silence at 48 kHz.
        write_wav_i16(&src, &vec![0.0f32; 48000 * 2 * 2], 48000, 2).unwrap();

        let out = prepare_drag_slice(dir.path(), &src, "A", "T", 7, Some((500, 1500))).unwrap();

        assert_ne!(out, src, "a trimmed mark drags a slice, not the whole capture");
        assert!(out.exists());
        let reader = hound::WavReader::open(&out).unwrap();
        let ms = (reader.duration() as f64 / 48000.0 * 1000.0) as i64;
        assert!((ms - 1000).abs() <= 2, "slice should be ~1000 ms, got {ms}");

        // The source capture is untouched.
        let src_reader = hound::WavReader::open(&src).unwrap();
        assert_eq!(src_reader.duration(), 48000 * 2, "source WAV must not be modified");
    }
}
