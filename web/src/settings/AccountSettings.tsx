import { KeyRound, Orbit, ShieldCheck, UserRound } from 'lucide-react';
import { useMemo, useState } from 'react';
import { api } from '../api';
import { useAuth } from '../auth';
import { tr } from '../i18n';

type AccountTab = 'info' | 'password';

export function AccountSettings() {
  const { user } = useAuth();
  const [activeTab, setActiveTab] = useState<AccountTab>('info');
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [problem, setProblem] = useState('');
  const [submitting, setSubmitting] = useState(false);
  const [challenge, setChallenge] = useState(() => createHumanChallenge());
  const [challengeProgress, setChallengeProgress] = useState<string[]>([]);
  const [challengeVerified, setChallengeVerified] = useState(false);
  const [challengeHint, setChallengeHint] = useState('');
  const [challengeOpen, setChallengeOpen] = useState(false);

  const planRemaining = useMemo(() => formatRemaining(user?.plan.expriAt), [user?.plan.expriAt]);
  if (!user) return null;
  const isFree = user.plan.type === 0 || user.plan.name.toUpperCase() === 'FREE';

  const resetChallenge = () => {
    setChallenge(createHumanChallenge());
    setChallengeProgress([]);
    setChallengeVerified(false);
    setChallengeHint('');
  };

  const submitPasswordChange = async () => {
    setSubmitting(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      window.dispatchEvent(new Event('chatcmd:auth-required'));
    } catch (error) {
      setChallengeOpen(false);
      setProblem(error instanceof Error ? error.message : tr('Password change failed.'));
    } finally {
      setSubmitting(false);
    }
  };

  const requestPasswordChange = () => {
    setProblem('');
    if (!currentPassword) return setProblem(tr('Current password is required.'));
    if (newPassword.length < 8) return setProblem(tr('New password must contain at least 8 characters.'));
    if (newPassword !== confirmPassword) return setProblem(tr('Password confirmation does not match.'));
    if (currentPassword === newPassword) return setProblem(tr('New password must be different from current password.'));
    resetChallenge();
    setChallengeOpen(true);
  };

  const pickAndContinue = (token: string) => {
    if (challengeVerified || challengeProgress.includes(token)) return;
    const expected = challenge.sequence[challengeProgress.length];
    if (token !== expected) {
      setChallengeProgress([]);
      setChallengeHint(tr('Wrong pattern. The sequence has been reset.'));
      return;
    }
    const next = [...challengeProgress, token];
    setChallengeProgress(next);
    setChallengeHint('');
    if (next.length === challenge.sequence.length) {
      setChallengeVerified(true);
      window.setTimeout(() => void submitPasswordChange(), 320);
    }
  };

  return <div className="account-settings">
    <div className="account-subtabs" role="tablist" aria-label={tr('Account sections')}>
      <button type="button" role="tab" aria-selected={activeTab === 'info'} className={activeTab === 'info' ? 'active' : ''} onClick={() => setActiveTab('info')}><UserRound />{tr('Information')}</button>
      <button type="button" role="tab" aria-selected={activeTab === 'password'} className={activeTab === 'password' ? 'active' : ''} onClick={() => setActiveTab('password')}><KeyRound />{tr('Change password')}</button>
    </div>

    {activeTab === 'info' && <div className="account-info-grid" role="tabpanel">
      <AccountValue label="ID" value={String(user.id)} />
      <AccountValue label={tr('Email')} value={user.email} />
      <AccountValue label={tr('Plan')} value={user.plan.name} />
      {!isFree && user.plan.expriAt && <AccountValue label={tr('Plan time remaining')} value={planRemaining ?? formatDate(user.plan.expriAt)} hint={formatDate(user.plan.expriAt)} />}
      {isFree && <AccountValue label={tr('Use until')} value={formatDate(user.useNextTime)} />}
      {isFree && <AccountValue label={tr('Next reset')} value={formatDate(user.useNextReset)} />}
    </div>}

    {activeTab === 'password' && <div className="account-password-panel" role="tabpanel">
      <div className="account-security-note"><ShieldCheck /><div><strong>{tr('Security')}</strong><span>{tr('Changing your password signs this device out and revokes all refresh tokens. Sign in again with the new password.')}</span></div></div>
      <div className="account-password-grid">
        <label>{tr('Current password')}<input type="password" autoComplete="current-password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} /></label>
        <label>{tr('New password')}<input type="password" autoComplete="new-password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} /></label>
        <label>{tr('Confirm new password')}<input type="password" autoComplete="new-password" value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} /></label>
      </div>
      {problem && <div className="account-password-error" role="alert">{problem}</div>}
      <div className="account-password-actions"><button type="button" className="button primary" disabled={submitting} onClick={requestPasswordChange}><KeyRound />{submitting ? tr('Changing password...') : tr('Change password')}</button></div>
    </div>}

    {challengeOpen && <div className="modal-backdrop account-constellation-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !submitting) setChallengeOpen(false); }}>
      <div className={`modal account-constellation-modal ${challengeVerified ? 'verified' : ''}`} role="dialog" aria-modal="true" aria-labelledby="security-constellation-title">
        <div className="account-constellation-heading">
          <span><Orbit /></span>
          <div><strong id="security-constellation-title">{tr('Security constellation')}</strong><small>{tr('Connect the stars in the shown order to unlock this action.')}</small></div>
          <button type="button" disabled={submitting} onClick={resetChallenge}>{tr('Shuffle')}</button>
        </div>
        <div className="account-constellation-sequence" aria-label={tr('Constellation route')}>
          {challenge.sequence.map((token, index) => <span key={token} className={index < challengeProgress.length ? 'done' : ''}>{token}</span>)}
        </div>
        <div className="account-constellation-board">
          <svg viewBox="0 0 300 300" aria-hidden="true">
            {challengeProgress.slice(1).map((token, index) => {
              const from = challenge.nodes.find((node) => node.id === challengeProgress[index]);
              const to = challenge.nodes.find((node) => node.id === token);
              return from && to ? <line key={`${from.id}-${to.id}`} x1={from.x} y1={from.y} x2={to.x} y2={to.y} /> : null;
            })}
          </svg>
          {challenge.nodes.map((node) => <button key={node.id} type="button" className={challengeProgress.includes(node.id) ? 'active' : ''} style={{ left: `${node.x / 3}%`, top: `${node.y / 3}%` }} disabled={submitting || challengeVerified || challengeProgress.includes(node.id)} onClick={() => pickAndContinue(node.id)} aria-label={node.id}><span>{node.id}</span></button>)}
        </div>
        <div className="account-constellation-status" role="status">{submitting ? tr('Changing password...') : challengeVerified ? tr('Security check complete. You can continue.') : challengeHint || tr('{count}/{total} stars connected', { count: challengeProgress.length, total: challenge.sequence.length })}</div>
        <div className="account-constellation-modal-actions"><button type="button" className="button secondary" disabled={submitting} onClick={() => setChallengeOpen(false)}>{tr('Cancel')}</button></div>
      </div>
    </div>}
  </div>;
}

