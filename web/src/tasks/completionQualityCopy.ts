import { appLocale } from '../i18n';
import type { VerificationState, WorkOutcome } from '../types';

const copy = {
  en: {
    quality: 'Completion quality', outcome: 'Work outcome', verification: 'Verification', scope: 'Scope', blockers: 'Blockers', limitations: 'Limitations', evidence: 'Evidence', criteria: 'Acceptance criteria', covered: 'covered', uncovered: 'not covered', command: 'Command', exit: 'Exit', reason: 'Reason',
    outcomeCompleted: 'Completed', outcomePartial: 'Partial', outcomeBlocked: 'Blocked', passed: 'Passed', failed: 'Failed', notRun: 'Not run', notApplicable: 'Not applicable', stale: 'Stale', unknown: 'Unknown',
  },
  vi: {
    quality: 'Chất lượng hoàn tất', outcome: 'Kết quả công việc', verification: 'Xác minh', scope: 'Phạm vi', blockers: 'Trở ngại', limitations: 'Giới hạn', evidence: 'Bằng chứng', criteria: 'Tiêu chí chấp nhận', covered: 'đã bao phủ', uncovered: 'chưa bao phủ', command: 'Lệnh', exit: 'Mã thoát', reason: 'Lý do',
    outcomeCompleted: 'Hoàn tất', outcomePartial: 'Một phần', outcomeBlocked: 'Bị chặn', passed: 'Đạt', failed: 'Thất bại', notRun: 'Chưa chạy', notApplicable: 'Không áp dụng', stale: 'Đã cũ', unknown: 'Chưa xác định',
  },
} as const;

export function qualityCopy() { return appLocale().startsWith('vi') ? copy.vi : copy.en; }
export function outcomeLabel(value: WorkOutcome) { const labels = qualityCopy(); return value === 'partial' ? labels.outcomePartial : value === 'blocked' ? labels.outcomeBlocked : labels.outcomeCompleted; }
export function verificationLabel(value: VerificationState) { return qualityCopy()[value]; }
