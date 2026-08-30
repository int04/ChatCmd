import { BadgePercent, Check, CheckCircle2, ChevronLeft, Crown, LoaderCircle, PackageCheck, Sparkles, WalletCards } from 'lucide-react';
import { useEffect, useState } from 'react';
import { api, ApiError, type DealCheckResult, type PlanPurchaseResult, type ServicePlan } from '../api';
import { Modal } from '../components';
import { billingTr, formatVnd } from './billingI18n';

type Props = {
  currentPlanType: number;
  onClose: () => void;
  onTopUp: () => void;
  onPurchased: (result: PlanPurchaseResult) => void | Promise<void>;
};

export function PlanPurchaseModal({ currentPlanType, onClose, onTopUp, onPurchased }: Props) {
  const [plans, setPlans] = useState<ServicePlan[]>([]);
  const [balance, setBalance] = useState(0);
  const [selected, setSelected] = useState<ServicePlan | null>(null);
  const [loading, setLoading] = useState(true);
  const [problem, setProblem] = useState('');
  const [dealCode, setDealCode] = useState('');
  const [dealResult, setDealResult] = useState<DealCheckResult | null>(null);
  const [dealProblem, setDealProblem] = useState('');
  const [checkingDeal, setCheckingDeal] = useState(false);
  const [purchasing, setPurchasing] = useState(false);

  useEffect(() => {
    let active = true;
    void Promise.all([api.servicePlans(), api.billingBalance()])
      .then(([nextPlans, nextBalance]) => {
        if (!active) return;
        setPlans(nextPlans);
        setBalance(nextBalance.vnd);
      })
      .catch(() => { if (active) setProblem(billingTr('Could not load service plans.')); })
      .finally(() => { if (active) setLoading(false); });
    return () => { active = false; };
  }, []);

  const normalizedDeal = dealCode.trim().toUpperCase();
  const validPreview = dealResult && dealResult.code.trim().toUpperCase() === normalizedDeal ? dealResult : null;
  const finalPrice = selected ? validPreview?.finalPrice ?? selected.price : 0;
  const needsDealCheck = Boolean(normalizedDeal) && !validPreview;
  const insufficient = Boolean(selected && balance < finalPrice);

  const selectPlan = (plan: ServicePlan) => {
    if (plan.price <= 0 || plan.type < currentPlanType) return;
    setSelected(plan);
    setDealCode('');
    setDealResult(null);
    setDealProblem('');
    setProblem('');
  };

  const checkDeal = async () => {
    if (!selected) return;
    if (!normalizedDeal) {
      setDealResult(null);
      setDealProblem(billingTr('Deal code is required.'));
      return;
    }
    setCheckingDeal(true);
    setDealProblem('');
    try {
      setDealResult(await api.checkDeal(normalizedDeal, selected.id));
    } catch (error) {
      setDealResult(null);
      setDealProblem(dealErrorMessage(error));
    } finally {
      setCheckingDeal(false);
    }
  };

  const purchase = async () => {
    if (!selected || needsDealCheck || insufficient) return;
    setPurchasing(true);
    setProblem('');
    try {
      const result = await api.purchasePlan(selected.id, normalizedDeal || null);
      await onPurchased(result);
    } catch (error) {
      if (error instanceof ApiError && error.problem?.code === 'insufficient_balance') {
        try { setBalance((await api.billingBalance()).vnd); } catch { /* keep last balance */ }
      }
      setProblem(purchaseErrorMessage(error));
    } finally {
      setPurchasing(false);
    }
  };

  const openTopUp = () => {
    onClose();
    onTopUp();
  };

  return <Modal title={selected ? billingTr('Plan payment') : billingTr('Service plans')} description={selected ? selected.name : billingTr('Choose the plan you want to buy or renew.')} close={onClose} className="billing-modal plan-purchase-modal">
    {loading ? <div className="billing-loading"><LoaderCircle className="spin" />{billingTr('Loading service plans...')}</div> : selected ? <div className="plan-checkout">
      <button type="button" className="billing-back" disabled={purchasing} onClick={() => setSelected(null)}><ChevronLeft />{billingTr('Back to plans')}</button>
      <div className="plan-checkout-summary">
        <div><span>{billingTr('Current balance')}</span><strong>{formatVnd(balance)}</strong></div>
        <div><span>{billingTr('Original price')}</span><strong>{formatVnd(selected.price)}</strong></div>
        <div><span>{billingTr('Discount')}</span><strong>{formatVnd(validPreview?.discountAmount ?? 0)}</strong></div>
        <div className="total"><span>{billingTr('Final price')}</span><strong>{formatVnd(finalPrice)}</strong></div>
      </div>
      <div className="deal-check-box">
        <label className="billing-field"><span>{billingTr('Discount code')}</span><div className="deal-input-row"><input value={dealCode} maxLength={200} placeholder={billingTr('Enter discount code')} disabled={purchasing} onChange={(event) => { setDealCode(event.target.value); setDealResult(null); setDealProblem(''); }} /><button type="button" className="button secondary" disabled={checkingDeal || purchasing || !normalizedDeal} onClick={() => void checkDeal()}>{checkingDeal ? <LoaderCircle className="spin" /> : <BadgePercent />}{checkingDeal ? billingTr('Checking...') : billingTr('Check code')}</button></div></label>
        {validPreview && <div className="deal-success"><CheckCircle2 />{billingTr('Discount code applied: {value}% off.', { value: validPreview.value })}</div>}
        {dealProblem && <div className="billing-error" role="alert">{dealProblem}</div>}
        {needsDealCheck && !dealProblem && <small>{billingTr('Enter and check the discount code before purchasing.')}</small>}
      </div>
      {insufficient && <div className="billing-warning"><WalletCards />{billingTr('Not enough balance for this plan.')}</div>}
      {problem && <div className="billing-error" role="alert">{problem}</div>}
      <div className="billing-actions">
        {insufficient ? <button type="button" className="button primary" onClick={openTopUp}><WalletCards />{billingTr('Top up')}</button> : <button type="button" className="button primary" disabled={purchasing || needsDealCheck} onClick={() => void purchase()}>{purchasing ? <LoaderCircle className="spin" /> : <PackageCheck />}{purchasing ? billingTr('Purchasing...') : billingTr('Purchase plan')}</button>}
      </div>
    </div> : <>
      {problem && <div className="billing-error" role="alert">{problem}</div>}
      <div className="plan-list plan-selection-grid">
        {plans.map((plan) => {
          const unavailable = plan.type < currentPlanType;
          const current = plan.type === currentPlanType;
          const free = plan.type === 0 || plan.price <= 0;
          const recommended = plan.type === 1;
          const premium = plan.type >= 2;
          const features = planFeatures(plan.type);
          const buttonLabel = free ? billingTr('Included plan') : current ? billingTr('Renew plan') : billingTr('Upgrade plan');
          return <article key={plan.id} className={`plan-card plan-tier-${plan.type} ${unavailable ? 'unavailable' : ''} ${current ? 'current' : ''} ${recommended && !current ? 'recommended' : ''}`}>
            <div className="plan-card-topline">
              <div className="plan-card-icon">{premium ? <Crown /> : recommended ? <Sparkles /> : <PackageCheck />}</div>
              <div className="plan-card-badges">
                {current && <span className="plan-badge current">{billingTr('Current plan')}</span>}
                {!current && recommended && <span className="plan-badge recommended">{billingTr('Recommended')}</span>}
                {!current && premium && <span className="plan-badge premium">{billingTr('Most features')}</span>}
              </div>
            </div>
            <div className="plan-card-copy">
              <div className="plan-card-title-row"><strong>{plan.name}</strong><span>{free ? billingTr('Free') : formatVnd(plan.price)}</span></div>
              <p>{planDescription(plan.type)}</p>
            </div>
            <div className="plan-feature-list">
              {features.map((feature) => <div key={feature}><Check /><span>{feature}</span></div>)}
            </div>
            <div className="plan-card-footer">
              <span>{free ? billingTr('Included plan') : billingTr('{days} days', { days: plan.days })}</span>
              <button type="button" className={`button ${recommended || premium ? 'primary' : 'secondary'}`} disabled={free || unavailable} onClick={() => selectPlan(plan)}>{unavailable ? billingTr('Unavailable') : buttonLabel}</button>
            </div>
          </article>;
        })}
      </div>
    </>}
  </Modal>;
}

