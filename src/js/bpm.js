// bpm.js — pure BPM detection, no DOM/Web Audio deps so node can unit-test it.
// Algorithm: one-pole lowpass (kicks live below ~150 Hz) → peak picking with a
// refractory window → consecutive-interval tempo candidates folded into 70–180
// BPM → largest agreeing cluster wins. Confidence = cluster share of candidates.

export function lowpass(samples, sampleRate, cutoffHz = 150) {
  const out = new Float32Array(samples.length);
  const rc = 1 / (2 * Math.PI * cutoffHz);
  const dt = 1 / sampleRate;
  const alpha = dt / (rc + dt);
  let y = 0;
  for (let i = 0; i < samples.length; i++) {
    y += alpha * (samples[i] - y);
    out[i] = y;
  }
  return out;
}

export function detectBpm(samples, sampleRate) {
  if (!samples || samples.length < sampleRate) return { bpm: null, confidence: 0 };
  const low = lowpass(samples, sampleRate, 150);

  let max = 0;
  let sumSq = 0;
  for (let i = 0; i < low.length; i++) {
    const a = Math.abs(low[i]);
    if (a > max) max = a;
    sumSq += low[i] * low[i];
  }
  if (max < 1e-4) return { bpm: null, confidence: 0 };

  // Crest-factor gate: beats are impulsive (spiky peaks over near-silence, crest
  // ~10); noise/pads/drones are statistically flat (crest ~4) and would otherwise
  // yield refractory-spaced fake peaks that cluster into a confident wrong tempo.
  const rms = Math.sqrt(sumSq / low.length);
  if (max / rms < 5) return { bpm: null, confidence: 0 };

  // Peaks: >= 70% of max, with a 250ms refractory window (240 BPM ceiling).
  const thresh = 0.7 * max;
  const refractory = Math.floor(0.25 * sampleRate);
  const peaks = [];
  let i = 0;
  while (i < low.length) {
    if (Math.abs(low[i]) >= thresh) {
      let best = i;
      const end = Math.min(low.length, i + refractory);
      for (let j = i; j < end; j++) {
        if (Math.abs(low[j]) > Math.abs(low[best])) best = j;
      }
      peaks.push(best);
      i = best + refractory;
    } else {
      i++;
    }
  }
  if (peaks.length < 4) return { bpm: null, confidence: 0 };

  // Tempo candidates from consecutive peak intervals, octave-folded into 70–180.
  const cands = [];
  for (let k = 1; k < peaks.length; k++) {
    const sec = (peaks[k] - peaks[k - 1]) / sampleRate;
    if (sec < 0.01) continue;
    let bpm = 60 / sec;
    while (bpm < 70) bpm *= 2;
    while (bpm > 180) bpm /= 2;
    cands.push(bpm);
  }
  if (cands.length < 3) return { bpm: null, confidence: 0 };

  let bestBpm = null;
  let bestCount = 0;
  for (const c of cands) {
    const cluster = cands.filter((x) => Math.abs(x - c) / c < 0.04);
    if (cluster.length > bestCount) {
      bestCount = cluster.length;
      bestBpm = cluster.reduce((a, b) => a + b, 0) / cluster.length;
    }
  }
  return { bpm: Math.round(bestBpm), confidence: bestCount / cands.length };
}

// Snap a loop to a whole number of beats so synced lanes stay phase-locked.
// Prefers rounding; falls back to flooring if rounding overruns the buffer.
export function snapLoopSeconds(rawLoopSec, bpm, maxSec = Infinity) {
  const beat = 60 / bpm;
  let beats = Math.round(rawLoopSec / beat);
  if (beats < 1) beats = 1;
  if (beats * beat > maxSec) beats = Math.floor(maxSec / beat);
  if (beats < 1) return null;
  return beats * beat;
}
