// playground.js — Playground looping-jam tab. All audio is Web Audio; clip WAVs
// are read-only. Lanes loop together, beat-synced when detectBpm is confident.
import { invoke, convertFileSrc } from "./bridge.js";
import { detectBpm, snapLoopSeconds } from "./bpm.js";
import { icon } from "./icons.js";

import { ALL_BARS, BAR_COUNT, barSeconds, scheduleLane, schedulePattern, encodeWav } from "./arrangement.js";

const fileInput = document.getElementById("pg-files");
const nameInput = document.getElementById("pg-name");
const saveButton = document.getElementById("pg-save");
const exportButton = document.getElementById("pg-export");
const status = document.getElementById("pg-status");
let lastMixId = null;
let saving = false;
let onPlay = () => {};

const LS_KEY = "samplify.playground.v1";
const MAX_LANES = 8;
const CONFIDENT = 0.6;

const lanesEl = document.getElementById("playground-lanes");
const emptyEl = document.getElementById("playground-empty");
const playBtn = document.getElementById("pg-play");
const bpmSlider = document.getElementById("pg-bpm");
const bpmVal = document.getElementById("pg-bpm-val");
const clearBtn = document.getElementById("pg-clear");
const badgeEl = document.getElementById("playground-badge");

let ctx = null;
let masterGain = null;
let lanes = [];
let masterBpm = 120;
let masterBpmSet = false; // true once a synced lane or the user has chosen a tempo
let playing = false;
let startAt = 0; // shared AudioContext start anchor so lanes stay phase-locked
let rafId = null;
let toastFn = (m) => console.log("[playground]", m);

export function initPlayground({ showToast, onPlaybackStart } = {}) {
  if (showToast) toastFn = showToast;
  if (onPlaybackStart) onPlay = onPlaybackStart;
  document.getElementById("pg-import").onclick = () => fileInput.click();
  fileInput.onchange = async () => {
    const files = [...fileInput.files];
    fileInput.value = "";
    for (const file of files) {
      if (lanes.length >= MAX_LANES) { toastFn("All eight lanes are in use"); break; }
      try {
        ensureCtx();
        const buffer = await ctx.decodeAudioData(await file.arrayBuffer());
        const title = file.name.replace(/\.[^.]+$/, "").slice(0, 200) || "Imported audio";
        const markId = await saveTake(title, "import", buffer);
        const [row] = await invoke("get_marks_by_ids", { ids: [markId] });
        await playgroundAddClip({markId, title, artist: row.artist, noteText: title, filePath: row.file_path}, {buffer});
      } catch (err) { toastFn("Import failed: " + (err.message || err)); }
    }
  };
  nameInput.oninput = persist;
  saveButton.onclick = saveMix;
  exportButton.onclick = async () => {
    try {
      const path = await invoke("export_mark", { markId: lastMixId });
      if (path) status.textContent = "Exported to " + path;
    } catch (err) { toastFn("Export failed: " + err); }
  };
  playBtn.onclick = () => (playing ? stopAll() : playAll());
  bpmSlider.oninput = () => {
    masterBpm = Number(bpmSlider.value);
    masterBpmSet = true;
    bpmVal.textContent = masterBpm;
    if (playing) playAll();
    persist();
  };
  clearBtn.onclick = () => {
    stopAll();
    lanes = [];
    nameInput.value = "";
    lastMixId = null;
    exportButton.hidden = true;
    status.textContent = "";
    renderLanes();
    persist();
  };
  restore();
}

export function renderPlayground() {
  renderLanes();
}

function ensureCtx() {
  if (!ctx) {
    ctx = new (window.AudioContext || window.webkitAudioContext)();
    masterGain = ctx.createGain();
    masterGain.connect(ctx.destination);
  }
  if (ctx.state === "suspended") ctx.resume();
}

async function saveTake(title, kind, buffer) {
  const encodedTitle = btoa(String.fromCharCode(...new TextEncoder().encode(title)));
  return invoke("save_playground_take", encodeWav(buffer), {
    headers: { "x-samplify-kind": kind, "x-samplify-title": encodedTitle },
  });
}