function planDescription(type: number) {
  if (type === 0) return billingTr('Free plan description');
  if (type === 1) return billingTr('Normal plan description');
  return billingTr('VIP plan description');
}

function planFeatures(type: number) {
  if (type === 0) return [billingTr('Up to 5 hours of use per day')];
  if (type === 1) return [billingTr('Unlimited usage time')];
  return [
    billingTr('Unlimited usage time'),
    billingTr('Chat directly in the system'),
    billingTr('Expanded features'),
    billingTr('Early access to new features'),
  ];
}

function dealErrorMessage(error: unknown) {
  if (!(error instanceof ApiError)) return error instanceof Error ? error.message : billingTr('Purchase failed.');
  switch (error.problem?.code) {
    case 'deal_required': return billingTr('Deal code is required.');
    case 'plan_not_found': return billingTr('Plan does not exist.');
    case 'deal_not_found': return billingTr('Deal code does not exist.');
    case 'deal_exhausted': return billingTr('Deal code has no remaining uses.');
    case 'deal_invalid': return billingTr('Deal configuration is invalid.');
    case 'deal_plan_not_allowed': return billingTr('Deal code does not apply to this plan.');
    default: return error.message;
  }
}

function purchaseErrorMessage(error: unknown) {
  if (!(error instanceof ApiError)) return error instanceof Error ? error.message : billingTr('Purchase failed.');
  switch (error.problem?.code) {
    case 'user_not_found': return billingTr('Your account could not be found. Please sign in again.');
    case 'plan_not_found': return billingTr('Plan does not exist.');
    case 'plan_not_purchasable': return billingTr('This plan cannot be purchased.');
    case 'plan_lower_than_current': return billingTr('You cannot buy a lower plan than your current plan.');
    case 'deal_not_found': return billingTr('Deal code does not exist.');
    case 'deal_exhausted': return billingTr('Deal code has no remaining uses.');
    case 'deal_invalid': return billingTr('Deal configuration is invalid.');
    case 'deal_plan_not_allowed': return billingTr('Deal code does not apply to this plan.');
    case 'insufficient_balance': return billingTr('Not enough balance for this plan.');
    default: return error.message || billingTr('Purchase failed.');
  }
}
