// visualizer.js — Built-in real-time audio visualizer for the bottom-left rail widget.
// Renders live frequency & waveform data when audio is playing, and an atmospheric
// 3D wireframe soundscape when idle based on the loaded sample's peaks.

let canvas = null;
let ctx = null;
let audioCtx = null;
let analyser = null;
let modeIndex = 0;
const MODES = [
  { id: "landscape", label: "3D Terrain", desc: "Volumetric Wireframe" },
  { id: "spectrum", label: "Spectrum", desc: "Real-time 3D EQ" },
  { id: "liquid", label: "Liquid Wave", desc: "Reactive Oscilloscope" },
];

let freqData = null;
let timeData = null;
let samplePeaks = [];
let animId = null;
let isAudioActive = false;
let idlePhase = 0;
let peakHold = new Array(32).fill(0);
let modeLabelEl = null;
let statusLabelEl = null;

export function initVisualizer(canvasEl, modeEl, statusEl) {
  canvas = canvasEl;
  if (!canvas) return;
  ctx = canvas.getContext("2d");
  modeLabelEl = modeEl;
  statusLabelEl = statusEl;
  updateLabels();

  resize();
  window.addEventListener("resize", resize);
  document.addEventListener("visibilitychange", () => {
    if (!document.hidden) { resize(); render(); }
  });

  const widget = canvas.closest(".visualizer-card") || canvas;
  widget.addEventListener("click", () => {
    cycleMode();
  });

  render();
  startLoop();
}

function updateLabels() {
  const m = MODES[modeIndex];
  if (modeLabelEl) modeLabelEl.textContent = m.label;
  if (statusLabelEl) {
    statusLabelEl.textContent = isAudioActive
      ? (analyser && audioCtx && audioCtx.state === "running" ? "Live mix" : "Playing")
      : m.desc;
  }
}

export function cycleMode() {
  modeIndex = (modeIndex + 1) % MODES.length;
  updateLabels();
}

let playheadFrac = 0;
let playheadEnergy = 0;

export function setSamplePeaks(peaks) {
  if (Array.isArray(peaks) && peaks.length > 0) {
    samplePeaks = peaks;
    render();
  }
}

export function setAudioActive(active) {
  isAudioActive = !!active;
  if (!active) {
    playheadEnergy = 0;
  }
  updateLabels();
}

/** Update playhead position during HTML audio playback for peak-reactive visualization */
export function updateVisualizerPlayhead(currentTime, duration) {
  if (duration > 0 && currentTime >= 0) {
    playheadFrac = Math.min(1, Math.max(0, currentTime / duration));
    if (samplePeaks.length > 0) {
      const idx = Math.floor(playheadFrac * samplePeaks.length);
      const pk = samplePeaks[idx] || 0;
      playheadEnergy = playheadEnergy * 0.7 + pk * 0.3;
    }
  }
}

/** Connect a Web Audio node (e.g. Stem Mixer masterGain) cleanly */
export function connectAudioSource(source, sharedCtx = null) {
  try {
    if (sharedCtx) {
      audioCtx = sharedCtx;
      if (audioCtx.state === "suspended") {
        audioCtx.resume().catch(() => {});
      }
      analyser = audioCtx.createAnalyser();
      analyser.fftSize = 128;
      analyser.smoothingTimeConstant = 0.82;
      freqData = new Uint8Array(analyser.frequencyBinCount);
      timeData = new Uint8Array(analyser.fftSize);
      if (source && typeof source.connect === "function") {
        source.connect(analyser);
      }
    }
  } catch (err) {
    console.warn("Visualizer Web Audio connect notice:", err);
  }
}

export function disconnectAudioSource() {
  try {
    if (analyser) {
      analyser.disconnect();
      analyser = null;
    }
  } catch (err) {
    console.warn("Visualizer disconnect notice:", err);
  }
}

function resize() {
  if (!canvas || !ctx) return;
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  const w = rect.width || 180;
  const h = rect.height || 96;

  canvas.width = Math.floor(w * dpr);
  canvas.height = Math.floor(h * dpr);
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
}

function startLoop() {
  if (animId) {
    cancelAnimationFrame(animId);
    clearTimeout(animId);
  }
  function frame() {
    render();
    animId = document.hidden ? setTimeout(frame, 80) : requestAnimationFrame(frame);
  }
  frame();
}

