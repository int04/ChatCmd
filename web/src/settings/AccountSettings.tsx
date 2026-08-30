import { Gift, KeyRound, Orbit, PackageCheck, ShieldCheck, UserRound, WalletCards } from 'lucide-react';
import { useCallback, useMemo, useState } from 'react';
import { api, ApiError, type GiftCodeRedeemResult, type PlanPurchaseResult } from '../api';
import { useAuth } from '../auth';
import { tr } from '../i18n';
import { billingTr, formatVnd } from './billingI18n';
import { PlanPurchaseModal } from './PlanPurchaseModal';
import { TopUpModal } from './TopUpModal';

type AccountTab = 'info' | 'giftcode' | 'password';

export function AccountSettings() {
  const { user, refresh } = useAuth();
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
  const [challengePurpose, setChallengePurpose] = useState<'password' | 'giftcode' | null>(null);
  const [giftCode, setGiftCode] = useState('');
  const [giftRedeeming, setGiftRedeeming] = useState(false);
  const [giftProblem, setGiftProblem] = useState('');
  const [giftResult, setGiftResult] = useState<GiftCodeRedeemResult | null>(null);
  const [topUpOpen, setTopUpOpen] = useState(false);
  const [purchaseOpen, setPurchaseOpen] = useState(false);

  const planRemaining = useMemo(() => formatRemaining(user?.plan.expriAt), [user?.plan.expriAt]);
  const handleBalanceChanged = useCallback(async (balance: number) => {
    setTopUpOpen(false);
    await refresh();
    window.alert(billingTr('Top up successful, balance {balance}', { balance: `${new Intl.NumberFormat('vi-VN').format(balance)}đ` }));
  }, [refresh]);
  const handlePurchased = useCallback(async (result: PlanPurchaseResult) => {
    setPurchaseOpen(false);
    try { await api.billingBalance(); } catch { /* auth info still refreshes the account snapshot */ }
    await refresh();
    window.alert(billingTr(result.extended ? 'Plan renewed successfully.' : 'Plan purchased successfully.'));
  }, [refresh]);
  if (!user) return null;
  const isFree = user.plan.type === 0 || user.plan.name.toUpperCase() === 'FREE';

  const redeemGiftCode = async () => {
    setGiftProblem('');
    setGiftResult(null);
    setGiftRedeeming(true);
    try {
      const result = await api.redeemGiftCode(giftCode);
      setGiftResult(result);
      setGiftCode('');
      await refresh();
    } catch (error) {
      setGiftProblem(giftCodeErrorMessage(error));
    } finally {
      setGiftRedeeming(false);
      setChallengeOpen(false);
      setChallengePurpose(null);
    }
  };

  const openChallenge = (purpose: 'password' | 'giftcode') => {
    resetChallenge();
    setChallengePurpose(purpose);
    setChallengeOpen(true);
  };

  const requestGiftCodeRedeem = () => {
    setGiftProblem('');
    setGiftResult(null);
    if (!giftCode.trim()) return setGiftProblem(tr('Enter a gift code first.'));
    openChallenge('giftcode');
  };

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
    openChallenge('password');
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
      window.setTimeout(() => {
        if (challengePurpose === 'giftcode') void redeemGiftCode();
        else if (challengePurpose === 'password') void submitPasswordChange();
      }, 320);
    }
  };

  return <div className="account-settings">
    <div className="account-subtabs" role="tablist" aria-label={tr('Account sections')}>
      <button type="button" role="tab" aria-selected={activeTab === 'info'} className={activeTab === 'info' ? 'active' : ''} onClick={() => setActiveTab('info')}><UserRound />{tr('Information')}</button>
      <button type="button" role="tab" aria-selected={activeTab === 'giftcode'} className={activeTab === 'giftcode' ? 'active' : ''} onClick={() => setActiveTab('giftcode')}><Gift />{tr('Gift code')}</button>
      <button type="button" role="tab" aria-selected={activeTab === 'password'} className={activeTab === 'password' ? 'active' : ''} onClick={() => setActiveTab('password')}><KeyRound />{tr('Change password')}</button>
    </div>

    {activeTab === 'info' && <div className="account-info-grid" role="tabpanel">
      <AccountValue label="ID" value={String(user.id)} />
      <AccountValue label={tr('Email')} value={user.email} />
      <AccountValue label={tr('Remaining balance')} value={formatVnd(user.vnd)} action={<button type="button" className="account-value-action" onClick={() => setTopUpOpen(true)}><WalletCards />{billingTr('Top up')}</button>} />
      <AccountValue label={tr('Plan')} value={user.plan.name} action={<button type="button" className="account-value-action" onClick={() => setPurchaseOpen(true)}><PackageCheck />{billingTr('Buy service plan')}</button>} />
      {!isFree && user.plan.expriAt && <AccountValue label={tr('Plan time remaining')} value={planRemaining ?? formatDate(user.plan.expriAt)} hint={formatDate(user.plan.expriAt)} />}
      {isFree && <AccountValue label={tr('Use until')} value={formatDate(user.useNextTime)} />}
      {isFree && <AccountValue label={tr('Next reset')} value={formatDate(user.useNextReset)} />}
    </div>}

    {activeTab === 'giftcode' && <div className="account-giftcode-panel" role="tabpanel">
      <div className="account-giftcode-intro">
        <span><Gift /></span>
        <div><strong>{tr('Redeem a gift code')}</strong><small>{tr('Enter the code you received. Your plan will update automatically after a successful redemption.')}</small></div>
      </div>
      <div className="account-giftcode-form">
        <label htmlFor="account-giftcode-input">{tr('Gift code')}</label>
        <div className="account-giftcode-input-row">
          <input id="account-giftcode-input" value={giftCode} maxLength={200} autoComplete="off" spellCheck={false} placeholder={tr('Enter gift code')} onKeyDown={(event) => { if (event.key === 'Enter' && !giftRedeeming && giftCode.trim()) { event.preventDefault(); requestGiftCodeRedeem(); } }} onChange={(event) => { setGiftCode(event.target.value); setGiftProblem(''); setGiftResult(null); }} />
          <button type="button" className="button primary" disabled={giftRedeeming || !giftCode.trim()} onClick={requestGiftCodeRedeem}><Gift />{giftRedeeming ? tr('Redeeming...') : tr('Redeem')}</button>
        </div>
        <small>{tr('Gift codes are not case-sensitive. Spaces at the beginning or end are ignored.')}</small>
      </div>
      {giftProblem && <div className="account-giftcode-error" role="alert">{giftProblem}</div>}
      {giftResult && <div className="account-giftcode-success" role="status">
        <div className="account-giftcode-success-heading"><span><Gift /></span><div><strong>{tr('Gift code redeemed')}</strong><small>{tr('Your account plan has been refreshed.')}</small></div></div>
        <div className="account-giftcode-result-grid">
          <AccountValue label={tr('Plan received')} value={giftResult.planName} />
          <AccountValue label={tr('Added time')} value={tr('{days} days', { days: giftResult.days })} />
          <AccountValue label={tr('Remaining uses')} value={String(giftResult.remainingUses)} />
          <AccountValue label={tr('Expires at')} value={formatDate(giftResult.expiresAt)} />
        </div>
      </div>}
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

    {topUpOpen && <TopUpModal userId={user.id} currentBalance={user.vnd} onClose={() => setTopUpOpen(false)} onBalanceChanged={handleBalanceChanged} />}
    {purchaseOpen && <PlanPurchaseModal currentPlanType={user.plan.type} onClose={() => setPurchaseOpen(false)} onTopUp={() => setTopUpOpen(true)} onPurchased={handlePurchased} />}

    {challengeOpen && <div className="modal-backdrop account-constellation-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget && !submitting && !giftRedeeming) { setChallengeOpen(false); setChallengePurpose(null); } }}>
      <div className={`modal account-constellation-modal ${challengeVerified ? 'verified' : ''}`} role="dialog" aria-modal="true" aria-labelledby="security-constellation-title">
        <div className="account-constellation-heading">
          <span><Orbit /></span>
          <div><strong id="security-constellation-title">{tr('Security constellation')}</strong><small>{tr('Connect the stars in the shown order to unlock this action.')}</small></div>
          <button type="button" disabled={submitting || giftRedeeming} onClick={resetChallenge}>{tr('Shuffle')}</button>
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
          {challenge.nodes.map((node) => <button key={node.id} type="button" className={challengeProgress.includes(node.id) ? 'active' : ''} style={{ left: `${node.x / 3}%`, top: `${node.y / 3}%` }} disabled={submitting || giftRedeeming || challengeVerified || challengeProgress.includes(node.id)} onClick={() => pickAndContinue(node.id)} aria-label={node.id}><span>{node.id}</span></button>)}
        </div>
        <div className="account-constellation-status" role="status">{giftRedeeming ? tr('Redeeming...') : submitting ? tr('Changing password...') : challengeVerified ? tr('Security check complete. You can continue.') : challengeHint || tr('{count}/{total} stars connected', { count: challengeProgress.length, total: challenge.sequence.length })}</div>
        <div className="account-constellation-modal-actions"><button type="button" className="button secondary" disabled={submitting || giftRedeeming} onClick={() => { setChallengeOpen(false); setChallengePurpose(null); }}>{tr('Cancel')}</button></div>
      </div>
    </div>}
  </div>;
}

