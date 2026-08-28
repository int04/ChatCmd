import { Bot, CircleAlert, Clock3 } from 'lucide-react';
import { tr } from '../i18n';
import type { SubagentApproval } from '../types';
import { ApprovalDecisionActions } from './ApprovalDecisionActions';

export function SubagentApprovalQueue({ approvals, onResolved }: { approvals: SubagentApproval[]; onResolved: (activityId: string) => void }) {
  if (!approvals.length) return null;
  return <section className="subagent-approval-queue" aria-label={tr('Subagent permission requests')} aria-live="polite">
    <header><div><CircleAlert aria-hidden="true" /><strong>{tr('A subagent is requesting permission')}</strong></div><span>{tr('{count} requests', { count: approvals.length })}</span></header>
    <div className="subagent-approval-list">{approvals.map((approval) => <article className="subagent-approval-alert" key={`${approval.childTaskId}:${approval.activityId}`}>
      <div className="subagent-approval-copy"><span className="subagent-approval-icon" aria-hidden="true"><Bot /></span><div><strong>{approval.agentName}</strong><p>{tr('requests permission to run')} <code>{approval.tool || 'tool'}</code></p><small><Clock3 /> {formatApprovalInput(approval.input)}</small></div></div>
      <ApprovalDecisionActions target={{ taskId: approval.childTaskId, activityId: approval.activityId, turnId: approval.childTurnId }} onResolved={() => onResolved(approval.activityId)} />
    </article>)}</div>
  </section>;
}
function formatApprovalInput(input: unknown) { if (!input) return tr('No parameters'); try { const value = typeof input === 'string' ? input : JSON.stringify(input); return value.length > 180 ? `${value.slice(0, 179)}…` : value; } catch { return tr('Parameters cannot be displayed'); } }
