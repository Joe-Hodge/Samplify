// stems.js — the Stem Mixer: the four stems split from a note, played in sample
// lock-step through Web Audio with per-stem mute / solo / fader, presets, a
// shared transport, and three ways out (export the mix, save it as a note, drag
// it into a DAW). Stem WAVs are read-only here; bouncing goes through the
// backend so exported audio matches what was heard.
import { invoke, convertFileSrc } from "./bridge.js";
import { icon } from "./icons.js";
import { fmtTime } from "./tags.js";

export const RAW_STEMS = ["drums", "bass", "other", "vocals"];

const STEM_META = {
  drums:  { label: "Drums",  token: "--tag-break",   icon: "drum" },
  bass:   { label: "Bass",   token: "--tag-bass",    icon: "bass" },
  other:  { label: "Other",  token: "--tag-texture", icon: "wave" },
  vocals: { label: "Vocals", token: "--tag-vocal",   icon: "mic" },
};

const PRESETS = [
  { id: "all",      label: "All",          mute: [],         solo: [] },
  { id: "karaoke",  label: "Karaoke",      mute: ["vocals"], solo: [] },
  { id: "acapella", label: "Acapella",     mute: [],         solo: ["vocals"] },
  { id: "dnb",      label: "Drums + Bass", mute: [],         solo: ["drums", "bass"] },
];

const PEAK_BUCKETS = 500;

let deps = { showToast: (m) => console.log(m), onPlaybackStart: () => {}, playgroundAddClip: null, startPathsDrag: null, onSetChanged: () => {} };
let root, lanesEl, presetsEl, noteEl, posEl, playBtn, loopBtn, timeEl, seekEl, seekFill, seekKnob, masterEl;

let ctx = null;
let masterGain = null;
let set = null;              // StemSet from the backend
let lanes = [];              // [{ stem, buffer, peaks, gain, muted, soloed, gainNode, source, color }]
let playing = false;
let looping = true;
let anchorPos = 0;           // clip position (s) when playback last (re)started
let anchorCtx = 0;           // AudioContext time it (re)started at
let rafId = null;
let preset = "all";

export function initStemMixer(options = {}) {
  deps = { ...deps, ...options };
  root = document.getElementById("stem-mixer");
  lanesEl = document.getElementById("sm-lanes");
  presetsEl = document.getElementById("sm-presets");
  noteEl = document.getElementById("sm-note");
  posEl = document.getElementById("sm-pos");
  playBtn = document.getElementById("sm-play");
  loopBtn = document.getElementById("sm-loop");
  timeEl = document.getElementById("sm-time");
  seekEl = document.getElementById("sm-seek");
  seekFill = document.getElementById("sm-seek-fill");
  seekKnob = document.getElementById("sm-seek-knob");
  masterEl = document.getElementById("sm-master");

  playBtn.onclick = () => (playing ? pause() : play());
  loopBtn.onclick = () => { looping = !looping; loopBtn.classList.toggle("active", looping); if (playing) restartSources(position()); };
  loopBtn.classList.toggle("active", looping);
  masterEl.oninput = () => applyMix();
  document.getElementById("sm-close").onclick = closeStemMixer;
  document.getElementById("sm-export").onclick = () => bounce(false);
  document.getElementById("sm-save").onclick = () => bounce(true);
  document.getElementById("sm-delete").onclick = deleteStems;
  document.getElementById("sm-drag").addEventListener("mousedown", async (e) => {
    e.preventDefault(); e.stopPropagation();
    if (!set || !deps.startPathsDrag) return;
    try {
      const path = await invoke("prepare_stem_mix_drag", { sourceMarkId: set.source_mark_id, gains: gainsForBounce() });
      await deps.startPathsDrag([path]);
    } catch (err) { deps.showToast(`Drag failed: ${err}`); }
  });

  // Click or drag on the seek bar scrubs; pointer capture keeps a drag alive
  // when the cursor leaves the bar.
  const seekFromEvent = (clientX) => {
    const r = seekEl.getBoundingClientRect();
    const frac = Math.min(1, Math.max(0, (clientX - r.left) / r.width));
    seek(frac * duration());
  };
  seekEl.addEventListener("pointerdown", (e) => { seekEl.setPointerCapture(e.pointerId); seekFromEvent(e.clientX); });
  seekEl.addEventListener("pointermove", (e) => { if (e.buttons === 1) seekFromEvent(e.clientX); });

  presetsEl.innerHTML = PRESETS.map((p) => `<button class="sm-preset" data-preset="${p.id}">${p.label}</button>`).join("");
  presetsEl.querySelectorAll(".sm-preset").forEach((b) => (b.onclick = () => applyPreset(b.dataset.preset)));

  window.addEventListener("keydown", (e) => {
    if (!isStemMixerOpen()) return;
    const t = e.target;
    if (t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable)) return;
    if (e.code === "Space") {
      e.preventDefault();
      playing ? pause() : play();
    } else if (e.code === "KeyL") {
      e.preventDefault();
      looping = !looping;
      loopBtn.classList.toggle("active", looping);
      if (playing) restartSources(position());
    } else if (e.code === "Escape") {
      e.preventDefault();
      closeStemMixer();
    } else if (e.key === "1") {
      e.preventDefault();
      applyPreset("all");
    } else if (e.key === "2") {
      e.preventDefault();
      applyPreset("karaoke");
    } else if (e.key === "3") {
      e.preventDefault();
      applyPreset("acapella");
    } else if (e.key === "4") {
      e.preventDefault();
      applyPreset("dnb");
    }
  });
  window.addEventListener("resize", () => { if (isStemMixerOpen()) drawAll(); });
}

