import { CheckCircle2, CircleAlert, CircleHelp, ShieldCheck } from 'lucide-react';
import type { CompletionQualityReport, VerificationState } from '../types';
import { outcomeLabel, qualityCopy, verificationLabel } from './completionQualityCopy';

export function CompletionQualityCard({ report }: { report: CompletionQualityReport }) {
  const labels = qualityCopy();
  const coveredCriteria = report.criteria.filter((item) => item.covered).length;
  return <section className={`turn-quality-report ${report.verification}`} aria-label={labels.quality}>
    <header className="turn-quality-header">
      <div><span className="turn-quality-kicker">{labels.quality}</span><h3>{outcomeLabel(report.workOutcome)}</h3></div>
      <span className="turn-quality-mark">{verificationIcon(report.verification)}</span>
    </header>
    <div className="turn-quality-summary">
      <span><small>{labels.verification}</small><strong>{verificationLabel(report.verification)}</strong></span>
      <span><small>{labels.criteria}</small><strong>{coveredCriteria}/{report.criteria.length}</strong></span>
      <span><small>{labels.evidence}</small><strong>{report.evidence.length}</strong></span>
    </div>
    {(report.verificationScope || report.verificationReason) && <div className="turn-quality-narrative">
      {report.verificationScope && <section><span>{labels.scope}</span><p>{report.verificationScope}</p></section>}
      {report.verificationReason && <section><span>{labels.reason}</span><p>{report.verificationReason}</p></section>}
    </div>}
    {report.criteria.length > 0 && <section className="turn-quality-section"><header><strong>{labels.criteria}</strong><span>{coveredCriteria}/{report.criteria.length}</span></header><ul className="turn-quality-checklist">{report.criteria.map((item, index) => <li className={item.covered ? 'covered' : 'uncovered'} key={`${item.criterion}:${index}`}><span>{item.covered ? <CheckCircle2 /> : <CircleAlert />}</span><p>{item.criterion}</p><small>{item.covered ? labels.covered : labels.uncovered}</small></li>)}</ul></section>}
    {report.evidence.length > 0 && <section className="turn-quality-section"><header><strong>{labels.evidence}</strong><span>{report.evidence.length}</span></header><ul className="turn-quality-evidence">{report.evidence.map((item) => <li key={item.executionId}><div><code>{item.command?.executable || item.executionId}</code>{item.exitCode !== undefined && <span>{labels.exit}: {item.exitCode ?? '—'}</span>}</div>{item.cwd && <code>{item.cwd}</code>}{item.reason && <small>{labels.reason}: {item.reason}</small>}</li>)}</ul></section>}
    {report.blockers.length > 0 && <QualityNotice title={labels.blockers} tone="danger" items={report.blockers} />}
    {report.limitations.length > 0 && <QualityNotice title={labels.limitations} tone="muted" items={report.limitations} />}
  </section>;
}

function QualityNotice({ title, tone, items }: { title: string; tone: 'danger' | 'muted'; items: string[] }) {
  return <section className={`turn-quality-notice ${tone}`} role={tone === 'danger' ? 'status' : undefined}><header><strong>{title}</strong></header><ul>{items.map((item, index) => <li key={`${item}:${index}`}>{item}</li>)}</ul></section>;
}

function verificationIcon(state: VerificationState) {
  if (state === 'passed') return <CheckCircle2 aria-hidden="true" />;
  if (state === 'failed' || state === 'stale') return <CircleAlert aria-hidden="true" />;
  if (state === 'notRun' || state === 'notApplicable') return <CircleHelp aria-hidden="true" />;
  return <ShieldCheck aria-hidden="true" />;
}
