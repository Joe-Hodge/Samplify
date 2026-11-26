// WaveformSelector: A shared canvas-based audio waveform and selection editor.
// Unified implementation for popup.js and library.js.

export class WaveformSelector {
  constructor(canvasEl, wrapperEl, options = {}) {
    this.canvas = canvasEl;
    this.wrapper = wrapperEl || canvasEl;
    this.options = Object.assign({
      getPeaks: () => [],
      getDurationMs: () => 0,
      onSelectionChange: (startMs, endMs) => {},
      onSelectionDragMs: (startMs, endMs, mode) => {},
      onSeek: (ms) => {},
      isLooping: () => false,
      isPlaying: () => false,
      getPlayheadMs: () => null,
      minSelMs: 1000,
      allowMiddleDrag: false,
      allowPan: false,
      canDraw: () => true,
      renderStyle: 'card', // 'card' or 'library'
    }, options);

    this.viewStartMs = 0;
    this.viewEndMs = 0;
    this.selStartMs = 0;
    this.selEndMs = 0;
    this.hasSelection = false;
    this.drag = null; // { mode, anchorMs, offsetMs, moved, downX, downMs }
    this.drawQueued = false;

    this.initEvents();
  }

  clamp(v, lo, hi) {
    return Math.max(lo, Math.min(hi, v));
  }

  viewSpanMs() {
    return Math.max(1, this.viewEndMs - this.viewStartMs);
  }

  msToX(ms, w) {
    return ((ms - this.viewStartMs) / this.viewSpanMs()) * w;
  }

  xToMs(x, w) {
    return this.viewStartMs + (x / w) * this.viewSpanMs();
  }

  setPeaks(peaks) {
    // Force redraw
    this.scheduleDraw();
  }

  setDurationMs(durationMs) {
    this.viewStartMs = 0;
    this.viewEndMs = durationMs;
    this.scheduleDraw();
  }

  setSelection(startMs, endMs) {
    if (startMs === null || endMs === null) {
      this.hasSelection = false;
    } else {
      this.hasSelection = true;
      const durationMs = this.options.getDurationMs();
      this.selStartMs = this.clamp(startMs, 0, durationMs);
      this.selEndMs = this.clamp(endMs, 0, durationMs);
      
      // Ensure min selection limit
      const minSelMs = this.options.minSelMs;
      if (this.selEndMs - this.selStartMs < minSelMs) {
        this.selEndMs = Math.min(durationMs, this.selStartMs + minSelMs);
        if (this.selEndMs - this.selStartMs < minSelMs) {
          this.selStartMs = Math.max(0, this.selEndMs - minSelMs);
        }
      }
    }
    this.scheduleDraw();
  }

  clearSelection() {
    this.hasSelection = false;
    this.scheduleDraw();
  }

  getSelection() {
    if (!this.hasSelection) return null;
    return { startMs: this.selStartMs, endMs: this.selEndMs };
  }

  setSelectionInternal(startMs, endMs) {
    const durationMs = this.options.getDurationMs();
    let a = this.clamp(startMs, 0, durationMs);
    let b = this.clamp(endMs, 0, durationMs);
    const minSelMs = this.options.minSelMs;
    
    if (b - a < minSelMs) {
      if (this.drag && this.drag.mode === "left") {
        a = b - minSelMs;
        if (a < 0) { a = 0; b = minSelMs; }
      } else {
        b = a + minSelMs;
        if (b > durationMs) { b = durationMs; a = durationMs - minSelMs; }
      }
    }
    this.selStartMs = a;
    this.selEndMs = b;
    this.scheduleDraw();

    if (this.options.onSelectionDragMs) {
      this.options.onSelectionDragMs(this.selStartMs, this.selEndMs, this.drag ? this.drag.mode : null);
    }
  }

  setView(startMs, endMs) {
    const durationMs = this.options.getDurationMs();
    const minSpan = Math.min(durationMs, 500); // minimum 0.5s zoom window
    let a = startMs;
    let b = endMs;

    if (b - a < minSpan) b = a + minSpan;
    if (a < 0) { b -= a; a = 0; }
    if (b > durationMs) { a -= (b - durationMs); b = durationMs; if (a < 0) a = 0; }
    
    this.viewStartMs = a;
    this.viewEndMs = b;
    this.scheduleDraw();
  }

