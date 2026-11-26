// bridge.js — Tauri v2 global API (withGlobalTauri = true)
export const invoke = window.__TAURI__.core.invoke;
export const listen = window.__TAURI__.event.listen;
export const emit = window.__TAURI__.event.emit;
export const getCurrentWindow = window.__TAURI__.window.getCurrentWindow;
export const convertFileSrc = window.__TAURI__.core.convertFileSrc;
