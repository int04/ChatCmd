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

  it('surfaces v2 directory metadata, continuation, truncation, and content reference metadata', () => {
    const output = formatToolOutput('fs_list_v2', {
      schemaVersion: 1,
      data: {
        items: [{ path: 'D:/repo/a.rs', name: 'a.rs', entryType: 'file', size: 10 }],
        directoryVersion: 'sha256:directory-version',
        sort: 'filesystem',
      },
      page: { nextCursor: 'opaque-secret-cursor', hasMore: true },
      truncation: { truncated: true, reason: 'metadataBudget', returnedItems: 1 },
      contentRef: { id: 'artifact-1', mediaType: 'application/json' },
    });

    expect(output).toContain('📄 D:/repo/a.rs');
    expect(output).toContain('Thứ tự: Filesystem');
    expect(output).toContain('Phiên bản thư mục: sha256:directory-version');
    expect(output).toContain('Còn dữ liệu ở trang tiếp theo.');
    expect(output).toContain('Kết quả bị cắt: Metadata Budget');
    expect(output).toContain('Nội dung đầy đủ: artifact-1');
    expect(output).not.toContain('opaque-secret-cursor');
  });

  it('renders command identity and terminal facts without exposing argument values', () => {
    const output = formatToolOutput('command_run', {
      executionId: 'execution-1',
      command: { executable: 'cargo', argumentCount: 3, argumentsSha256: 'sha256:test' },
      cwd: 'D:/repo', terminalState: 'exited', exitCode: 7, elapsedMs: 42,
      stdout: 'PASS text is not authoritative', stderr: 'failure details',
    });

    expect(output).toContain('Lệnh: cargo');
    expect(output).toContain('Exit code: 7');
    expect(output).toContain('Thư mục: D:/repo');
    expect(output).not.toContain('sha256:test');
  });
});
