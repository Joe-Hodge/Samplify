import { test } from "node:test";
import assert from "node:assert/strict";
import { detectBpm, snapLoopSeconds } from "../src/js/bpm.js";

// Synthetic "kick" track: 50ms 60Hz sine bursts with a fast decay, one per beat.
function clickTrack(bpm, seconds, sampleRate = 44100) {
  const n = Math.floor(seconds * sampleRate);
  const out = new Float32Array(n);
  const period = 60 / bpm;
  for (let t = 0; t < seconds; t += period) {
    const start = Math.floor(t * sampleRate);
    for (let i = 0; i < Math.floor(0.05 * sampleRate) && start + i < n; i++) {
      out[start + i] += Math.sin((2 * Math.PI * 60 * i) / sampleRate) * Math.exp(-i / (0.01 * sampleRate));
    }
  }
  return out;
}

// Deterministic pseudo-noise (no Math.random so failures reproduce).
function noise(seconds, sampleRate = 44100) {
  const n = Math.floor(seconds * sampleRate);
  const out = new Float32Array(n);
  let seed = 1234567;
  for (let i = 0; i < n; i++) {
    seed = (seed * 1103515245 + 12345) & 0x7fffffff;
    out[i] = (seed / 0x3fffffff) - 1;
  }
  return out;
}

for (const bpm of [90, 120, 174]) {
  test(`detects ${bpm} BPM click track within ±2`, () => {
    const { bpm: got, confidence } = detectBpm(clickTrack(bpm, 12), 44100);
    assert.ok(got !== null, "expected a bpm");
    assert.ok(Math.abs(got - bpm) <= 2, `expected ~${bpm}, got ${got}`);
    assert.ok(confidence >= 0.6, `expected confidence >= 0.6, got ${confidence}`);
  });
}

test("silence yields no bpm", () => {
  const { bpm } = detectBpm(new Float32Array(44100 * 5), 44100);
  assert.equal(bpm, null);
});

test("noise yields low confidence or no bpm", () => {
  const { bpm, confidence } = detectBpm(noise(8), 44100);
  assert.ok(bpm === null || confidence < 0.6, `noise came back confident: ${bpm} @ ${confidence}`);
});

test("too-short input yields no bpm", () => {
  const { bpm } = detectBpm(clickTrack(120, 0.5), 44100);
  assert.equal(bpm, null);
});

test("snapLoopSeconds rounds to whole beats", () => {
  // 3.9s at 120 BPM (beat = 0.5s) → 8 beats = 4.0s
  assert.ok(Math.abs(snapLoopSeconds(3.9, 120, 10) - 4.0) < 1e-9);
});

test("snapLoopSeconds clamps up to at least one beat", () => {
  // 0.2s at 120 BPM rounds to 0 beats → clamps to 1 beat = 0.5s
  assert.ok(Math.abs(snapLoopSeconds(0.2, 120, 10) - 0.5) < 1e-9);
});

test("snapLoopSeconds returns null when one beat can't fit", () => {
  assert.equal(snapLoopSeconds(0.2, 120, 0.4), null);
});

test("snapLoopSeconds floors when rounding would exceed the buffer", () => {
  // 3.9s at 120 BPM wants 4.0s but only 3.95s is available → 7 beats = 3.5s
  assert.ok(Math.abs(snapLoopSeconds(3.9, 120, 3.95) - 3.5) < 1e-9);
});