export function isStemMixerOpen() {
  return !!root && root.style.display !== "none";
}

/** Which note's stems are loaded, or null. */
export function stemMixerSourceId() {
  return set ? (set.source_mark_id ?? set.sourceMarkId) : null;
}

/** Playground clip for one mixer lane — the stem WAV, not the source capture. */
export function stemLaneAsPlaygroundClip(lane) {
  if (!set || !lane) return null;
  const filePath = lane.filePath;
  const markId = lane.markId;
  if (!filePath || markId == null) return null;
  const label = STEM_META[lane.stem]?.label || lane.stem;
  return {
    markId,
    title: set.title,
    artist: set.artist,
    noteText: `${set.note_text || set.title} — ${label}`,
    filePath,
    trimStartMs: null,
    trimEndMs: null,
    tags: [],
    rating: null,
    key: set.key || null,
    bpm: set.bpm || null,
    stem: lane.stem,
    sourceMarkId: set.source_mark_id ?? set.sourceMarkId ?? null,
  };
}

/** Soloed stem clips when the mixer is open; empty if nothing is soloed. */
export function mixerSoloedPlaygroundClips() {
  if (!isStemMixerOpen()) return [];
  return lanes.filter((l) => l.soloed).map(stemLaneAsPlaygroundClip).filter(Boolean);
}

/** Open the mixer on the stems of `markId` (a split note, or one of its stems). */
export async function openStemMixer(markId) {
  let next;
  try { next = await invoke("get_stem_set", { markId }); }
  catch (err) { deps.showToast(`${err}`); return; }
  if (!next || !next.stems || !next.stems.length) { deps.showToast("This note has no stems yet"); return; }

  // Pause main clip transport so it doesn't play underneath stems
  if (deps.onPlaybackStart) deps.onPlaybackStart();

  stopSources();
  playing = false;
  anchorPos = 0;
  set = next;
  ensureCtx();
  root.style.display = "block";
  noteEl.textContent = set.note_text || `${set.title} — stems`;
  lanesEl.innerHTML = set.stems.map(() => `<div class="sm-lane sm-lane-loading"></div>`).join("");
  updatePlayButton();

  const decoded = await Promise.all(set.stems.map(async (s) => {
    try {
      const res = await fetch(convertFileSrc(s.file_path || s.filePath));
      if (!res.ok) throw new Error("HTTP " + res.status);
      return await ctx.decodeAudioData(await res.arrayBuffer());
    } catch (e) {
      console.error("stems: decode failed", s.file_path, e);
      return null;
    }
  }));
  if (set !== next) return; // another set was opened while decoding
  lanes = set.stems.map((s, i) => {
    const prev = lanes.find((l) => l.stem === s.stem);
    return {
      stem: s.stem,
      markId: s.mark_id ?? s.markId,
      filePath: s.file_path || s.filePath,
      buffer: decoded[i],
      peaks: decoded[i] ? peaksFor(decoded[i]) : null,
      gain: prev ? prev.gain : 1,
      muted: false,
      soloed: false,
      gainNode: null,
      source: null,
      color: cssColor(STEM_META[s.stem]?.token || "--accent"),
    };
  });
  const missing = lanes.filter((l) => !l.buffer);
  if (missing.length) deps.showToast(`Couldn't load ${missing.map((l) => l.stem).join(", ")} — the file may have moved`);
  posEl.textContent = `${fmtTime(set.track_ms)}–${fmtTime(set.track_ms + duration() * 1000)}`;
  applyPreset("all");
  renderLanes();
  root.scrollIntoView({ block: "nearest", behavior: "smooth" });
}

