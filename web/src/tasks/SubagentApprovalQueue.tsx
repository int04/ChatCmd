import { Bot, CircleAlert, Clock3 } from 'lucide-react';

import type { SubagentApproval } from '../types';
import { ApprovalDecisionActions } from './ApprovalDecisionActions';

export function SubagentApprovalQueue({ approvals, onResolved }: { approvals: SubagentApproval[]; onResolved: (activityId: string) => void }) {
  if (!approvals.length) return null;
  return <section className="subagent-approval-queue" aria-label="Yêu cầu quyền từ Agent con" aria-live="polite">
    <header><div><CircleAlert aria-hidden="true" /><strong>Agent con đang xin quyền</strong></div><span>{approvals.length} yêu cầu</span></header>
    <div className="subagent-approval-list">
      {approvals.map((approval) => <article className="subagent-approval-alert" key={`${approval.childTaskId}:${approval.activityId}`}>
        <div className="subagent-approval-copy">
          <span className="subagent-approval-icon" aria-hidden="true"><Bot /></span>
          <div><strong>{approval.agentName}</strong><p>xin quyền chạy <code>{approval.tool || 'tool'}</code></p><small><Clock3 /> {formatApprovalInput(approval.input)}</small></div>
        </div>
        <ApprovalDecisionActions target={{ taskId: approval.childTaskId, activityId: approval.activityId, turnId: approval.childTurnId }} onResolved={() => onResolved(approval.activityId)} />
      </article>)}
    </div>
  </section>;
}

function formatApprovalInput(input: unknown) {
  if (!input) return 'Không có tham số';
  try {
    const value = typeof input === 'string' ? input : JSON.stringify(input);
    return value.length > 180 ? `${value.slice(0, 179)}…` : value;
  } catch { return 'Tham số không thể hiển thị'; }
}
