import { describe, expect, it } from 'vitest';
import { prepareChatGptMessage } from './messageAttachments';

describe('prepareChatGptMessage', () => {
  it('sends the raw trimmed message when nothing is attached', () => {
    expect(prepareChatGptMessage('  Xin chào  ', {})).toBe('Xin chào');
  });

  it('adds only the selected plugin', () => {
    expect(prepareChatGptMessage('Làm việc này', { pluginName: 'rust_test' }))
      .toBe('plugin @rust_test\n\nyêu cầu: Làm việc này');
  });

  it('adds only the selected project folder', () => {
    expect(prepareChatGptMessage('Làm việc này', { projectFolder: ' D:\\DEV\\CmdGPT ' }))
      .toBe('Thư mục dự án: D:\\DEV\\CmdGPT\n\nyêu cầu: Làm việc này');
  });

  it('adds plugin and project in the requested order', () => {
    expect(prepareChatGptMessage('Làm việc này', { pluginName: 'rust_test', projectFolder: 'D:\\DEV\\CmdGPT' }))
      .toBe('plugin @rust_test\nThư mục dự án: D:\\DEV\\CmdGPT\n\nyêu cầu: Làm việc này');
  });
});