export function closeStemMixer() {
  pause();
  root.style.display = "none";
  set = null;
  lanes = [];
}

/** Pause the mixer (the main transport is taking over the speakers). */
export function stemMixerPause() {
  if (playing) pause();
}

// ---------- audio ----------

function ensureCtx() {
  if (!ctx) {
    ctx = new (window.AudioContext || window.webkitAudioContext)();
    masterGain = ctx.createGain();
    masterGain.connect(ctx.destination);
    if (deps.connectAudioSource) deps.connectAudioSource(masterGain, ctx);
  }
  if (ctx.state === "suspended") ctx.resume();
}

function duration() {
  let d = 0;
  for (const l of lanes) if (l.buffer && l.buffer.duration > d) d = l.buffer.duration;
  return d;
}

function position() {
  const d = duration();
  if (!playing || !ctx) return Math.min(anchorPos, d);
  // Sources are scheduled 50ms ahead, so the first frames after play() sit
  // slightly before the anchor; clamp rather than report a negative time.
  const p = Math.max(0, anchorPos + (ctx.currentTime - anchorCtx));
  if (d <= 0) return 0;
  return looping ? p % d : Math.min(p, d);
}

function effectiveGain(lane) {
  const anySolo = lanes.some((l) => l.soloed);
  return lane.muted || (anySolo && !lane.soloed) ? 0 : lane.gain;
}

function gainsForBounce() {
  const g = {};
  for (const l of lanes) g[l.stem] = Number(effectiveGain(l).toFixed(3));
  return g;
}

function applyMix() {
  if (ctx) {
    const now = ctx.currentTime;
    for (const l of lanes) {
      if (l.gainNode) {
        const target = effectiveGain(l);
        try {
          l.gainNode.gain.cancelScheduledValues(now);
          l.gainNode.gain.setValueAtTime(target, now);
        } catch (e) {
          l.gainNode.gain.value = target;
        }
      }
    }
    if (masterGain) {
      const masterVal = Number(masterEl.value) / 100;
      try {
        masterGain.gain.cancelScheduledValues(now);
        masterGain.gain.setValueAtTime(masterVal, now);
      } catch (e) {
        masterGain.gain.value = masterVal;
      }
    }
  }
  lanes.forEach((l) => {
    const el = document.getElementById(`sm-lane-${l.stem}`);
    if (el) el.classList.toggle("silent", effectiveGain(l) === 0);
  });
  if (!playing) drawAll();
}

function restartSources(fromPos) {
  stopSources();
  ensureCtx();
  const startAt = ctx.currentTime + 0.05;
  const d = duration();
  for (const l of lanes) {
    if (!l.buffer) continue;
    const src = ctx.createBufferSource();
    src.buffer = l.buffer;
    src.loop = looping;
    src.loopStart = 0;
    src.loopEnd = l.buffer.duration;
    const g = ctx.createGain();
    g.gain.value = effectiveGain(l);
    src.connect(g);
    g.connect(masterGain);
    src.start(startAt, Math.min(Math.max(0, fromPos), Math.max(0, d - 0.01)));
    l.source = src;
    l.gainNode = g;
  }
  anchorPos = fromPos;
  anchorCtx = startAt;
  masterGain.gain.value = Number(masterEl.value) / 100;
}

