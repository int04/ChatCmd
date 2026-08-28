import { afterEach, describe, expect, it } from 'vitest';

import { resolveAppLanguage, setAppLanguage, tr } from '../i18n';

describe('app language resolution', () => {
  afterEach(() => setAppLanguage('en', false));

  it('uses Vietnamese for vi browser locales and English for en locales', () => {
    expect(resolveAppLanguage('vi-VN')).toBe('vi');
    expect(resolveAppLanguage('en-US')).toBe('en');
  });

  it('falls back to English for every unsupported browser locale', () => {
    expect(resolveAppLanguage('fr-FR')).toBe('en');
    expect(resolveAppLanguage('ja-JP')).toBe('en');
    expect(resolveAppLanguage('')).toBe('en');
  });

  it('lets a saved user choice override the browser locale', () => {
    expect(resolveAppLanguage('en-US', 'vi')).toBe('vi');
    expect(resolveAppLanguage('vi-VN', 'en')).toBe('en');
  });

  it('switches translated UI text at runtime', () => {
    setAppLanguage('vi', false);
    expect(tr('Settings')).toBe('Cài đặt');
    setAppLanguage('en', false);
    expect(tr('Settings')).toBe('Settings');
  });
});
