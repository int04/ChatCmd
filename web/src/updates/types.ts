export type UpdatePhase =
  | 'idle'
  | 'checking'
  | 'available'
  | 'upToDate'
  | 'downloading'
  | 'verifying'
  | 'extracting'
  | 'preparing'
  | 'readyToRestart'
  | 'restarting'
  | 'failed'
  | 'unsupported';

export interface UpdateStatus {
  currentVersion: string;
  latestVersion: string | null;
  releaseTag: string | null;
  releaseName: string | null;
  releaseUrl: string | null;
  note: string | null;
  platform: string;
  architecture: string;
  debugBuild: boolean;
  phase: UpdatePhase;
  updateAvailable: boolean;
  downloadAvailable: boolean;
  assetName: string | null;
  checksumVerified: boolean;
  progressPercent: number | null;
  downloadedBytes: number;
  totalBytes: number | null;
  message: string | null;
}

export const ACTIVE_UPDATE_PHASES: UpdatePhase[] = [
  'checking',
  'downloading',
  'verifying',
  'extracting',
  'preparing',
  'restarting',
];

export function isActiveUpdatePhase(phase: UpdatePhase) {
  return ACTIVE_UPDATE_PHASES.includes(phase);
}