  fit() {
    this.viewStartMs = 0;
    this.viewEndMs = this.options.getDurationMs();
    this.scheduleDraw();
  }

  zoomToSelection() {
    if (!this.hasSelection) return;
    const pad = (this.selEndMs - this.selStartMs) * 0.15;
    this.setView(this.selStartMs - pad, this.selEndMs + pad);
  }

  zoomView(factor, centerMs) {
    const durationMs = this.options.getDurationMs();
    const span = this.viewSpanMs();
    const minSpan = Math.min(durationMs, 500);
    const newSpan = this.clamp(span * factor, minSpan, durationMs);
    const c = centerMs != null ? centerMs : (this.viewStartMs + this.viewEndMs) / 2;
    const frac = (c - this.viewStartMs) / span;
    this.setView(c - frac * newSpan, c - frac * newSpan + newSpan);
  }

  nudgeSelection(edge, deltaMs) {
    if (edge === "in") {
      this.setSelectionInternal(this.selStartMs + deltaMs, this.selEndMs);
    } else {
      this.setSelectionInternal(this.selStartMs, this.selEndMs + deltaMs);
    }
    this.scheduleDraw();
    if (this.options.onSelectionChange) {
      this.options.onSelectionChange(this.selStartMs, this.selEndMs);
    }
  }

  sizeCanvas() {
    const dpr = window.devicePixelRatio || 1;
    const w = this.canvas.clientWidth || 600;
    const h = this.canvas.clientHeight || 120;
    this.canvas.width = Math.max(1, Math.round(w * dpr));
    this.canvas.height = Math.max(1, Math.round(h * dpr));
    const ctx = this.canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    return { ctx, w, h };
  }

  scheduleDraw() {
    if (this.drawQueued) return;
    this.drawQueued = true;
    const run = () => {
      this.drawQueued = false;
      this.draw();
    };
    if (document.hidden) {
      setTimeout(run, 0);
    } else {
      requestAnimationFrame(run);
    }
  }