async function saveMix() {
  if (saving || !lanes.length) return;
  saving = true;
  saveButton.disabled = true;
  status.textContent = "Rendering four bars…";
  try {
    const title = nameInput.value.trim() || "Untitled mix";
    const sampleRate = 48000;
    const offline = new OfflineAudioContext(2, Math.round(BAR_COUNT * barSeconds(masterBpm) * sampleRate), sampleRate);
    for (const lane of lanes) scheduleLane(offline, offline.destination, lane, laneLoop(lane), effectiveGain(lane), masterBpm, 0);
    const rendered = await offline.startRendering();
    lastMixId = await saveTake(title, "mix", rendered);
    exportButton.hidden = false;
    status.textContent = title + " saved to Library";
  } catch (err) { status.textContent = "Save failed: " + (err.message || err); }
  finally { saving = false; saveButton.disabled = !lanes.length; }
}

export async function playgroundAddClip(clip, { fromRestore = false, savedLane = null, buffer = null } = {}) {
  if (lanes.some((l) => l.clip.markId === clip.markId)) {
    if (!fromRestore) toastFn("Already in the Playground");
    return;
  }
  ensureCtx();
  try {
    if (!buffer) {
      const res = await fetch(convertFileSrc(clip.filePath));
      if (!res.ok) throw new Error("HTTP " + res.status);
      buffer = await ctx.decodeAudioData(await res.arrayBuffer());
    }
  } catch (err) {
    if (!fromRestore) toastFn("Could not load " + (clip.noteText || clip.title));
    return;
  }
  if (lanes.some(l => l.clip.markId === clip.markId)) return;
  const parent = clip.stem && lanes.find(l => l.clip.markId === clip.sourceMarkId);
  if (!parent && lanes.length >= MAX_LANES) {
    if (!fromRestore) toastFn("All eight lanes are in use");
    return;
  }
  if (parent) {
    disconnectLane(parent);
    lanes.splice(lanes.indexOf(parent), 1);
  }
  const region = loopRegion(clip, buffer);
  const det = clip.bpm > 0 && clip.bpm <= 400 ? {bpm: clip.bpm, confidence: 1} : detectBpm(mixToMono(buffer, region), buffer.sampleRate);
  const synced = det.bpm != null && det.confidence >= CONFIDENT;
  const lane = {
    clip,
    buffer,
    loopStart: region.start,
    loopEndRaw: region.end,
    laneBpm: synced ? det.bpm : null,
    peaks: null,
    gain: savedLane && savedLane.gain != null ? savedLane.gain : 0.9,
    muted: savedLane ? !!savedLane.muted : false,
    soloed: !!savedLane?.soloed,
    pan: savedLane?.pan ?? 0,
    bars: savedLane?.bars ?? ALL_BARS,
    nudge: savedLane && savedLane.nudge != null ? savedLane.nudge : 1,
    offset: savedLane && savedLane.offset != null ? savedLane.offset : 0,
    source: null,
    gainNode: null,
  };
  const eff = laneLoop(lane);
  lane.peaks = peaksFor(buffer, region.start, region.start + eff.len, 240);
  lanes.push(lane);
  if (synced && !masterBpmSet) {
    masterBpm = Math.max(Number(bpmSlider.min), Math.min(Number(bpmSlider.max), det.bpm));
    masterBpmSet = true;
    bpmSlider.value = masterBpm;
    bpmVal.textContent = masterBpm;
  }

  if (!fromRestore && synced && clip.bpm == null) writeBackBpm(clip, det.bpm);
  renderLanes();
  if (!fromRestore) persist();
  if (!fromRestore) toastFn(`Added to Playground — ${clip.stem ? clip.stem : (clip.noteText || clip.title)}`);
  if (playing) playAll();
}

// The lane's loop region inside its clip file: the mark's trim, else the whole clip.
function loopRegion(clip, buffer) {
  let start = 0;
  let end = buffer.duration;
  if (clip.trimStartMs != null && clip.trimEndMs != null && clip.trimEndMs > clip.trimStartMs) {
    start = Math.min(clip.trimStartMs / 1000, buffer.duration);
    end = Math.min(clip.trimEndMs / 1000, buffer.duration);
  }
  if (end - start < 0.05) {
    start = 0;
    end = buffer.duration;
  }
  return { start, end };
}

function mixToMono(buffer, region) {
  const sr = buffer.sampleRate;
  const s = Math.floor(region.start * sr);
  const e = Math.floor(region.end * sr);
  const out = new Float32Array(Math.max(0, e - s));
  for (let ch = 0; ch < buffer.numberOfChannels; ch++) {
    const data = buffer.getChannelData(ch);
    for (let i = s; i < e; i++) out[i - s] += data[i] / buffer.numberOfChannels;
  }
  return out;
}

