import type { CompletionQualityReport, TimelineEvent, VerificationState, WorkOutcome } from '../types';

const verificationStates = new Set<VerificationState>(['passed', 'failed', 'notRun', 'notApplicable', 'stale', 'unknown']);
const workOutcomes = new Set<WorkOutcome>(['completed', 'partial', 'blocked']);

export function completionQualityReport(events: TimelineEvent[]): CompletionQualityReport | null {
  for (let index = events.length - 1; index >= 0; index--) {
    const payload = object(events[index].payload);
    const report = object(payload.qualityReport);
    if (events[index].type !== 'status' || !Object.keys(report).length) continue;
    return parseReport(report);
  }
  return null;
}

function parseReport(value: Record<string, unknown>): CompletionQualityReport {
  const verification = verificationStates.has(value.verification as VerificationState) ? value.verification as VerificationState : 'unknown';
  const workOutcome = workOutcomes.has(value.workOutcome as WorkOutcome) ? value.workOutcome as WorkOutcome : 'completed';
  return {
    schemaVersion: number(value.schemaVersion) ?? 1,
    lifecycle: 'completed',
    workOutcome,
    workOutcomeProvenance: text(value.workOutcomeProvenance) || 'legacyDefault',
    verification,
    verificationProvenance: text(value.verificationProvenance) || 'unknown',
    verificationReason: optionalText(value.verificationReason),
    verificationScope: optionalText(value.verificationScope),
    criteria: array(value.criteria).slice(0, 64).map((item) => {
      const criterion = object(item);
      return { criterion: text(criterion.criterion), evidenceRefs: stringArray(criterion.evidenceRefs), covered: criterion.covered === true };
    }),
    evidence: array(value.evidence).slice(0, 64).map((item) => {
      const evidence = object(item);
      const status = verificationStates.has(evidence.status as VerificationState) ? evidence.status as VerificationState : 'unknown';
      const command = object(evidence.command);
      return {
        executionId: text(evidence.executionId), taskId: optionalText(evidence.taskId), turnId: optionalText(evidence.turnId),
        cwd: optionalText(evidence.cwd), command: Object.keys(command).length ? { executable: optionalText(command.executable), argumentCount: number(command.argumentCount), argumentsSha256: optionalText(command.argumentsSha256) } : undefined,
        startedAtMs: number(evidence.startedAtMs), finishedAtMs: number(evidence.finishedAtMs), terminalState: optionalText(evidence.terminalState),
        exitCode: evidence.exitCode === null ? null : number(evidence.exitCode), timedOut: evidence.timedOut === true, cancelled: evidence.cancelled === true,
        artifactRef: evidence.artifactRef === null ? null : optionalText(evidence.artifactRef), status, reason: optionalText(evidence.reason),
      };
    }).filter((item) => item.executionId),
    diagnostics: array(value.diagnostics).slice(0, 64).map((item) => { const diagnostic = object(item); return { code: text(diagnostic.code) || 'unknown', evidenceRef: optionalText(diagnostic.evidenceRef), message: optionalText(diagnostic.message) }; }),
    blockers: stringArray(value.blockers),
    limitations: stringArray(value.limitations),
  };
}

function object(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
function array(value: unknown): unknown[] { return Array.isArray(value) ? value : []; }
function text(value: unknown): string { return typeof value === 'string' ? value.trim().slice(0, 2_000) : ''; }
function optionalText(value: unknown): string | undefined { return text(value) || undefined; }
function stringArray(value: unknown): string[] { return array(value).slice(0, 64).map(text).filter(Boolean); }
function number(value: unknown): number | undefined { return typeof value === 'number' && Number.isFinite(value) ? value : undefined; }
