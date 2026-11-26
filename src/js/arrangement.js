// Four-bar, 4/4 arrangement. The gate and clip run on the same audio clock,
// including in OfflineAudioContext, so an export follows the live bar buttons.
export const BAR_COUNT = 4;
export const ALL_BARS = (1 << BAR_COUNT) - 1;
export const barSeconds = bpm => 4 * 60 / bpm;

export function scheduleLane(context, destination, lane, loop, volume, bpm, when) {
  const source = context.createBufferSource();
  source.buffer = lane.buffer;
  source.loop = true;
  source.loopStart = loop.start;
  source.loopEnd = loop.start + loop.len;
  source.playbackRate.value = loop.rate;
  const gainNode = context.createGain();
  gainNode.gain.value = volume;
  const panNode = context.createStereoPanner();
  panNode.pan.value = lane.pan;
  const gate = context.createGain();
  source.connect(gainNode).connect(panNode).connect(gate).connect(destination);
  gate.gain.value = 0;
  const pattern = schedulePattern(context, gate, lane.bars, bpm, when, 0);
  const offset = ((lane.offset % loop.len) + loop.len) % loop.len;
  source.start(when, loop.start + offset);
  return { source, pattern, gainNode, panNode, gate };
}

export function schedulePattern(context, gate, bars, bpm, when, elapsed) {
  // One repeating control signal per lane. Interpolation between the last
  // and first sample of adjacent bars gives changes a short click-free ramp.
  const steps = 512;
  const envelope = context.createBuffer(1, BAR_COUNT * steps, 8000);
  const values = envelope.getChannelData(0);
  for (let bar = 0; bar < BAR_COUNT; bar++) {
    values.fill((bars >> bar) & 1, bar * steps, (bar + 1) * steps);
  }
  const pattern = context.createBufferSource();
  pattern.buffer = envelope;
  pattern.loop = true;
  pattern.playbackRate.value = steps / (envelope.sampleRate * barSeconds(bpm));
  pattern.connect(gate.gain);
  const cycle = BAR_COUNT * barSeconds(bpm);
  const offset = (Math.max(0, elapsed) % cycle) / cycle * envelope.duration;
  pattern.start(when, offset);
  return pattern;
}

export function encodeWav(buffer) {
  const channels = buffer.numberOfChannels;
  if (channels < 1 || channels > 2) throw new Error("Choose a mono or stereo recording");
  const byteCount = buffer.length * channels * 2;
  if (byteCount + 44 > 128 * 1024 * 1024) throw new Error("Trim the recording to under 128 MB of decoded audio");
  const bytes = new ArrayBuffer(44 + byteCount);
  const view = new DataView(bytes);
  const label = (pos, value) => [...value].forEach((c, i) => view.setUint8(pos + i, c.charCodeAt(0)));
  label(0, "RIFF"); view.setUint32(4, 36 + byteCount, true); label(8, "WAVE");
  label(12, "fmt "); view.setUint32(16, 16, true); view.setUint16(20, 1, true);
  view.setUint16(22, channels, true); view.setUint32(24, buffer.sampleRate, true);
  view.setUint32(28, buffer.sampleRate * channels * 2, true);
  view.setUint16(32, channels * 2, true); view.setUint16(34, 16, true);
  label(36, "data"); view.setUint32(40, byteCount, true);
  const data = Array.from({length: channels}, (_, ch) => buffer.getChannelData(ch));
  let position = 44;
  for (let frame = 0; frame < buffer.length; frame++) {
    for (const channel of data) {
      const sample = Math.min(1, Math.max(-1, channel[frame]));
      view.setInt16(position, Math.round(sample * (sample < 0 ? 32768 : 32767)), true);
      position += 2;
    }
  }
  return bytes;
}
