'use strict';

/**
 * Voice overlay window resizer — eased right-edge-anchored bounds morph.
 *
 * Extracted from main.cjs so the same implementation runs in the app and in
 * the Electron smoke tests (scratch/electron_resize_smoke.cjs).
 *
 * Right-edge anchoring: the window grows/shrinks toward the LEFT so the mic
 * orb stays under the cursor position — dragging still decides the edge.
 * VOICE_NO_RESIZE_ANIM=1 forces the instant path (safe fallback).
 */

const EASE_MS = 260;

function createVoiceResizer({ envVar = 'VOICE_NO_RESIZE_ANIM', durationMs = EASE_MS } = {}) {
  let timer = null;

  function stop() {
    if (timer) {
      clearInterval(timer);
      timer = null;
    }
  }

  function setBounds(win, target) {
    if (!win || win.isDestroyed()) return;
    try {
      win.setBounds(target);
    } catch (_e) {}
  }

  function animateTo(win, target, duration) {
    stop();
    if (!win || win.isDestroyed()) return;
    const start = win.getBounds();
    const t0 = Date.now();
    timer = setInterval(() => {
      if (!win || win.isDestroyed()) {
        stop();
        return;
      }
      const p = Math.min(1, (Date.now() - t0) / duration);
      const e = 1 - Math.pow(1 - p, 3); // easeOutCubic
      setBounds(win, {
        x: Math.round(start.x + (target.x - start.x) * e),
        y: Math.round(start.y + (target.y - start.y) * e),
        width: Math.round(start.width + (target.width - start.width) * e),
        height: Math.round(start.height + (target.height - start.height) * e),
      });
      if (p >= 1) stop();
    }, 16);
  }

  function resize(win, w, h, animate) {
    if (!win || win.isDestroyed()) return;
    const bounds = win.getBounds();
    const nextWidth = Math.round(w);
    const nextHeight = Math.round(h);
    const target = {
      x: bounds.x - (nextWidth - bounds.width),
      y: bounds.y,
      width: nextWidth,
      height: nextHeight,
    };
    if (animate && process.env[envVar] !== '1') {
      animateTo(win, target, durationMs);
    } else {
      stop();
      setBounds(win, target);
    }
  }

  return { resize, stop };
}

module.exports = { createVoiceResizer };