function createHumanChallenge() {
  const ids = ['N1', 'N2', 'N3', 'N4', 'N5', 'N6', 'N7', 'N8', 'N9'];
  const positions = [
    [45, 45], [150, 38], [255, 54],
    [58, 150], [150, 142], [244, 158],
    [48, 252], [150, 244], [252, 248],
  ] as const;
  const nodes = shuffle(ids).map((id, index) => ({ id, x: positions[index][0], y: positions[index][1] }));
  return { nodes, sequence: shuffle(ids).slice(0, 4) };
}

function shuffle<T>(items: T[]) {
  const result = [...items];
  for (let index = result.length - 1; index > 0; index -= 1) {
    const swapIndex = Math.floor(Math.random() * (index + 1));
    [result[index], result[swapIndex]] = [result[swapIndex], result[index]];
  }
  return result;
}

function AccountValue({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return <div className="account-value"><span>{label}</span><strong>{value || '—'}</strong>{hint && <small>{hint}</small>}</div>;
}

function formatDate(value?: string | null) {
  if (!value) return '—';
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(date);
}

function formatRemaining(value?: string | null) {
  if (!value) return null;
  const milliseconds = new Date(value).getTime() - Date.now();
  if (!Number.isFinite(milliseconds)) return null;
  if (milliseconds <= 0) return tr('Expired');
  const days = Math.floor(milliseconds / 86_400_000);
  const hours = Math.floor((milliseconds % 86_400_000) / 3_600_000);
  if (days > 0) return tr('{days} days {hours} hours', { days, hours });
  const minutes = Math.max(1, Math.ceil(milliseconds / 60_000));
  return tr('{minutes} minutes', { minutes });
}