function render() {
  if (!canvas || !ctx) return;
  const rect = canvas.getBoundingClientRect();
  const w = rect.width;
  const h = rect.height;
  if (!w || !h) return;

  ctx.clearRect(0, 0, w, h);

  // Background subtle dark depth
  const bgGrad = ctx.createLinearGradient(0, 0, 0, h);
  bgGrad.addColorStop(0, "rgba(10, 13, 19, 0.95)");
  bgGrad.addColorStop(1, "rgba(5, 7, 10, 0.98)");
  ctx.fillStyle = bgGrad;
  ctx.fillRect(0, 0, w, h);

  // Get live audio data if available
  let hasRealSignal = false;
  if (analyser && isAudioActive && audioCtx && audioCtx.state === "running") {
    analyser.getByteFrequencyData(freqData);
    analyser.getByteTimeDomainData(timeData);
    for (let i = 0; i < freqData.length; i++) {
      if (freqData[i] > 10) { hasRealSignal = true; break; }
    }
  }

  idlePhase += hasRealSignal || (isAudioActive && playheadEnergy > 0.02) ? 0.04 : 0.015;

  const currentMode = MODES[modeIndex].id;
  if (currentMode === "landscape") {
    render3DLandscape(w, h, hasRealSignal);
  } else if (currentMode === "spectrum") {
    render3DSpectrum(w, h, hasRealSignal);
  } else {
    renderLiquidWave(w, h, hasRealSignal);
  }
}

