import { describe, expect, it } from 'vitest';
import { isExtensionVersionOutdated } from '../extensions/version';

describe('ChatGPT extension minimum version', () => {
  it('accepts the detected 0.1.20 extension when an older ChatCMD build requires 0.1.17', () => {
    expect(isExtensionVersionOutdated('0.1.20', '0.1.17')).toBe(false);
  });

  it('compares numeric parts and rejects only older versions', () => {
    expect(isExtensionVersionOutdated('0.1.9', '0.1.17')).toBe(true);
    expect(isExtensionVersionOutdated('0.1.17', '0.1.17')).toBe(false);
    expect(isExtensionVersionOutdated('0.1.17.1', '0.1.17')).toBe(false);
    expect(isExtensionVersionOutdated('0.1.17', '0.1.17.1')).toBe(true);
  });
});