function peaksFor(buffer, startSec, endSec, buckets) {
  const data = buffer.getChannelData(0);
  const sr = buffer.sampleRate;
  const s = Math.floor(startSec * sr);
  const e = Math.min(data.length, Math.floor(endSec * sr));
  const per = Math.max(1, Math.floor((e - s) / buckets));
  const out = [];
  for (let b = 0; b < buckets; b++) {
    let m = 0;
    const st = s + b * per;
    const en = Math.min(e, st + per);
    for (let i = st; i < en; i++) {
      const a = Math.abs(data[i]);
      if (a > m) m = a;
    }
    out.push(m);
  }
  const peak = Math.max(...out, 0.001);
  return out.map((v) => v / peak);
}

// Effective loop + rate: synced lanes snap to whole beats and follow master BPM.
function laneLoop(lane) {
  const rawLen = lane.loopEndRaw - lane.loopStart;
  if (lane.laneBpm) {
    const maxLen = rawLen;
    const snapped = snapLoopSeconds(rawLen, lane.laneBpm, maxLen);
    if (snapped) {
      return { start: lane.loopStart, len: snapped, rate: (masterBpm / lane.laneBpm) * lane.nudge, synced: true };
    }
  }
  return { start: lane.loopStart, len: rawLen, rate: lane.nudge, synced: false };
}

function effectiveGain(lane) {
  const anySolo = lanes.some((l) => l.soloed);
  return lane.muted || (anySolo && !lane.soloed) ? 0 : lane.gain;
}

function refreshGains() {
  for (const lane of lanes) {
    if (lane.gainNode) lane.gainNode.gain.value = effectiveGain(lane);
  }
}

function playAll() {
  if (!lanes.length) {
    toastFn("Add clips from the Library first");
    return;
  }
  ensureCtx();
  stopSources();
  startAt = ctx.currentTime + 0.06;
  for (const lane of lanes) {
    Object.assign(lane, scheduleLane(ctx, masterGain, lane, laneLoop(lane), effectiveGain(lane), masterBpm, startAt));
  }
  onPlay();
  playing = true;
  playBtn.innerHTML = icon("stop", 16);
  playBtn.classList.add("active");
  playBtn.setAttribute("aria-label", "Stop arrangement");
  if (rafId) cancelAnimationFrame(rafId);
  animate();
}

function disconnectLane(lane) {
  for (const key of ["source", "pattern", "gainNode", "panNode", "gate"]) {
    const node = lane[key];
    if (!node) continue;
    if (key === "source" || key === "pattern") node.stop();
    node.disconnect();
    lane[key] = null;
  }
}

function stopSources() {
  lanes.forEach(disconnectLane);
}

export function stopAll() {
  stopSources();
  playing = false;
  if (rafId) cancelAnimationFrame(rafId);
  rafId = null;
  playBtn.innerHTML = icon("play", 16);
  playBtn.classList.remove("active");
  playBtn.setAttribute("aria-label", "Play arrangement");
  renderStaticWaves();
}

function animate() {
  if (!playing) return;
  for (const lane of lanes) drawLane(lane);
  rafId = requestAnimationFrame(animate);
}

function wrap(x, len) {
  if (!(len > 0)) return 0;
  x = x % len;
  return x < 0 ? x + len : x;
}

function beatSec(lane) {
  return lane.laneBpm ? 60 / lane.laneBpm : 0;
}

function snapOffset(off, lane, l) {
  const b = beatSec(lane);
  if (!b) return wrap(off, l.len);
  const grid = b / 4;
  return wrap(Math.round(off / grid) * grid, l.len);
}

function formatOffset(lane) {
  const l = laneLoop(lane);
  const off = wrap(lane.offset || 0, l.len);
  if (off < 0.008 || l.len - off < 0.008) return "0";
  if (lane.laneBpm) {
    const q = Math.round((off / beatSec(lane)) * 4) / 4;
    if (Math.abs(q) < 0.01) return "0";
    return (q > 0 ? "+" : "") + q + " beat" + (Math.abs(q) === 1 ? "" : "s");
  }
  return "+" + off.toFixed(2) + "s";
}