function giftCodeErrorMessage(error: unknown) {
  if (!(error instanceof ApiError)) return error instanceof Error ? error.message : tr('Gift code redemption failed.');
  switch (error.problem?.code) {
    case 'giftcode_required': return tr('Enter a gift code first.');
    case 'giftcode_too_long': return tr('Gift code is too long.');
    case 'giftcode_not_found': return tr('Gift code does not exist.');
    case 'giftcode_already_used': return tr('This gift code has already been used by your account.');
    case 'giftcode_exhausted': return tr('Gift code has no remaining uses.');
    case 'giftcode_plan_lower_than_current': return tr('This gift code is for a lower plan than your current plan.');
    case 'giftcode_invalid':
    case 'giftcode_invalid_days':
    case 'giftcode_plan_not_found': return tr('This gift code is temporarily unavailable. Please contact support.');
    case 'user_not_found': return tr('Your account could not be found. Please sign in again.');
    default: return error.message || tr('Gift code redemption failed.');
  }
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

function AccountValue({ label, value, hint, action }: { label: string; value: string; hint?: string; action?: React.ReactNode }) {
  return <div className="account-value"><span>{label}</span><div className="account-value-main"><strong>{value || '—'}</strong>{action}</div>{hint && <small>{hint}</small>}</div>;
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
