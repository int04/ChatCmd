import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { ApprovalDecisionActions } from './ApprovalDecisionActions';

describe('ApprovalDecisionActions', () => {
  const target = { taskId: 'task', activityId: 'activity', turnId: 'turn' };

  it('offers a reusable grant only for safe-read approvals', () => {
    const { rerender } = render(<ApprovalDecisionActions target={target} reusable />);
    expect(screen.getByRole('button', { name: /allow similar/i })).toBeInTheDocument();
    rerender(<ApprovalDecisionActions target={target} reusable={false} />);
    expect(screen.queryByRole('button', { name: /allow similar/i })).not.toBeInTheDocument();
  });
});