function waveFill(silent) {
  if (silent) return "color-mix(in srgb, var(--muted) 40%, transparent)";
  const a = getComputedStyle(document.documentElement).getPropertyValue("--accent").trim();
  return a;
}

function drawLane(lane) {
  const canvas = document.getElementById(`pg-wave-${lane.clip.markId}`);
  if (!canvas || !canvas.clientWidth) return;
  const dpr = window.devicePixelRatio || 1;
  const w = canvas.clientWidth;
  const h = canvas.clientHeight;
  if (canvas.width !== Math.floor(w * dpr)) {
    canvas.width = Math.floor(w * dpr);
    canvas.height = Math.floor(h * dpr);
  }
  const g = canvas.getContext("2d");
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  g.clearRect(0, 0, w, h);
  const silent = effectiveGain(lane) === 0;
  g.fillStyle = silent ? getComputedStyle(document.documentElement).getPropertyValue("--muted-dark").trim() : waveFill(false);
  const n = lane.peaks.length;
  const l = laneLoop(lane);
  const shift = Math.floor((wrap(lane.offset || 0, l.len) / l.len) * n);
  const bw = w / n;
  for (let i = 0; i < n; i++) {
    const ph = Math.max(1, lane.peaks[(i + shift) % n] * (h - 4));
    g.fillRect(i * bw, (h - ph) / 2, Math.max(1, bw - 1), ph);
  }
  if (playing && ctx && lane.source) {
    const elapsed = Math.max(0, ctx.currentTime - startAt) * l.rate;
    const frac = (elapsed % l.len) / l.len;
    g.fillStyle = getComputedStyle(document.documentElement).getPropertyValue("--text").trim();
    g.fillRect(frac * w, 0, 1.5, h);
  }
}

function renderStaticWaves() {
  for (const lane of lanes) drawLane(lane);
}

function nudgeLane(lane, delta) {
  lane.nudge = Math.min(1.2, Math.max(0.8, Math.round((lane.nudge + delta) * 100) / 100));
  persist();
  renderLanes();
  if (playing) playAll();
}

function shiftLane(lane, beatsOrSec) {
  const l = laneLoop(lane);
  const step = lane.laneBpm ? beatSec(lane) * beatsOrSec : beatsOrSec;
  lane.offset = wrap((lane.offset || 0) + step, l.len);
  persist();
  renderLanes();
  if (playing) playAll();
}

function bindWaveDrag(canvas, lane) {
  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const rect = canvas.getBoundingClientRect();
    const l0 = laneLoop(lane);
    const startX = e.clientX;
    const startOff = lane.offset || 0;
    const snap = l0.synced && !e.altKey;
    canvas.classList.add("dragging");
    const onMove = (ev) => {
      const l = laneLoop(lane);
      let next = startOff - ((ev.clientX - startX) / rect.width) * l.len;
      next = wrap(next, l.len);
      if (snap) next = snapOffset(next, lane, l);
      lane.offset = next;
      drawLane(lane);
      const read = canvas.closest(".pg-lane")?.querySelector(".pg-offset-val");
      if (read) read.textContent = formatOffset(lane);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      canvas.classList.remove("dragging");
      persist();
      if (playing) playAll();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  });
  canvas.addEventListener("dblclick", () => {
    lane.offset = 0;
    persist();
    renderLanes();
    if (playing) playAll();
  });
}

function bindReorder(grip, laneEl) {
  grip.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    laneEl.classList.add("dragging");
    const onMove = (ev) => {
      const others = [...lanesEl.querySelectorAll(".pg-lane")].filter((n) => n !== laneEl);
      let placed = false;
      for (const o of others) {
        const r = o.getBoundingClientRect();
        if (ev.clientY < r.top + r.height / 2) {
          lanesEl.insertBefore(laneEl, o);
          placed = true;
          break;
        }
      }
      if (!placed) lanesEl.appendChild(laneEl);
    };
    const onUp = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      laneEl.classList.remove("dragging");
      const ids = [...lanesEl.querySelectorAll(".pg-lane")].map((n) => Number(n.dataset.markId));
      lanes.sort((a, b) => ids.indexOf(a.clip.markId) - ids.indexOf(b.clip.markId));
      persist();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
  });
}