  draw() {
    const peaks = this.options.getPeaks();
    if (!peaks || peaks.length === 0) return;

    const { ctx, w, h } = this.sizeCanvas();
    ctx.clearRect(0, 0, w, h);

    const durationMs = this.options.getDurationMs() || 1;
    const isCard = this.options.renderStyle === 'card';
    const horizonY = Math.floor(h * (isCard ? 0.50 : 0.48));
    const gain = 1 / Math.max(0.05, peaks.reduce((m, v) => (v > m ? v : m), 0));

    // Pillar dimensions for volumetric 3D look
    const colW = isCard ? 2.2 : 2.8;
    const colGap = isCard ? 1.4 : 1.6;
    const step = colW + colGap;
    const numCols = Math.floor(w / step);

    // Optional subtle ambient stage glow behind active peaks
    const bgGlow = ctx.createRadialGradient(w * 0.5, horizonY * 0.9, 10, w * 0.5, horizonY, w * 0.6);
    bgGlow.addColorStop(0, "rgba(255, 149, 0, 0.04)");
    bgGlow.addColorStop(1, "rgba(0, 0, 0, 0)");
    ctx.fillStyle = bgGlow;
    ctx.fillRect(0, 0, w, h);

    // Precalculate selection bounds in pixels
    const selX0 = this.hasSelection ? this.clamp(this.msToX(this.selStartMs, w), 0, w) : 0;
    const selX1 = this.hasSelection ? this.clamp(this.msToX(this.selEndMs, w), 0, w) : w;

    // Selection background soft floor illumination
    if (this.hasSelection && selX1 > selX0) {
      const selGlow = ctx.createLinearGradient(0, 0, 0, h);
      selGlow.addColorStop(0, "rgba(255, 149, 0, 0.03)");
      selGlow.addColorStop(0.5, "rgba(255, 149, 0, 0.08)");
      selGlow.addColorStop(1, "rgba(255, 149, 0, 0.02)");
      ctx.fillStyle = selGlow;
      ctx.fillRect(selX0, 0, selX1 - selX0, h);
    }

    // Helper for rounded rect capsule with fallback
    const drawCapsule = (x, y, cw, ch, r) => {
      ctx.beginPath();
      if (ctx.roundRect) {
        ctx.roundRect(x, y, cw, ch, r);
      } else {
        ctx.rect(x, y, cw, ch);
      }
      ctx.fill();
    };

    // Render 3D Pillars & Surface Floor Reflections
    for (let c = 0; c < numCols; c++) {
      const colX = c * step;
      const msLo = this.xToMs(colX, w);
      const msHi = this.xToMs(colX + step, w);
      if (msHi < 0 || msLo > durationMs) continue;

      let iLo = Math.floor((msLo / durationMs) * peaks.length);
      let iHi = Math.ceil((msHi / durationMs) * peaks.length);
      iLo = this.clamp(iLo, 0, peaks.length - 1);
      iHi = this.clamp(iHi, iLo + 1, peaks.length);

      let peak = 0;
      for (let i = iLo; i < iHi; i++) if (peaks[i] > peak) peak = peaks[i];

      const norm = Math.min(1, peak * gain);
      const maxH = h * (isCard ? 0.78 : 0.70);
      const barH = Math.max(3, norm * maxH);
      const halfH = barH * 0.5;
      const topY = horizonY - halfH;
      const botY = horizonY + halfH;

      const inSel = this.hasSelection ? (colX + colW >= selX0 && colX <= selX1) : true;

      // 1. 3D Pillar Vertical Gradient
      const grad = ctx.createLinearGradient(0, topY, 0, botY);
      if (inSel) {
        grad.addColorStop(0, "#fff8e7");     // incandescent crest tip
        grad.addColorStop(0.18, "#ffc857");  // luminous gold highlight
        grad.addColorStop(0.5, "#ff9500");   // vibrant studio orange body
        grad.addColorStop(0.82, "#cc6200");  // warm burnt amber
        grad.addColorStop(1, "#732c00");     // dark amber baseline root
      } else {
        grad.addColorStop(0, "rgba(220, 150, 70, 0.32)");
        grad.addColorStop(0.5, "rgba(180, 100, 30, 0.22)");
        grad.addColorStop(1, "rgba(90, 45, 10, 0.12)");
      }
      ctx.fillStyle = grad;
      drawCapsule(colX, topY, colW, barH, Math.min(colW / 2, barH / 2));

      // 2. 3D Floor Horizon Reflection (mirror decaying downward)
      const refH = halfH * 0.65;
      if (refH > 2) {
        const refGrad = ctx.createLinearGradient(0, botY, 0, botY + refH);
        if (inSel) {
          refGrad.addColorStop(0, "rgba(255, 149, 0, 0.32)");
          refGrad.addColorStop(0.4, "rgba(255, 120, 0, 0.12)");
          refGrad.addColorStop(1, "rgba(255, 100, 0, 0.0)");
        } else {
          refGrad.addColorStop(0, "rgba(140, 75, 20, 0.10)");
          refGrad.addColorStop(1, "rgba(80, 40, 10, 0.0)");
        }
        ctx.fillStyle = refGrad;
        drawCapsule(colX, botY + 1, colW, refH, [0, 0, colW / 2, colW / 2]);
      }

      // 3. Floating Volumetric Ambient Sparks near intense crests
      if (norm > 0.62 && (c % 3 === 0)) {
        const sparkOffset = ((c * 47) % 22) + 4;
        const sparkY = topY - sparkOffset;
        if (sparkY > 6) {
          const sparkA = inSel ? 0.45 + 0.4 * ((c * 17) % 10) / 10 : 0.15;
          const sparkR = 1.0 + ((c * 7) % 3) * 0.4;
          ctx.fillStyle = inSel ? `rgba(255, 235, 180, ${sparkA})` : `rgba(200, 130, 60, ${sparkA * 0.4})`;
          ctx.beginPath();
          ctx.arc(colX + colW * 0.5, sparkY, sparkR, 0, Math.PI * 2);
          ctx.fill();
        }
      }
    }

    // 4. Selection Boundary Handles & Fog Dimming
    if (this.hasSelection) {
      // Atmospheric fog over unselected outer flanks
      ctx.fillStyle = "rgba(9, 10, 15, 0.55)";
      if (selX0 > 0) ctx.fillRect(0, 0, selX0, h);
      if (selX1 < w) ctx.fillRect(selX1, 0, w - selX1, h);

      // Boundary vertical glass lasers & handles
      const drawBoundary = (bx, isLeft) => {
        if (bx < 0 || bx > w) return;
        // Laser beam
        ctx.fillStyle = "rgba(255, 149, 0, 0.95)";
        ctx.fillRect(bx - 1, 0, 2, h);
        // Soft aura
        ctx.fillStyle = "rgba(255, 149, 0, 0.18)";
        ctx.fillRect(bx - 3, 0, 6, h);

        // Center grip pill
        const gripH = 26;
        const gripW = 5;
        const gripX = isLeft ? bx : bx - gripW;
        ctx.fillStyle = "#ffaa33";
        drawCapsule(gripX, horizonY - gripH * 0.5, gripW, gripH, 2.5);
      };

      drawBoundary(selX0, true);
      drawBoundary(selX1, false);
    }

    // 5. Volumetric 3D Playhead with Floating Timestamp Pill
    const playheadMs = this.options.getPlayheadMs ? this.options.getPlayheadMs() : null;
    if (playheadMs !== null && playheadMs >= this.viewStartMs && playheadMs <= this.viewEndMs) {
      const px = this.msToX(playheadMs, w);
      if (px >= 0 && px <= w) {
        // Vertical laser core + glow aura
        ctx.fillStyle = "rgba(255, 160, 30, 0.28)";
        ctx.fillRect(px - 3, 0, 6, h);

        ctx.fillStyle = "#ffffff";
        ctx.fillRect(px - 0.75, 0, 1.5, h);

        // Floor contact hotspot (illuminated ellipse on floor plane)
        ctx.fillStyle = "rgba(255, 200, 80, 0.75)";
        ctx.beginPath();
        ctx.ellipse(px, horizonY + 2, 6, 2.2, 0, 0, Math.PI * 2);
        ctx.fill();

        // Top Floating Time Badge (Pill)
        const totalSec = Math.max(0, playheadMs / 1000);
        const m = Math.floor(totalSec / 60);
        const s = totalSec % 60;
        const timeText = `${m}:${s < 10 ? '0' : ''}${s.toFixed(1)}`;

        ctx.font = "600 10px Inter, system-ui, sans-serif";
        const textWidth = ctx.measureText(timeText).width;
        const pillW = textWidth + 14;
        const pillH = 18;
        const pillX = Math.max(4, Math.min(w - pillW - 4, px - pillW / 2));
        const pillY = 4;

        // Badge shadow & background
        ctx.fillStyle = "rgba(14, 17, 24, 0.92)";
        ctx.strokeStyle = "rgba(255, 149, 0, 0.75)";
        ctx.lineWidth = 1;
        ctx.beginPath();
        if (ctx.roundRect) {
          ctx.roundRect(pillX, pillY, pillW, pillH, 9);
        } else {
          ctx.rect(pillX, pillY, pillW, pillH);
        }
        ctx.fill();
        ctx.stroke();

        // Badge text
        ctx.fillStyle = "#ffffff";
        ctx.textAlign = "center";
        ctx.textBaseline = "middle";
        ctx.fillText(timeText, pillX + pillW / 2, pillY + pillH / 2);
      }
    }
  }


