import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { api } from '../api';
import { SkillsPage } from '../pages/SkillsPage';

describe('SkillsPage GitHub installation', () => {
  beforeEach(() => {
    localStorage.clear();
    vi.spyOn(api, 'skills').mockResolvedValue([]);
  });

  afterEach(() => vi.restoreAllMocks());

  it('previews a multi-skill repository and installs only selected skills', async () => {
    const preview = vi.spyOn(api, 'previewSkills').mockResolvedValue({
      repositoryUrl: 'https://github.com/quangpl/browser-extension-skills',
      skippedInvalid: 0,
      skills: [
        { name: 'extension-create', title: 'extension-create', description: 'Create browser extensions', path: 'skills/extension-create', installed: false },
        { name: 'extension-test', title: 'extension-test', description: 'Test browser extensions', path: 'skills/extension-test', installed: false },
      ],
    });
    const install = vi.spyOn(api, 'installSkills').mockResolvedValue({
      skills: [{ id: 'extension-create', title: 'extension-create', description: 'Create browser extensions', source: 'global', enabled: true, canDelete: true, options: [] }],
    });
    const user = userEvent.setup();
    render(<SkillsPage />);

    await user.click(await screen.findByRole('button', { name: 'Add from GitHub' }));
    await user.type(screen.getByLabelText('GitHub repository'), 'https://github.com/quangpl/browser-extension-skills,');
    await user.click(screen.getByRole('button', { name: 'Find skills' }));

    expect(await screen.findByRole('heading', { name: '2 skills found' })).toBeInTheDocument();
    expect(preview).toHaveBeenCalledWith('https://github.com/quangpl/browser-extension-skills');
    const checkboxes = screen.getAllByRole('checkbox');
    expect(checkboxes).toHaveLength(2);
    expect(checkboxes[0]).toBeChecked();
    expect(checkboxes[1]).toBeChecked();

    await user.click(checkboxes[1]);
    await user.click(screen.getByRole('button', { name: 'Install 1 skill' }));

    expect(install).toHaveBeenCalledWith(
      'https://github.com/quangpl/browser-extension-skills',
      ['skills/extension-create'],
    );
    expect(await screen.findByRole('heading', { name: 'extension-create' })).toBeInTheDocument();
  });
});