function renderLanes() {
  badgeEl.textContent = lanes.length;
  badgeEl.style.display = lanes.length ? "" : "none";
  emptyEl.style.display = lanes.length ? "none" : "block";
  lanesEl.innerHTML = "";
  saveButton.disabled = saving || !lanes.length;
  playBtn.disabled = !lanes.length;
  clearBtn.disabled = !lanes.length;
  lanes.forEach((lane) => {
    const c = lane.clip;
    const l = laneLoop(lane);
    const nudgePct = Math.round((lane.nudge - 1) * 100);
    const el = document.createElement("div");
    el.className = "pg-lane";
    el.dataset.markId = String(c.markId);
    el.innerHTML = `
      <div class="pg-lane-head">
        <button class="pg-grip" title="Drag to reorder">${icon("grip", 14)}</button>
        <div class="pg-lane-title">
          <span class="pg-title-text"></span>
          <span class="pg-artist-text"></span>
        </div>
        <span class="pg-bpm-badge ${l.synced ? "synced" : ""}" title="${l.synced ? "Beat-synced to the master tempo" : "No confident beat found — plays at natural speed"}">${l.synced ? lane.laneBpm + " BPM" : "—"}</span>
        <button class="pg-remove" title="Remove lane">${icon("close", 14)}</button>
      </div>
      <div class="pg-wave-wrap" title="Drag to shift timing · double-click resets">
        <canvas class="pg-wave" id="pg-wave-${c.markId}"></canvas>
        <div class="pg-downbeat"></div>
      </div>
      <div class="pg-lane-controls">
        <button class="pg-btn pg-mute ${lane.muted ? "on" : ""}" title="Mute" aria-pressed="${lane.muted}">M</button>
        <button class="pg-btn pg-solo ${lane.soloed ? "on" : ""}" title="Solo" aria-pressed="${lane.soloed}">S</button>
        <input class="pg-vol" type="range" min="0" max="100" value="${Math.round(lane.gain * 100)}" title="Lane volume" />
        <div class="pg-shift" title="Shift start against the shared downbeat">
          <button class="pg-btn pg-shift-back" title="Earlier">${icon("back", 12)}</button>
          <span class="pg-offset-val">${formatOffset(lane)}</span>
          <button class="pg-btn pg-shift-fwd flip" title="Later">${icon("back", 12)}</button>
        </div>
        <div class="pg-nudge" title="Fine speed nudge (pitch follows)">
          <button class="pg-btn pg-nudge-down">−</button>
          <span class="pg-nudge-val">${nudgePct === 0 ? "±0%" : (nudgePct > 0 ? "+" : "") + nudgePct + "%"}</span>
          <button class="pg-btn pg-nudge-up">+</button>
        </div>
        <label class="pg-pan-label">Pan <input class="pg-pan" type="range" min="-100" max="100" value="${Math.round(lane.pan * 100)}" /></label>
        <div class="pg-bars" role="group" aria-label="Active bars"><span>Bars</span></div>
      </div>`;
    el.querySelector(".pg-title-text").textContent = laneTitle(c);
    el.querySelector(".pg-artist-text").textContent = c.artist || "";
    const bars = el.querySelector(".pg-bars");
    for (let i = 0; i < BAR_COUNT; i++) {
      const button = document.createElement("button");
      button.className = "pg-btn";
      button.textContent = i + 1;
      button.setAttribute("aria-label", "Bar " + (i + 1));
      button.setAttribute("aria-pressed", Boolean(lane.bars & (1 << i)));
      button.onclick = () => {
        lane.bars ^= 1 << i;
        button.setAttribute("aria-pressed", Boolean(lane.bars & (1 << i)));
        if (playing) {
          const when = ctx.currentTime + 0.01;
          const old = lane.pattern;
          old.stop(when);
          old.onended = () => old.disconnect();
          lane.pattern = schedulePattern(ctx, lane.gate, lane.bars, masterBpm, when, when - startAt);
        }
        persist();
      };
      bars.appendChild(button);
    }
    el.querySelector(".pg-pan").oninput = (e) => {
      lane.pan = Number(e.target.value) / 100;
      if (lane.panNode) lane.panNode.pan.value = lane.pan;
      persist();
    };
    el.querySelector(".pg-remove").onclick = () => {
      const wasPlaying = playing;
      stopAll();
      lanes.splice(lanes.indexOf(lane), 1);
      renderLanes();
      persist();
      if (wasPlaying && lanes.length) playAll();
    };
    el.querySelector(".pg-mute").onclick = (e) => {
      lane.muted = !lane.muted;
      e.target.classList.toggle("on", lane.muted);
      e.target.setAttribute("aria-pressed", lane.muted);
      refreshGains();
      drawLane(lane);
      persist();
    };
    el.querySelector(".pg-solo").onclick = (e) => {
      lane.soloed = !lane.soloed;
      e.target.classList.toggle("on", lane.soloed);
      e.target.setAttribute("aria-pressed", lane.soloed);
      refreshGains();
      renderStaticWaves();
      persist();
    };
    el.querySelector(".pg-vol").oninput = (e) => {
      lane.gain = Number(e.target.value) / 100;
      refreshGains();
      persist();
    };
    el.querySelector(".pg-nudge-down").onclick = () => nudgeLane(lane, -0.02);
    el.querySelector(".pg-nudge-up").onclick = () => nudgeLane(lane, +0.02);
    el.querySelector(".pg-shift-back").onclick = () => shiftLane(lane, lane.laneBpm ? -1 : -0.1);
    el.querySelector(".pg-shift-fwd").onclick = () => shiftLane(lane, lane.laneBpm ? 1 : 0.1);
    bindWaveDrag(el.querySelector(".pg-wave"), lane);
    bindReorder(el.querySelector(".pg-grip"), el);
    lanesEl.appendChild(el);
  });
  renderStaticWaves();
}

