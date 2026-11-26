import test from "node:test";
import assert from "node:assert/strict";
import { encodeWav, barSeconds, BAR_COUNT } from "../src/js/arrangement.js";

test("writes interleaved stereo PCM with signed extrema and a complete RIFF header", () => {
  const tracks = [[1, -2], [-1, 0.5]];
  const bytes = encodeWav({numberOfChannels: 2, length: 2, sampleRate: 48000, getChannelData: ch => tracks[ch]});
  const view = new DataView(bytes);
  assert.equal(bytes.byteLength, 52);
  assert.equal(view.getUint32(4, true), 44);
  assert.equal(view.getUint32(24, true), 48000);
  assert.equal(view.getUint32(40, true), 8);
  assert.deepEqual([44, 46, 48, 50].map(p => view.getInt16(p, true)), [32767, -32768, -32768, 16384]);
});

test("bounds imported PCM before allocating and keeps four full bars at the session tempo", () => {
  assert.throws(() => encodeWav({numberOfChannels: 6}), /mono or stereo/);
  assert.throws(() => encodeWav({numberOfChannels: 2, length: 40000000}), /128 MB/);
  assert.equal(BAR_COUNT * barSeconds(120), 8);
  assert.equal(BAR_COUNT * barSeconds(96), 10);
});
