import { Banknote, Building2, LoaderCircle, QrCode } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import { api } from '../api';
import { Modal } from '../components';
import { billingTr, formatVnd } from './billingI18n';

const QUICK_AMOUNTS = [10_000, 50_000, 100_000, 200_000, 500_000];
const POLL_MS = 2500;

type Props = {
  userId: number;
  currentBalance: number;
  onClose: () => void;
  onBalanceChanged: (balance: number) => void | Promise<void>;
};

export function TopUpModal({ userId, currentBalance, onClose, onBalanceChanged }: Props) {
  const [amountText, setAmountText] = useState('');
  const [paymentAmount, setPaymentAmount] = useState<number | null>(null);
  const [baselineBalance, setBaselineBalance] = useState(currentBalance);
  const [preparing, setPreparing] = useState(false);
  const [problem, setProblem] = useState('');
  const transferContent = `CHATCMD${userId}`;

  const amount = useMemo(() => Number(amountText.replace(/\D/g, '')) || 0, [amountText]);
  const qrUrl = paymentAmount ? `https://img.vietqr.io/image/MB-0987118554-compact2.png?amount=${paymentAmount}&accountName=PHAN%20VAN%20TUNG&addInfo=${encodeURIComponent(transferContent)}` : '';

  useEffect(() => {
    if (paymentAmount === null) return;
    let active = true;
    let checking = false;
    const check = async () => {
      if (checking) return;
      checking = true;
      try {
        const next = await api.billingBalance();
        if (active && next.vnd !== baselineBalance) {
          active = false;
          await onBalanceChanged(next.vnd);
        }
      } catch {
        if (active) setProblem(billingTr('Could not check current balance.'));
      } finally {
        checking = false;
      }
    };
    void check();
    const timer = window.setInterval(() => void check(), POLL_MS);
    return () => { active = false; window.clearInterval(timer); };
  }, [baselineBalance, onBalanceChanged, paymentAmount]);

  const startPayment = async () => {
    if (!Number.isSafeInteger(amount) || amount <= 0) {
      setProblem(billingTr('Amount must be greater than 0.'));
      return;
    }
    setProblem('');
    setPreparing(true);
    try {
      const balance = await api.billingBalance();
      setBaselineBalance(balance.vnd);
      setPaymentAmount(amount);
    } catch {
      setProblem(billingTr('Could not check current balance.'));
    } finally {
      setPreparing(false);
    }
  };

  const cancelPayment = () => {
    setPaymentAmount(null);
    setProblem('');
  };

  return <Modal title={billingTr('Top up balance')} description={billingTr('Choose a payment method and follow the instructions below.')} close={onClose} className="billing-modal">
    <div className="billing-modal-grid">
      <aside className="billing-tabs" role="tablist" aria-label={billingTr('Top up balance')}>
        <button type="button" className="active" role="tab" aria-selected="true"><Building2 />{billingTr('Bank transfer')}</button>
      </aside>
      <section className="billing-panel" role="tabpanel">
        {paymentAmount === null ? <>
          <div className="billing-section-heading"><Banknote /><div><strong>{billingTr('Choose amount')}</strong><span>{billingTr('Select a quick amount or enter another amount.')}</span></div></div>
          <div className="topup-quick-grid">
            {QUICK_AMOUNTS.map((value) => <button key={value} type="button" className={amount === value ? 'active' : ''} onClick={() => { setAmountText(String(value)); setProblem(''); }}>{formatVnd(value)}</button>)}
          </div>
          <label className="billing-field"><span>{billingTr('Custom amount')}</span><input inputMode="numeric" value={amountText} placeholder={billingTr('Enter amount')} onChange={(event) => { setAmountText(event.target.value.replace(/\D/g, '')); setProblem(''); }} /></label>
          {problem && <div className="billing-error" role="alert">{problem}</div>}
          <div className="billing-actions"><button type="button" className="button primary" disabled={preparing || amount <= 0} onClick={() => void startPayment()}>{preparing ? <LoaderCircle className="spin" /> : <QrCode />}{preparing ? billingTr('Preparing payment information...') : billingTr('Continue')}</button></div>
        </> : <>
          <div className="billing-section-heading"><QrCode /><div><strong>{billingTr('Scan QR to transfer')}</strong><span>{billingTr('Waiting for bank transfer...')}</span></div></div>
          <div className="topup-payment-layout">
            <img className="topup-qr" src={qrUrl} alt={billingTr('Scan QR to transfer')} width="320" height="320" />
            <dl className="topup-transfer-details">
              <div><dt>{billingTr('Account')}</dt><dd>0987118554 - NH. MBbank - CTK: PHAN VAN TUNG</dd></div>
              <div><dt>{billingTr('Amount')}</dt><dd>{formatVnd(paymentAmount)}</dd></div>
              <div><dt>{billingTr('Transfer content')}</dt><dd><code>{transferContent}</code></dd></div>
            </dl>
          </div>
          <div className="topup-polling"><LoaderCircle className="spin" /><span>{billingTr('Balance is checked automatically every few seconds.')}</span></div>
          {problem && <div className="billing-error" role="alert">{problem}</div>}
          <div className="billing-actions"><button type="button" className="button secondary" onClick={cancelPayment}>{billingTr('Cancel transaction')}</button></div>
        </>}
      </section>
    </div>
  </Modal>;
}
