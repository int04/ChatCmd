import { KeyRound, ShieldCheck, UserRound } from 'lucide-react';
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

  const planRemaining = useMemo(() => formatRemaining(user?.plan.expriAt), [user?.plan.expriAt]);
  if (!user) return null;
  const isFree = user.plan.type === 0 || user.plan.name.toUpperCase() === 'FREE';

  const changePassword = async () => {
    setProblem('');
    if (!currentPassword) return setProblem(tr('Current password is required.'));
    if (newPassword.length < 8) return setProblem(tr('New password must contain at least 8 characters.'));
    if (newPassword !== confirmPassword) return setProblem(tr('Password confirmation does not match.'));
    if (currentPassword === newPassword) return setProblem(tr('New password must be different from current password.'));
    setSubmitting(true);
    try {
      await api.changePassword(currentPassword, newPassword);
      window.dispatchEvent(new Event('chatcmd:auth-required'));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : tr('Password change failed.'));
    } finally {
      setSubmitting(false);
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
      <div className="account-password-actions"><button type="button" className="button primary" disabled={submitting} onClick={() => void changePassword()}><KeyRound />{submitting ? tr('Changing password...') : tr('Change password')}</button></div>
    </div>}
  </div>;
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
