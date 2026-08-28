import { useCallback, useEffect, useState } from 'react';

export function useResizableWidth({ storageKey, cssVariable, defaultWidth, minWidth, maxWidth, direction = 1 }: {
  storageKey: string;
  cssVariable: string;
  defaultWidth: number;
  minWidth: number;
  maxWidth: number;
  direction?: 1 | -1;
}) {
  const [width, setWidth] = useState(() => readWidth(storageKey, defaultWidth, minWidth, maxWidth));

  useEffect(() => {
    document.documentElement.style.setProperty(cssVariable, `${width}px`);
    try { localStorage.setItem(storageKey, String(width)); } catch { /* storage can be unavailable */ }
  }, [cssVariable, storageKey, width]);

  const resizeTo = useCallback((value: number) => setWidth(clamp(value, minWidth, maxWidth)), [maxWidth, minWidth]);

  const onPointerDown = useCallback((event: React.PointerEvent<HTMLElement>) => {
    if (event.pointerType === 'mouse' && event.button !== 0) return;
    event.preventDefault();
    const startX = event.clientX;
    const startWidth = width;
    const target = event.currentTarget;
    target.setPointerCapture(event.pointerId);
    document.documentElement.classList.add('panel-resizing');

    const move = (moveEvent: PointerEvent) => resizeTo(startWidth + (moveEvent.clientX - startX) * direction);
    const end = () => {
      document.documentElement.classList.remove('panel-resizing');
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', end);
      target.removeEventListener('pointercancel', end);
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', end);
    target.addEventListener('pointercancel', end);
  }, [direction, resizeTo, width]);

  const onKeyDown = useCallback((event: React.KeyboardEvent<HTMLElement>) => {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const delta = event.key === 'ArrowRight' ? 12 : -12;
    resizeTo(width + delta * direction);
  }, [direction, resizeTo, width]);

  return { width, onPointerDown, onKeyDown };
}

function readWidth(storageKey: string, fallback: number, min: number, max: number) {
  try {
    const value = Number(localStorage.getItem(storageKey));
    return Number.isFinite(value) && value > 0 ? clamp(value, min, max) : fallback;
  } catch { return fallback; }
}
function clamp(value: number, min: number, max: number) { return Math.min(max, Math.max(min, Math.round(value))); }
