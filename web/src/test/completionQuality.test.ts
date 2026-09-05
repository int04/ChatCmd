import { describe, expect, it } from 'vitest';
import type { TimelineEvent, VerificationState } from '../types';
import { completionQualityReport } from '../tasks/completionQuality';

function event(verification: VerificationState, extra: Record<string, unknown> = {}): TimelineEvent {
  return { id: verification, type: 'status', occurredAt: '2026-01-01T00:00:00Z', payload: { status: 'quality', qualityReport: { schemaVersion: 1, lifecycle: 'completed', workOutcome: 'partial', verification, verificationProvenance: 'server', criteria: [], evidence: [], diagnostics: [], blockers: [], limitations: [], ...extra } } };
}

describe('completion quality report', () => {
  it.each<VerificationState>(['passed', 'failed', 'notRun', 'notApplicable', 'stale', 'unknown'])('keeps server verification state %s separate from lifecycle', (verification) => {
    const report = completionQualityReport([event(verification)]);
    expect(report?.lifecycle).toBe('completed');
    expect(report?.workOutcome).toBe('partial');
    expect(report?.verification).toBe(verification);
  });

  it('bounds and sanitizes malformed persisted values', () => {
    const report = completionQualityReport([event('unknown', { verification: '<script>', blockers: ["blocked", 4], evidence: [{ executionId: 'run-1', status: '<b>pass</b>', command: { executable: '<img>' } }] })]);
    expect(report?.verification).toBe('unknown');
    expect(report?.blockers).toEqual(['blocked']);
    expect(report?.evidence[0].status).toBe('unknown');
    expect(report?.evidence[0].command?.executable).toBe('<img>');
  });
});
