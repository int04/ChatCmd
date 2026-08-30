import type { Task, WorkspaceProject } from '../types';

export interface WorkspaceTaskGroups {
  projects: Array<{ project: WorkspaceProject; tasks: Task[] }>;
  unclassified: Task[];
}

export function canonicalProjectPath(path: string) {
  let clean = path.trim().replace(/\\/g, '/');
  while (clean.length > 1 && clean.endsWith('/')) clean = clean.slice(0, -1);
  return /^[a-z]:/i.test(clean) || clean.startsWith('//') ? clean.toLowerCase() : clean;
}

export function groupTasksByWorkspaceProjects(projects: WorkspaceProject[], tasks: Task[]): WorkspaceTaskGroups {
  const byPath = new Map<string, Task[]>();
  for (const project of projects) byPath.set(canonicalProjectPath(project.path), []);

  const unclassified: Task[] = [];
  for (const task of tasks) {
    const path = task.projectFolder?.trim();
    const group = path ? byPath.get(canonicalProjectPath(path)) : undefined;
    if (group) group.push(task);
    else unclassified.push(task);
  }

  return {
    projects: projects.map((project) => ({ project, tasks: byPath.get(canonicalProjectPath(project.path)) ?? [] })),
    unclassified,
  };
}
