import { FormEvent, ReactNode, useCallback, useEffect, useState } from 'react';
import { Eye, EyeOff, KeyRound, LoaderCircle, LockKeyhole } from 'lucide-react';
import { api } from '../api';
import { tr } from '../i18n';
import './auth.css';

type Mode = 'loading' | 'setup' | 'login' | 'authenticated';

export function AuthGate({ children }: { children: ReactNode }) {
  const [mode, setMode] = useState<Mode>('loading');
  const [password, setPassword] = useState('');
  const [confirmPassword, setConfirmPassword] = useState('');
  const [showPassword, setShowPassword] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const refresh = useCallback(async () => {
    setError('');
    try {
      const status = await api.authStatus();
      setMode(status.authenticated ? 'authenticated' : status.configured ? 'login' : 'setup');
    } catch (reason) {
      setMode('login');
      setError(reason instanceof Error ? reason.message : tr('Unable to check authentication status.'));
    }
  }, []);

  useEffect(() => { void refresh(); }, [refresh]);
  useEffect(() => {
    const requireAuth = () => {
      setPassword('');
      setConfirmPassword('');
      setShowPassword(false);
      setMode('login');
    };
    window.addEventListener('chatcmd-auth-required', requireAuth);
    return () => window.removeEventListener('chatcmd-auth-required', requireAuth);
  }, []);

  if (mode === 'loading') {
    return <div className="auth-screen"><div className="auth-loading"><LoaderCircle className="spin" /></div></div>;
  }
  if (mode === 'authenticated') return <>{children}</>;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    if (mode === 'setup' && password !== confirmPassword) {
      setError(tr('Password confirmation does not match.'));
      return;
    }
    setBusy(true);
    try {
      if (mode === 'setup') await api.setupAuth(password, confirmPassword);
      else await api.login(password);
      setPassword('');
      setConfirmPassword('');
      setShowPassword(false);
      setMode('authenticated');
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : tr('Authentication failed.'));
    } finally {
      setBusy(false);
    }
  };

  const settingUp = mode === 'setup';
  const passwordType = showPassword ? 'text' : 'password';

  return <div className="auth-screen">
    <form className="auth-card" onSubmit={(event) => void submit(event)}>
      <div className="auth-mark">{settingUp ? <KeyRound /> : <LockKeyhole />}</div>
      <div className="auth-title">
        <span className="auth-product">ChatCMD</span>
        <h1>{settingUp ? tr('Create a password') : tr('Sign in')}</h1>
        <p>{settingUp ? tr('Protect this dashboard.') : tr('Enter your password to continue.')}</p>
      </div>

      <div className="auth-fields">
        <label className="auth-field">
          <span>{tr('Password')}</span>
          <div className="auth-input-wrap">
            <input autoFocus autoComplete={settingUp ? 'new-password' : 'current-password'} minLength={settingUp ? 8 : undefined} maxLength={256} type={passwordType} value={password} onChange={(event) => setPassword(event.target.value)} required />
            <button type="button" className="auth-eye" aria-label={showPassword ? tr('Hide password') : tr('Show password')} onClick={() => setShowPassword((current) => !current)}>{showPassword ? <EyeOff /> : <Eye />}</button>
          </div>
        </label>
        {settingUp && <label className="auth-field">
          <span>{tr('Confirm password')}</span>
          <div className="auth-input-wrap"><input autoComplete="new-password" minLength={8} maxLength={256} type={passwordType} value={confirmPassword} onChange={(event) => setConfirmPassword(event.target.value)} required /></div>
        </label>}
      </div>

      {error && <div className="auth-error" role="alert">{error}</div>}
      <button className="auth-submit" disabled={busy} type="submit">{busy && <LoaderCircle className="spin" />}{busy ? tr('Please wait…') : settingUp ? tr('Continue') : tr('Sign in')}</button>
      {settingUp && <span className="auth-footnote">{tr('Minimum 8 characters')}</span>}
    </form>
  </div>;
}