  initEvents() {
    const canvas = this.canvas;
    const wrapper = this.wrapper;

    wrapper.addEventListener("mousedown", (e) => {
      if (e.target.closest && (e.target.closest(".waveform-pin") || e.target.closest(".wave-toolbar"))) return;

      const durationMs = this.options.getDurationMs();
      if (!durationMs) return;

      const rect = canvas.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const w = rect.width;
      const ms = this.clamp(this.xToMs(x, w), 0, durationMs);
      const grab = 9;

      if (this.hasSelection) {
        const x0 = this.msToX(this.selStartMs, w);
        const x1 = this.msToX(this.selEndMs, w);

        if (Math.abs(x - x0) < grab) {
          this.drag = { mode: "left", moved: false, downX: e.clientX };
          if (this.options.onActiveEdgeChange) this.options.onActiveEdgeChange("in");
          e.preventDefault();
          return;
        }
        if (Math.abs(x - x1) < grab) {
          this.drag = { mode: "right", moved: false, downX: e.clientX };
          if (this.options.onActiveEdgeChange) this.options.onActiveEdgeChange("out");
          e.preventDefault();
          return;
        }
      }

      if (this.options.allowPan && e.shiftKey) {
        this.drag = {
          mode: "pan",
          anchorMs: ms,
          viewA: this.viewStartMs,
          viewB: this.viewEndMs,
          px: e.clientX,
          w
        };
        e.preventDefault();
        return;
      }

      if (this.options.allowMiddleDrag && this.hasSelection && ms > this.selStartMs && ms < this.selEndMs) {
        this.drag = {
          mode: "middle",
          anchorMs: ms,
          offsetMs: ms - this.selStartMs,
          moved: false,
          downX: e.clientX
        };
        e.preventDefault();
        return;
      }

      // Draw mode
      this.drag = {
        mode: "draw",
        moved: false,
        downX: e.clientX,
        downMs: ms
      };
      e.preventDefault();
    });

    window.addEventListener("mousemove", (e) => {
      if (!this.drag) return;
      const durationMs = this.options.getDurationMs();
      if (!durationMs) return;

      const rect = canvas.getBoundingClientRect();
      const w = rect.width;
      const x = Math.max(0, Math.min(w, e.clientX - rect.left));
      const ms = this.xToMs(x, w);

      if (!this.drag.moved && Math.abs(e.clientX - this.drag.downX) <= 3) return;
      this.drag.moved = true;

      if (this.drag.mode === "left") {
        this.setSelectionInternal(ms, this.selEndMs);
      } else if (this.drag.mode === "right") {
        this.setSelectionInternal(this.selStartMs, ms);
      } else if (this.drag.mode === "middle") {
        const len = this.selEndMs - this.selStartMs;
        let a = this.clamp(ms - this.drag.offsetMs, 0, durationMs - len);
        this.setSelectionInternal(a, a + len);
      } else if (this.drag.mode === "draw") {
        if (this.options.canDraw && !this.options.canDraw()) return;
        this.hasSelection = true;
        this.setSelectionInternal(
          Math.min(this.drag.downMs, ms),
          Math.max(this.drag.downMs, ms)
        );
      } else if (this.drag.mode === "pan") {
        const dMs = ((e.clientX - this.drag.px) / w) * (this.drag.viewB - this.drag.viewA);
        this.setView(this.drag.viewA - dMs, this.drag.viewB - dMs);
      }
    });

    window.addEventListener("mouseup", (e) => {
      if (!this.drag) return;
      const dragMode = this.drag.mode;
      const moved = this.drag.moved;
      
      const durationMs = this.options.getDurationMs();
      
      const drag = this.drag;
      this.drag = null;

      if (dragMode === "middle" && !moved) {
        // Clicked inside selection -> seek
        const rect = canvas.getBoundingClientRect();
        const x = Math.max(0, Math.min(rect.width, e.clientX - rect.left));
        let ms = this.clamp(this.xToMs(x, rect.width), this.selStartMs, this.selEndMs);
        if (this.options.onSeek) this.options.onSeek(ms);
      } else if (dragMode === "draw" && !moved) {
        // Clicked outside selection -> seek
        const rect = canvas.getBoundingClientRect();
        const x = Math.max(0, Math.min(rect.width, e.clientX - rect.left));
        let ms = this.xToMs(x, rect.width);
        if (this.hasSelection) {
          ms = this.clamp(ms, this.selStartMs, this.selEndMs);
        } else {
          ms = this.clamp(ms, 0, durationMs);
        }
        if (this.options.onSeek) this.options.onSeek(ms);
      } else if (moved && (dragMode === "left" || dragMode === "right" || dragMode === "draw")) {
        if (this.options.onSelectionChange) {
          this.options.onSelectionChange(this.selStartMs, this.selEndMs);
        }
      }
      this.scheduleDraw();
    });

    wrapper.addEventListener("wheel", (e) => {
      const durationMs = this.options.getDurationMs();
      if (!durationMs) return;

      e.preventDefault();
      const rect = canvas.getBoundingClientRect();
      const x = Math.max(0, Math.min(rect.width, e.clientX - rect.left));
      const centerMs = this.xToMs(x, rect.width);
      const factor = e.deltaY > 0 ? 1.2 : 1 / 1.2;
      this.zoomView(factor, centerMs);
    }, { passive: false });
  }
}
