import { Check, LoaderCircle, ShieldCheck, ShieldX } from 'lucide-react';
import { useEffect, useState } from 'react';
import { api } from '../api';
import { tr } from '../i18n';
import type { ApprovalGrant, CommandExecutionMode } from '../types';

export function TaskAccessCard({ taskId, grantTaskId = taskId, defaultMode, grants = [] }: { taskId: string; grantTaskId?: string; defaultMode: CommandExecutionMode; grants?: ApprovalGrant[] }) {
  const [mode, setMode] = useState<CommandExecutionMode>(defaultMode); const [saving, setSaving] = useState(false); const [error, setError] = useState('');
  const [activeGrants, setActiveGrants] = useState(grants);
  useEffect(() => {
    setMode(defaultMode); setError(''); const controller = new AbortController();
    void api.taskExecutionMode(taskId, controller.signal).then((current) => setMode(current.mode)).catch((reason: unknown) => { if (!(reason instanceof DOMException && reason.name === 'AbortError')) setError(tr('Could not change access permissions. Please try again.')); });
    return () => controller.abort();
  }, [defaultMode, taskId]);
  useEffect(() => setActiveGrants(grants), [grants]);
  async function changeMode(next: CommandExecutionMode) {
    if (next === mode || saving) return; const previous = mode; setMode(next); setSaving(true); setError('');
    try { const current = await api.setTaskExecutionMode(taskId, next); setMode(current.mode); }
    catch { setMode(previous); setError(tr('Could not change access permissions. Please try again.')); }
    finally { setSaving(false); }
  }
  const options: { value: CommandExecutionMode; title: string; hint: string }[] = [
    { value: 'approval', title: tr('Approval'), hint: tr('Ask before running each command that has not been allowed.') },
    { value: 'allowAll', title: tr('Allow everything'), hint: tr('Run commands immediately without asking for approval first.') },
  ];
  return <section className="task-access-card" aria-labelledby={`task-access-title-${taskId}`}>
    <header><ShieldCheck aria-hidden="true" /><div><h3 id={`task-access-title-${taskId}`}>{tr('Access permissions')}</h3><p>{tr('Only applies to this conversation and commands requested next.')}</p></div></header>
    <fieldset disabled={saving} aria-describedby={error ? `task-access-error-${taskId}` : undefined}><legend className="sr-only">{tr('Access permissions')}</legend>{options.map((option) => <label className={mode === option.value ? 'active' : ''} key={option.value}><input type="radio" name={`task-access-${taskId}`} value={option.value} checked={mode === option.value} onChange={() => void changeMode(option.value)} /><span><strong>{option.title}</strong><small>{option.hint}</small></span>{mode === option.value && <Check aria-hidden="true" />}</label>)}</fieldset>
    {saving && <p className="task-access-feedback" role="status"><LoaderCircle className="spin" aria-hidden="true" />{tr('Applying…')}</p>}
    {error && <p className="task-access-error" id={`task-access-error-${taskId}`} role="alert">{error}</p>}
    {activeGrants.length > 0 && <div className="approval-grant-list"><h4>{tr('Active scoped grants')}</h4>{activeGrants.map((grant) => <div key={grant.id} className="approval-grant-item"><div><strong>{grant.allowedTools.join(', ')}{grant.inheritedFrom ? ` · ${tr('Inherited')}` : ''}</strong><small>{tr('{used} of {total} calls used', { used: grant.usedCalls, total: grant.maxCalls })} · {grant.pathScopes.map((scope) => scope.path).join(', ')}{grant.inheritedFrom ? ` · ${grant.inheritedFrom.slice(0, 8)}…${grant.childAttempt ? ` · #${grant.childAttempt}` : ''}` : ''}</small></div><button type="button" className="button danger compact" onClick={() => void api.revokeTaskApprovalGrant(grantTaskId, grant.id).then(() => setActiveGrants((items) => items.filter((item) => item.id !== grant.id))).catch(() => setError(tr('Could not revoke approval grant.')))}><ShieldX />{tr('Revoke')}</button></div>)}</div>}
  </section>;
}