function stopSources() {
  for (const l of lanes) {
    if (l.source) { try { l.source.stop(); } catch (e) { /* already stopped */ } l.source.disconnect(); l.source = null; }
    if (l.gainNode) { l.gainNode.disconnect(); l.gainNode = null; }
  }
}

function play() {
  if (!lanes.some((l) => l.buffer)) return;
  deps.onPlaybackStart();
  if (deps.onPlaybackStateChange) deps.onPlaybackStateChange(true);
  ensureCtx();
  restartSources(position() >= duration() - 0.02 ? 0 : position());
  playing = true;
  updatePlayButton();
  if (rafId) cancelAnimationFrame(rafId);
  tick();
}

function pause() {
  const p = position();
  stopSources();
  playing = false;
  anchorPos = p;
  if (rafId) cancelAnimationFrame(rafId);
  rafId = null;
  if (deps.onPlaybackStateChange) deps.onPlaybackStateChange(false);
  updatePlayButton();
  drawAll();
}

function seek(t) {
  const d = duration();
  const clamped = Math.max(0, Math.min(d > 0 ? d - 0.02 : 0, t));
  anchorPos = clamped;
  if (playing) restartSources(clamped);
  drawAll();
}

function tick() {
  if (!playing) return;
  const d = duration();
  if (!looping && d > 0 && position() >= d - 0.02) {
    pause();
    anchorPos = 0;
    drawAll();
    return;
  }
  drawAll();
  rafId = requestAnimationFrame(tick);
}

function updatePlayButton() {
  playBtn.innerHTML = icon(playing ? "pause" : "play", 16);
  playBtn.classList.toggle("active", playing);
  playBtn.title = playing ? "Pause (Space)" : "Play stems (Space)";
}

// ---------- presets / lane controls ----------

function applyPreset(id) {
  const p = PRESETS.find((x) => x.id === id) || PRESETS[0];
  preset = p.id;
  for (const l of lanes) {
    l.muted = p.mute.includes(l.stem);
    l.soloed = p.solo.includes(l.stem);
  }
  presetsEl.querySelectorAll(".sm-preset").forEach((b) => b.classList.toggle("active", b.dataset.preset === preset));
  syncLaneButtons();
  applyMix();
}

function markCustom() {
  preset = "custom";
  presetsEl.querySelectorAll(".sm-preset").forEach((b) => b.classList.remove("active"));
}

function syncLaneButtons() {
  for (const l of lanes) {
    const el = document.getElementById(`sm-lane-${l.stem}`);
    if (!el) continue;
    el.querySelector(".sm-mute").classList.toggle("on", l.muted);
    el.querySelector(".sm-solo").classList.toggle("on", l.soloed);
    el.querySelector(".sm-fader").value = Math.round(l.gain * 100);
    const valEl = el.querySelector(".sm-fader-val");
    if (valEl) valEl.textContent = `${Math.round(l.gain * 100)}%`;
  }
}

function toggleMute(lane) {
  lane.muted = !lane.muted;
  if (lane.muted) lane.soloed = false;
  markCustom();
  syncLaneButtons();
  applyMix();
}

function toggleSolo(lane) {
  lane.soloed = !lane.soloed;
  if (lane.soloed) lane.muted = false;
  markCustom();
  syncLaneButtons();
  applyMix();
}

// ---------- rendering ----------

function cssColor(token) {
  const v = getComputedStyle(document.documentElement).getPropertyValue(token).trim();
  return v || "#E8912D";
}

function peaksFor(buffer) {
  const data = buffer.getChannelData(0);
  const per = Math.max(1, Math.floor(data.length / PEAK_BUCKETS));
  const out = new Float32Array(PEAK_BUCKETS);
  for (let b = 0; b < PEAK_BUCKETS; b++) {
    let m = 0;
    const st = b * per, en = Math.min(data.length, st + per);
    for (let i = st; i < en; i += 2) { const a = Math.abs(data[i]); if (a > m) m = a; }
    out[b] = m;
  }
  let peak = 0.001;
  for (const v of out) if (v > peak) peak = v;
  for (let b = 0; b < PEAK_BUCKETS; b++) out[b] = out[b] / peak;
  return out;
}