async function writeBackBpm(clip, bpm) {
  try {
    await invoke("update_mark_meta", {
      markId: clip.markId,
      noteText: clip.noteText || "",
      tags: clip.tags || [],
      rating: clip.rating != null ? clip.rating : null,
      key: clip.key != null ? clip.key : null,
      bpm,
    });
    clip.bpm = bpm;
    console.log(`playground: wrote detected BPM ${bpm} back to mark ${clip.markId}`);
  } catch (e) {
    console.error("playground: bpm write-back failed", e);
  }
}

function laneTitle(c) {
  if (c.stem) return `${c.title} — ${c.stem}`;
  const m = c.noteText && String(c.noteText).match(/— (drums|bass|other|vocals)$/i);
  if (m) return `${c.title} — ${m[1].toLowerCase()}`;
  return c.noteText || c.title;
}

function persist() {
  const state = {
    masterBpm,
    masterBpmSet,
    name: nameInput.value,
    lanes: lanes.map((l) => ({
      markId: l.clip.markId,
      filePath: l.clip.filePath,
      title: l.clip.title,
      noteText: l.clip.noteText,
      stem: l.clip.stem || null,
      gain: l.gain,
      muted: l.muted,
      soloed: l.soloed,
      pan: l.pan,
      bars: l.bars,
      nudge: l.nudge,
      offset: l.offset || 0,
    })),
  };
  try { localStorage.setItem(LS_KEY, JSON.stringify(state)); } catch (e) { /* storage full/blocked */ }
}

async function restore() {
  let saved = null;
  try { saved = JSON.parse(localStorage.getItem(LS_KEY) || "null"); } catch (e) { saved = null; }
  if (!saved || !Array.isArray(saved.lanes) || !saved.lanes.length) {
    renderLanes();
    return;
  }
  nameInput.value = saved.name || "";
  masterBpm = Number(saved.masterBpm) || 120;
  masterBpmSet = !!saved.masterBpmSet;
  bpmSlider.value = masterBpm;
  bpmVal.textContent = masterBpm;
  let clips = [];
  try {
    clips = await invoke("get_marks_by_ids", { ids: saved.lanes.map((l) => l.markId) });
  } catch (e) {
    console.error("playground: restore failed", e);
    renderLanes();
    return;
  }
  for (const savedLane of saved.lanes) {
    const dbClip = clips.find(c => c.mark_id === savedLane.markId);
    if (!dbClip) continue;
    await playgroundAddClip(
      {
        markId: dbClip.mark_id,
        title: savedLane.title || dbClip.title,
        artist: dbClip.artist,
        noteText: savedLane.noteText || dbClip.note_text,
        filePath: savedLane.filePath || dbClip.file_path,
        trimStartMs: dbClip.trim_start_ms,
        trimEndMs: dbClip.trim_end_ms,
        tags: dbClip.tags,
        rating: dbClip.rating,
        key: dbClip.key,
        bpm: dbClip.bpm,
        stem: savedLane.stem || null,
      },
      { fromRestore: true, savedLane }
    );
  }
  persist(); // prunes saved lanes whose marks were deleted
}
