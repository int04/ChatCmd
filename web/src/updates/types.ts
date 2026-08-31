export type UpdatePhase =
  | 'idle'
  | 'checking'
  | 'available'
  | 'upToDate'
  | 'downloading'
  | 'extracting'
  | 'preparing'
  | 'readyToRestart'
  | 'restarting'
  | 'failed'
  | 'unsupported';

export interface UpdateStatus {
  currentVersion: string;
  latestVersion: string | null;
  note: string | null;
  platform: string;
  architecture: string;
  phase: UpdatePhase;
  updateAvailable: boolean;
  downloadAvailable: boolean;
  progressPercent: number | null;
  downloadedBytes: number;
  totalBytes: number | null;
  message: string | null;
}

export const ACTIVE_UPDATE_PHASES: UpdatePhase[] = ['checking', 'downloading', 'extracting', 'preparing', 'restarting'];

export function isActiveUpdatePhase(phase: UpdatePhase) {
  return ACTIVE_UPDATE_PHASES.includes(phase);
}