function renderLanes() {
  lanesEl.innerHTML = "";
  for (const lane of lanes) {
    const meta = STEM_META[lane.stem] || { label: lane.stem, icon: "wave" };
    const el = document.createElement("div");
    el.className = "sm-lane";
    el.id = `sm-lane-${lane.stem}`;
    el.style.setProperty("--lane", lane.color);
    el.innerHTML = `
      <div class="sm-lane-id">
        <span class="sm-lane-icon">${icon(meta.icon, 14)}</span>
        <span class="sm-lane-name">${meta.label}</span>
      </div>
      <button class="sm-btn sm-mute" title="Mute">M</button>
      <button class="sm-btn sm-solo" title="Solo">S</button>
      <canvas class="sm-wave" title="Click to seek"></canvas>
      <input class="sm-fader" type="range" min="0" max="150" value="${Math.round(lane.gain * 100)}" title="Level (double-click resets)" />
      <span class="sm-fader-val tabular-nums">${Math.round(lane.gain * 100)}%</span>
      <div class="sm-lane-actions">
        <button class="icon-btn sm-pg" title="Add this stem to the Playground">${icon("grid", 14)}</button>
        <button class="icon-btn sm-export" title="Export this stem as WAV">${icon("export", 14)}</button>
        <span class="icon-btn sm-drag" title="Drag this stem into your DAW">${icon("grip", 14)}</span>
      </div>`;
    el.querySelector(".sm-mute").onclick = () => toggleMute(lane);
    el.querySelector(".sm-solo").onclick = () => toggleSolo(lane);
    const fader = el.querySelector(".sm-fader");
    fader.oninput = () => {
      lane.gain = Number(fader.value) / 100;
      markCustom();
      const valEl = el.querySelector(".sm-fader-val");
      if (valEl) valEl.textContent = `${fader.value}%`;
      applyMix();
    };
    fader.ondblclick = () => {
      fader.value = 100;
      lane.gain = 1;
      markCustom();
      const valEl = el.querySelector(".sm-fader-val");
      if (valEl) valEl.textContent = "100%";
      applyMix();
    };
    el.querySelector(".sm-wave").onclick = (e) => {
      const r = e.currentTarget.getBoundingClientRect();
      seek(((e.clientX - r.left) / r.width) * duration());
    };
    el.querySelector(".sm-pg").onclick = (e) => {
      e.stopPropagation();
      if (!deps.playgroundAddClip) return;
      const clip = stemLaneAsPlaygroundClip(lane);
      if (!clip) { deps.showToast("That stem has no audio file"); return; }
      deps.playgroundAddClip(clip);
    };
    el.querySelector(".sm-export").onclick = async () => {
      try {
        const path = await invoke("export_mark", { markId: lane.markId });
        deps.showToast(`Exported → ${String(path).split(/[\\/]/).pop()}`);
      } catch (err) { deps.showToast(`Export failed: ${err}`); }
    };
    el.querySelector(".sm-drag").addEventListener("mousedown", async (e) => {
      e.preventDefault(); e.stopPropagation();
      if (!deps.startPathsDrag) return;
      try {
        const path = await invoke("prepare_drag_wav", { markId: lane.markId });
        await deps.startPathsDrag([path]);
      } catch (err) { deps.showToast(`Drag failed: ${err}`); }
    });
    if (!lane.buffer) el.classList.add("missing");
    lanesEl.appendChild(el);
  }
  syncLaneButtons();
  drawAll();
}

function drawAll() {
  const d = duration();
  const pos = position();
  const frac = d > 0 ? pos / d : 0;
  for (const lane of lanes) drawLane(lane, frac);
  timeEl.textContent = `${fmtClock(pos)} / ${fmtClock(d)}`;
  seekFill.style.width = `${frac * 100}%`;
  seekKnob.style.left = `${frac * 100}%`;
}

