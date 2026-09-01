import { Banknote, Building2, ExternalLink, LoaderCircle } from 'lucide-react';
import { useMemo, useState } from 'react';
import { ApiError, api } from '../api';
import { Modal } from '../components';
import { billingTr, formatVnd } from './billingI18n';

const QUICK_AMOUNTS = [10_000, 50_000, 100_000, 200_000, 500_000];
const MAX_PAYMENT_AMOUNT = 300_000_000;

type Props = {
  userId: number;
  currentBalance: number;
  onClose: () => void;
  onBalanceChanged: (balance: number) => void | Promise<void>;
};

function paymentErrorMessage(error: unknown) {
  if (!(error instanceof ApiError)) return billingTr('Could not create payment. Please try again.');
  switch (error.problem?.code) {
    case 'INVALID_AMOUNT': return billingTr('Amount must be between 1 and 300000000 VND.');
    case 'INVALID_CONTENT': return billingTr('Payment content is invalid.');
    case 'PAYMENT_NOT_CONFIGURED': return billingTr('Payment service is not configured.');
    case 'PAYMENT_PROVIDER_REJECTED': return billingTr('Payment provider rejected the request.');
    case 'PAYMENT_PROVIDER_UNAVAILABLE': return billingTr('Payment service is temporarily unavailable. Please try again.');
    case 'PAYMENT_PERSISTENCE_ERROR': return billingTr('Payment could not be saved. Please try again.');
    case 'PAYMENT_INTERNAL_ERROR': return billingTr('Payment service encountered an internal error.');
    default: return billingTr('Could not create payment. Please try again.');
  }
}

export function TopUpModal({ userId, onClose }: Props) {
  const [amountText, setAmountText] = useState('');
  const [preparing, setPreparing] = useState(false);
  const [problem, setProblem] = useState('');
  const transferContent = `CHATCMD${userId}`;

  const amount = useMemo(() => Number(amountText.replace(/\D/g, '')) || 0, [amountText]);

  const startPayment = async () => {
    if (!Number.isSafeInteger(amount) || amount <= 0 || amount > MAX_PAYMENT_AMOUNT) {
      setProblem(billingTr('Amount must be between 1 and 300000000 VND.'));
      return;
    }

    setProblem('');
    setPreparing(true);
    try {
      const payment = await api.createPayment(amount, transferContent);
      const payUrl = payment.data?.payUrl?.trim();
      if (!payUrl) throw new Error('payment payUrl is missing');

      const opened = window.open(payUrl, '_blank', 'noopener,noreferrer');
      if (!opened) {
        setProblem(billingTr('Could not open payment page. Please allow pop-ups and try again.'));
      }
    } catch (error) {
      setProblem(paymentErrorMessage(error));
    } finally {
      setPreparing(false);
    }
  };

  return <Modal title={billingTr('Top up balance')} description={billingTr('Choose a payment method and follow the instructions below.')} close={onClose} className="billing-modal">
    <div className="billing-modal-grid">
      <aside className="billing-tabs" role="tablist" aria-label={billingTr('Top up balance')}>
        <button type="button" className="active" role="tab" aria-selected="true"><Building2 />{billingTr('Bank transfer')}</button>
      </aside>
      <section className="billing-panel" role="tabpanel">
        <div className="billing-section-heading"><Banknote /><div><strong>{billingTr('Choose amount')}</strong><span>{billingTr('Select a quick amount or enter another amount.')}</span></div></div>
        <div className="topup-quick-grid">
          {QUICK_AMOUNTS.map((value) => <button key={value} type="button" className={amount === value ? 'active' : ''} onClick={() => { setAmountText(String(value)); setProblem(''); }}>{formatVnd(value)}</button>)}
        </div>
        <label className="billing-field"><span>{billingTr('Custom amount')}</span><input inputMode="numeric" value={amountText} placeholder={billingTr('Enter amount')} onChange={(event) => { setAmountText(event.target.value.replace(/\D/g, '')); setProblem(''); }} /></label>
        {problem && <div className="billing-error" role="alert">{problem}</div>}
        <div className="billing-actions"><button type="button" className="button primary" disabled={preparing || amount <= 0} onClick={() => void startPayment()}>{preparing ? <LoaderCircle className="spin" /> : <ExternalLink />}{preparing ? billingTr('Preparing payment information...') : billingTr('Continue')}</button></div>
      </section>
    </div>
  </Modal>;
}