/** 1. 3D Wireframe Landscape / Sound Mountain */
function render3DLandscape(w, h, hasSignal) {
  const rows = 7;
  const cols = 18;
  const colW = w / (cols - 1);
  const horizon = h * 0.32;
  const depthH = h * 0.60;

  ctx.lineWidth = 1;

  for (let r = 0; r < rows; r++) {
    const rowFrac = r / (rows - 1);
    const rowY = horizon + rowFrac * depthH;
    const rowScale = 0.5 + rowFrac * 0.5;
    const alpha = 0.15 + rowFrac * 0.75;

    ctx.beginPath();
    for (let c = 0; c < cols; c++) {
      const x = c * colW;
      let heightVal = 0;

      if (hasSignal && freqData) {
        const binIdx = Math.floor((c / cols) * (freqData.length * 0.65));
        heightVal = (freqData[binIdx] / 255) * 28 * rowScale;
      } else if (samplePeaks.length > 0) {
        const frac = isAudioActive
          ? Math.min(1, Math.max(0, playheadFrac + (c / cols - 0.5) * 0.3))
          : (c / cols);
        const pIdx = Math.floor(frac * samplePeaks.length);
        const pk = samplePeaks[Math.min(samplePeaks.length - 1, Math.max(0, pIdx))] || 0.2;
        const wave = Math.sin(c * 0.6 + idlePhase + r * 0.8) * 0.5 + 0.5;
        const live = isAudioActive ? (0.35 + 0.65 * Math.max(playheadEnergy, pk)) : (0.6 + 0.4 * wave);
        heightVal = pk * 22 * rowScale * live;
      } else {
        const wave = Math.sin(c * 0.5 + idlePhase + r * 0.9) * Math.cos(r * 0.4);
        heightVal = (Math.abs(wave)) * 16 * rowScale;
      }

      const y = rowY - heightVal;
      if (c === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }

    // Gradient stroke: front rows bright orange, back rows deep amber
    const strokeGrad = ctx.createLinearGradient(0, rowY - 20, 0, rowY);
    strokeGrad.addColorStop(0, `rgba(255, 180, 70, ${alpha})`);
    strokeGrad.addColorStop(1, `rgba(255, 149, 0, ${alpha * 0.4})`);
    ctx.strokeStyle = strokeGrad;
    ctx.stroke();

    // Floor fill under front-most row
    if (r === rows - 1) {
      ctx.lineTo(w, h);
      ctx.lineTo(0, h);
      ctx.closePath();
      const fillGrad = ctx.createLinearGradient(0, rowY, 0, h);
      fillGrad.addColorStop(0, "rgba(255, 149, 0, 0.08)");
      fillGrad.addColorStop(1, "rgba(0, 0, 0, 0)");
      ctx.fillStyle = fillGrad;
      ctx.fill();
    }
  }

  // Floating summit beacon
  const sunX = w * 0.5;
  const sunY = horizon - 6;
  const sunGrad = ctx.createRadialGradient(sunX, sunY, 0, sunX, sunY, 18);
  sunGrad.addColorStop(0, "rgba(255, 200, 100, 0.5)");
  sunGrad.addColorStop(1, "rgba(255, 149, 0, 0)");
  ctx.fillStyle = sunGrad;
  ctx.beginPath();
  ctx.arc(sunX, sunY, 18, 0, Math.PI * 2);
  ctx.fill();
}

/** 2. 3D Volumetric Spectrum EQ Bars */
function render3DSpectrum(w, h, hasSignal) {
  const bars = 20;
  const barW = (w - (bars - 1) * 3) / bars;
  const baseLine = h * 0.82;
  const maxH = h * 0.68;

  for (let i = 0; i < bars; i++) {
    let norm = 0;
    if (hasSignal && freqData) {
      const idx = Math.floor((i / bars) * (freqData.length * 0.7));
      norm = freqData[idx] / 255;
    } else if (samplePeaks.length > 0) {
      const frac = isAudioActive
        ? Math.min(1, Math.max(0, playheadFrac + (i / bars - 0.5) * 0.35))
        : (i / bars);
      const pIdx = Math.floor(frac * samplePeaks.length);
      const pk = samplePeaks[Math.min(samplePeaks.length - 1, Math.max(0, pIdx))] || 0.2;
      const wobble = 0.4 + 0.6 * Math.abs(Math.sin(idlePhase * 2 + i * 0.4));
      norm = isAudioActive ? Math.min(1, pk * 0.55 + playheadEnergy * 0.7) : pk * wobble;
    } else {
      norm = 0.15 + 0.35 * Math.abs(Math.sin(idlePhase * 1.5 + i * 0.5));
    }

    const bh = Math.max(3, norm * maxH);
    const bx = i * (barW + 3);
    const by = baseLine - bh;

    // Peak hold decay
    if (norm > peakHold[i]) peakHold[i] = norm;
    else peakHold[i] = Math.max(0, peakHold[i] - 0.015);

    // Pillar vertical gradient
    const grad = ctx.createLinearGradient(0, by, 0, baseLine);
    grad.addColorStop(0, "#fffae6");
    grad.addColorStop(0.2, "#ffb833");
    grad.addColorStop(0.7, "#ff9500");
    grad.addColorStop(1, "#803800");

    ctx.fillStyle = grad;
    if (ctx.roundRect) {
      ctx.beginPath();
      ctx.roundRect(bx, by, barW, bh, [2, 2, 0, 0]);
      ctx.fill();
    } else {
      ctx.fillRect(bx, by, barW, bh);
    }

    // Floor reflection
    const refH = bh * 0.35;
    const refGrad = ctx.createLinearGradient(0, baseLine, 0, baseLine + refH);
    refGrad.addColorStop(0, "rgba(255, 149, 0, 0.25)");
    refGrad.addColorStop(1, "rgba(255, 149, 0, 0)");
    ctx.fillStyle = refGrad;
    ctx.fillRect(bx, baseLine + 1, barW, refH);

    // Peak hold cap
    const capY = baseLine - peakHold[i] * maxH - 2;
    ctx.fillStyle = "#ffffff";
    ctx.fillRect(bx, capY, barW, 1.5);
  }
}

/** 3. Liquid Oscilloscope Wave */
function renderLiquidWave(w, h, hasSignal) {
  const mid = h * 0.5;
  const layers = [
    { color: "rgba(255, 200, 100, 0.8)", speed: 1.0, amp: 1.0, width: 2 },
    { color: "rgba(255, 149, 0, 0.45)", speed: 1.4, amp: 0.6, width: 1.5 },
    { color: "rgba(200, 80, 0, 0.25)", speed: 0.7, amp: 1.3, width: 1 },
  ];

  layers.forEach((l, idx) => {
    ctx.beginPath();
    ctx.lineWidth = l.width;
    ctx.strokeStyle = l.color;

    for (let x = 0; x <= w; x += 3) {
      let offset = 0;
      if (hasSignal && timeData) {
        const sIdx = Math.floor((x / w) * timeData.length);
        offset = ((timeData[sIdx] - 128) / 128) * (h * 0.38) * l.amp;
      } else if (isAudioActive && samplePeaks.length > 0) {
        const frac = Math.min(1, Math.max(0, playheadFrac + (x / w - 0.5) * 0.4));
        const pIdx = Math.floor(frac * samplePeaks.length);
        const pk = samplePeaks[Math.min(samplePeaks.length - 1, Math.max(0, pIdx))] || 0;
        const ph = idlePhase * l.speed + x * 0.04 + idx;
        offset = (pk * 0.6 + playheadEnergy * 0.4) * Math.sin(ph) * (h * 0.32) * l.amp;
      } else {
        const ph = idlePhase * l.speed + x * 0.04 + idx;
        offset = (Math.sin(ph) * Math.cos(ph * 0.5) * 16) * l.amp;
      }

      const y = mid + offset;
      if (x === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  });

  // Glowing center horizon line
  ctx.strokeStyle = "rgba(255, 149, 0, 0.15)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(0, mid);
  ctx.lineTo(w, mid);
  ctx.stroke();
}