function fmtClock(sec) {
  if (!isFinite(sec)) sec = 0;
  const m = Math.floor(sec / 60);
  const s = sec - m * 60;
  return `${m}:${s < 10 ? "0" : ""}${s.toFixed(1)}`;
}

function drawLane(lane, frac) {
  const canvas = document.querySelector(`#sm-lane-${lane.stem} .sm-wave`);
  if (!canvas || !canvas.clientWidth) return;
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth, h = canvas.clientHeight;
  if (canvas.width !== Math.floor(w * dpr) || canvas.height !== Math.floor(h * dpr)) {
    canvas.width = Math.floor(w * dpr);
    canvas.height = Math.floor(h * dpr);
  }
  const g = canvas.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  if (!lane.peaks) return;
  const silent = effectiveGain(lane) === 0;
  const n = lane.peaks.length;
  const bw = w / n;
  const inner = Math.max(1.5, bw * 0.75);
  const midY = h / 2;

  for (let i = 0; i < n; i++) {
    const amp = Math.max(0.06, lane.peaks[i]);
    const bh = Math.max(2, amp * (h - 6));
    const played = i / n <= frac;
    const x = i * bw + (bw - inner) / 2;
    const y = midY - bh / 2;

    if (silent) {
      g.fillStyle = "rgba(100, 105, 115, 0.16)";
    } else if (played) {
      const grad = g.createLinearGradient(0, y, 0, y + bh);
      grad.addColorStop(0, "#ffffff");
      grad.addColorStop(0.3, lane.color);
      grad.addColorStop(1, lane.color);
      g.fillStyle = grad;
    } else {
      g.fillStyle = hexToRgba(lane.color, 0.28);
    }
    if (g.roundRect) {
      g.beginPath();
      g.roundRect(x, y, inner, bh, Math.min(inner / 2, bh / 2));
      g.fill();
    } else {
      g.fillRect(x, y, inner, bh);
    }
  }

  // Luminous laser playhead
  if (frac > 0 && frac <= 1) {
    const px = frac * w;
    g.fillStyle = "rgba(255, 255, 255, 0.25)";
    g.fillRect(px - 2, 0, 4, h);
    g.fillStyle = "#ffffff";
    g.fillRect(px - 0.75, 0, 1.5, h);
  }
}

function hexToRgba(hexOrCss, alpha) {
  if (!hexOrCss) return `rgba(255, 149, 0, ${alpha})`;
  if (hexOrCss.startsWith("var(")) return hexOrCss;
  if (hexOrCss.startsWith("#")) {
    const r = parseInt(hexOrCss.slice(1, 3), 16) || 255;
    const g = parseInt(hexOrCss.slice(3, 5), 16) || 149;
    const b = parseInt(hexOrCss.slice(5, 7), 16) || 0;
    return `rgba(${r}, ${g}, ${b}, ${alpha})`;
  }
  return hexOrCss;
}


// ---------- bounce / delete ----------

async function bounce(saveAsNote) {
  if (!set) return;
  const gains = gainsForBounce();
  if (!Object.values(gains).some((g) => g > 0)) { deps.showToast("Every stem is muted — nothing to bounce"); return; }
  try {
    const res = await invoke("export_stem_mix", { sourceMarkId: set.source_mark_id, gains, saveAsNote });
    const name = String(res.path).split(/[\\/]/).pop();
    deps.showToast(saveAsNote ? `Mix saved as a new note ✓` : `Exported → ${name}`);
    if (saveAsNote) deps.onSetChanged(res.mark_id);
  } catch (err) { deps.showToast(`Bounce failed: ${err}`); }
}

async function deleteStems() {
  if (!set) return;
  if (!confirm("Delete these four stems? The original note and any saved mixes are kept.")) return;
  const id = set.source_mark_id;
  try {
    await invoke("delete_stem_set", { sourceMarkId: id });
    closeStemMixer();
    deps.showToast("Stems deleted");
    deps.onSetChanged(null);
  } catch (err) { deps.showToast(`Delete failed: ${err}`); }
}
