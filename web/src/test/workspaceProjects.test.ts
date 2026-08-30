import { describe, expect, it } from 'vitest';

import { canonicalProjectPath, groupTasksByWorkspaceProjects } from '../tasks/workspaceProjects';
import type { Task, WorkspaceProject } from '../types';

describe('workspace projects', () => {
  it('canonicalizes equivalent Windows drive and UNC paths', () => {
    expect(canonicalProjectPath('D:\\DEV\\Dotty\\')).toBe('d:/dev/dotty');
    expect(canonicalProjectPath('d:/dev/dotty')).toBe('d:/dev/dotty');
    expect(canonicalProjectPath('\\\\Server\\Share\\Project\\')).toBe('//server/share/project');
    expect(canonicalProjectPath('C:\\')).toBe('c:');
    expect(canonicalProjectPath('/')).toBe('/');
  });

  it('groups tasks from backend projectFolder and leaves unknown folders unclassified', () => {
    const projects: WorkspaceProject[] = [{ id: 'project-1', name: 'Dotty', path: 'D:\\DEV\\Dotty' }];
    const task = (id: string, projectFolder?: string | null): Task => ({ id, projectFolder, status: 'completed', updatedAtUtc: '2026-01-01T00:00:00Z' });
    const groups = groupTasksByWorkspaceProjects(projects, [
      task('matched', 'd:/dev/dotty/'),
      task('manual', 'D:\\DEV\\Other'),
      task('missing'),
    ]);

    expect(groups.projects[0].tasks.map(({ id }) => id)).toEqual(['matched']);
    expect(groups.unclassified.map(({ id }) => id)).toEqual(['manual', 'missing']);
  });
});
