import { useState } from 'react';
import { KeyRound, LoaderCircle, ShieldCheck } from 'lucide-react';
import { api } from '../api';
import { tr } from '../i18n';

export function SecuritySettings() {
  const [currentPassword, setCurrentPassword] = useState('');
  const [newPassword, setNewPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const submit = async () => {
    setMessage('');
    setError('');
    if (newPassword !== confirmPassword) {
      setError(tr('Password confirmation does not match.'));
      return;
    }
    setBusy(true);
    try {
      await api.changePassword(currentPassword, newPassword, confirmPassword);
      setCurrentPassword('');
      setNewPassword('');
      setConfirmPassword('');
      setMessage(tr('Password changed. Other signed-in browser sessions have been invalidated.'));
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Unable to change password.'));
    } finally {
      setBusy(false);
    }
  };

  return <div className="execution-settings">
    <div className="settings-intro"><span><ShieldCheck /></span><div><strong>{tr('GUI access security')}</strong><p>{tr('The management interface requires this password. MCP connection tokens are unchanged and continue to use their existing authentication.')}</p></div></div>
    <div className="settings-section-block">
      <div className="settings-section-heading"><div><strong><KeyRound />{tr('Change password')}</strong><p>{tr('Changing the password signs out every other GUI session. This browser receives a fresh session automatically.')}</p></div></div>
      <div className="settings-control-grid one-column">
        <label className="settings-field-card"><span className="settings-field-copy"><strong>{tr('Current password')}</strong><small>{tr('Required to confirm the change.')}</small></span><div className="settings-field-control"><input autoComplete="current-password" type="password" value={currentPassword} onChange={(event) => setCurrentPassword(event.target.value)} required /></div></label>
        <label className="settings-field-card"><span className="settings-field-copy"><strong>{tr('New password')}</strong><small>{tr('Use at least 8 characters.')}</small></span><div className="settings-field-control"><input autoComplete="new-password" minLength={8} maxLength={256} type="password" value={newPassword} onChange={(event) => setNewPassword(event.target.value)} required /></div></label>
        <label className="settings-field-card"><span className="settings-field-copy"><strong>{tr('Confirm new password')}</strong><small>{tr('Enter the new password again.')}</small></span><div className="settings-field-control"><input autoComplete="new-password" minLength={8} maxLength={256} type="password" value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} required /></div></label>
        {error && <div className="auth-error" role="alert">{error}</div>}
        {message && <div className="settings-workspace-status"><strong>{message}</strong></div>}
        <div><button className="button primary" disabled={busy || !currentPassword || !newPassword || !confirmPassword} type="button" onClick={() => void submit()}>{busy ? <LoaderCircle className="spin" /> : <KeyRound />}{busy ? tr('Changing…') : tr('Change password')}</button></div>
      </div>
    </div>
  </div>;
}
