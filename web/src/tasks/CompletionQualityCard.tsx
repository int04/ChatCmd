import { CheckCircle2, CircleAlert, CircleHelp, ShieldCheck } from 'lucide-react';
import type { CompletionQualityReport, VerificationState } from '../types';
import { outcomeLabel, qualityCopy, verificationLabel } from './completionQualityCopy';

export function CompletionQualityCard({ report }: { report: CompletionQualityReport }) {
  const labels = qualityCopy();
  return <section className={`turn-quality-report ${report.verification}`} aria-label={labels.quality}>
    <header>{verificationIcon(report.verification)}<strong>{labels.quality}</strong></header>
    <dl>
      <div><dt>{labels.outcome}</dt><dd>{outcomeLabel(report.workOutcome)}</dd></div>
      <div><dt>{labels.verification}</dt><dd>{verificationLabel(report.verification)}</dd></div>
      {report.verificationScope && <div><dt>{labels.scope}</dt><dd>{report.verificationScope}</dd></div>}
    </dl>
    {report.verificationReason && <p><strong>{labels.reason}:</strong> {report.verificationReason}</p>}
    {report.criteria.length > 0 && <details><summary>{labels.criteria} · {report.criteria.filter((item) => item.covered).length}/{report.criteria.length}</summary><ul>{report.criteria.map((item, index) => <li key={`${item.criterion}:${index}`}>{item.criterion} — {item.covered ? labels.covered : labels.uncovered}</li>)}</ul></details>}
    {report.evidence.length > 0 && <details><summary>{labels.evidence} · {report.evidence.length}</summary><ul>{report.evidence.map((item) => <li key={item.executionId}><code>{item.command?.executable || item.executionId}</code>{item.exitCode !== undefined && <> · {labels.exit}: {item.exitCode ?? '—'}</>}{item.cwd && <> · <code>{item.cwd}</code></>}{item.reason && <small>{labels.reason}: {item.reason}</small>}</li>)}</ul></details>}
    {report.blockers.length > 0 && <div role="status"><strong>{labels.blockers}</strong><ul>{report.blockers.map((item, index) => <li key={`${item}:${index}`}>{item}</li>)}</ul></div>}
    {report.limitations.length > 0 && <div><strong>{labels.limitations}</strong><ul>{report.limitations.map((item, index) => <li key={`${item}:${index}`}>{item}</li>)}</ul></div>}
  </section>;
}

function verificationIcon(state: VerificationState) {
  if (state === 'passed') return <CheckCircle2 aria-hidden="true" />;
  if (state === 'failed' || state === 'stale') return <CircleAlert aria-hidden="true" />;
  if (state === 'notRun' || state === 'notApplicable') return <CircleHelp aria-hidden="true" />;
  return <ShieldCheck aria-hidden="true" />;
}
