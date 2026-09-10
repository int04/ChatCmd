import { api } from './api';

export const DEFAULT_APP_FONT = 'Inter';
export const DEFAULT_TASK_FONT_SCALE = 100;
export const TASK_FONT_SCALE_PRESETS = [90, 100, 110, 120, 130] as const;

export const GOOGLE_FONT_PRESETS = [
  'Inter',
  'Be Vietnam Pro',
  'Roboto',
  'Noto Sans',
  'Open Sans',
  'Manrope',
] as const;

const FONT_LINK_ID = 'chatcmd-google-font';
const SYSTEM_FONT_STACK = 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif';
const MAX_UPLOADED_FONT_BYTES = 10 * 1024 * 1024;
let activeUploadedFont: FontFace | null = null;

export type UploadedFontMeta = { family: string; fileName: string; mimeType: string; size: number };

export function normalizeFontFamily(value?: string | null) {
  const font = value?.trim().replace(/[<>;"']/g, '').slice(0, 80);
  return font || DEFAULT_APP_FONT;
}

export function applyAppFont(value?: string | null) {
  if (typeof document === 'undefined') return;
  const font = normalizeFontFamily(value);
  clearUploadedFontFace();
  let link = document.getElementById(FONT_LINK_ID) as HTMLLinkElement | null;

  if (!link) {
    link = document.createElement('link');
    link.id = FONT_LINK_ID;
    link.rel = 'stylesheet';
    document.head.appendChild(link);
  }

  const family = encodeURIComponent(font).replace(/%20/g, '+');
  link.href = `https://fonts.googleapis.com/css2?family=${family}&display=swap`;
  document.documentElement.style.setProperty('--app-font-family', `"${font}", ${SYSTEM_FONT_STACK}`);
  document.documentElement.dataset.fontFamily = font;
  document.documentElement.dataset.fontSource = 'google';
}

export async function saveUploadedFont(file: File): Promise<UploadedFontMeta> {
  if (!/\.(ttf|otf|woff2?)$/i.test(file.name)) throw new Error('Only .ttf, .otf, .woff, and .woff2 font files are supported.');
  if (file.size <= 0 || file.size > MAX_UPLOADED_FONT_BYTES) throw new Error('Font file must be between 1 byte and 10 MB.');
  const family = normalizeFontFamily(file.name.replace(/\.(ttf|otf|woff2?)$/i, ''));
  const metadata = await api.uploadInterfaceFont(file, family);
  await applyUploadedFont(metadata.family);
  return metadata;
}

export async function applyUploadedFont(family?: string | null): Promise<UploadedFontMeta> {
  if (typeof FontFace === 'undefined' || typeof document === 'undefined') throw new Error('Custom fonts are not supported in this browser.');
  const normalizedFamily = normalizeFontFamily(family);
  const blob = await api.interfaceFontBlob();
  if (!blob.size) throw new Error('Uploaded font is no longer available.');
  const face = new FontFace(normalizedFamily, await blob.arrayBuffer());
  await face.load();
  clearUploadedFontFace();
  document.fonts.add(face);
  activeUploadedFont = face;
  const link = document.getElementById(FONT_LINK_ID) as HTMLLinkElement | null;
  if (link) link.removeAttribute('href');
  document.documentElement.style.setProperty('--app-font-family', `"${normalizedFamily}", ${SYSTEM_FONT_STACK}`);
  document.documentElement.dataset.fontFamily = normalizedFamily;
  document.documentElement.dataset.fontSource = 'uploaded';
  return { family: normalizedFamily, fileName: '', mimeType: blob.type || 'application/octet-stream', size: blob.size };
}

export async function removeUploadedFont() {
  await api.deleteInterfaceFont();
  clearUploadedFontFace();
}

function clearUploadedFontFace() {
  if (typeof document === 'undefined' || !activeUploadedFont) return;
  document.fonts.delete(activeUploadedFont);
  activeUploadedFont = null;
}

export function normalizeTaskFontScale(value?: number | null) {
  if (!Number.isFinite(value)) return DEFAULT_TASK_FONT_SCALE;
  return Math.min(130, Math.max(90, Math.round(Number(value))));
}

export function applyTaskFontScale(value?: number | null) {
  if (typeof document === 'undefined') return;
  const scale = normalizeTaskFontScale(value);
  const ratio = scale / 100;
  const root = document.documentElement;
  root.style.setProperty('--task-ui-scale', String(ratio));
  for (const base of [8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18]) {
    root.style.setProperty(`--task-font-${base}`, `${(base * ratio).toFixed(2)}px`);
  }
  const steps = (scale - DEFAULT_TASK_FONT_SCALE) / 5;
  root.style.setProperty('--task-space-delta', `${steps}px`);
  root.style.setProperty('--task-control-delta', `${steps * 1.5}px`);
  root.style.setProperty('--task-sidebar-delta', `${(scale - DEFAULT_TASK_FONT_SCALE) * 1.2}px`);
  root.dataset.taskFontScale = String(scale);
}
