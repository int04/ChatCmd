import { describe, expect, it } from 'vitest';
import { formatToolOutput } from '../tasks/taskToolOutput';

describe('tool result envelope rendering', () => {
  it('keeps the legacy fs_list array renderer unchanged', () => {
    const output = formatToolOutput('fs_list', [
      { path: 'D:/repo/src', name: 'src', entryType: 'directory', size: 0 },
    ]);

    expect(output).toContain('📁 D:/repo/src');
    expect(output).not.toContain('Còn dữ liệu');
  });

  it('surfaces continuation, truncation, and content reference metadata for fs_list_v2', () => {
    const output = formatToolOutput('fs_list_v2', {
      schemaVersion: 1,
      data: [{ path: 'D:/repo/a.rs', name: 'a.rs', entryType: 'file', size: 10 }],
      page: { nextCursor: 'opaque-secret-cursor', hasMore: true },
      truncation: { truncated: true, reason: 'byteBudget', returnedItems: 1 },
      contentRef: { id: 'artifact-1', mediaType: 'application/json' },
    });

    expect(output).toContain('📄 D:/repo/a.rs');
    expect(output).toContain('Còn dữ liệu ở trang tiếp theo.');
    expect(output).toContain('Kết quả bị cắt: Byte Budget');
    expect(output).toContain('Nội dung đầy đủ: artifact-1');
    expect(output).not.toContain('opaque-secret-cursor');
  });
});
