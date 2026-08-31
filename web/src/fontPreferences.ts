export const DEFAULT_APP_FONT = 'Inter';

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

export function normalizeFontFamily(value?: string | null) {
  const font = value?.trim().replace(/[<>;"']/g, '').slice(0, 80);
  return font || DEFAULT_APP_FONT;
}

export function applyAppFont(value?: string | null) {
  if (typeof document === 'undefined') return;
  const font = normalizeFontFamily(value);
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
}
