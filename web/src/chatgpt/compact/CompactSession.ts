import { api, ApiError } from '../../api';
import { resumeChatGptCompact } from '../../chatgptBridge';
import { compactText } from './copy';
import { isCompactTerminal, newestCompactFirst, type CompactJob } from './types';

export interface CompactSnapshot {
  active: CompactJob | null;
  history: CompactJob[];
  ready: boolean;
  busy: boolean;
  waking: boolean;
  error: string;
  extensionMessage: string;
}

/** One task-scoped store owns polling; the composer and sidebar share its snapshot. */
export class CompactSession {
  private snapshot: CompactSnapshot = { active: null, history: [], ready: false, busy: false, waking: false, error: '', extensionMessage: '' };
  private listeners = new Set<() => void>();
  private jobs = new Map<string, CompactJob>();
  private running = false;
  private generation = 0;
  private timer: number | undefined;
  private reading: Promise<void> | null = null;
  private recoverRequested = false;
  private lastWake: { id: string; at: number } | null = null;
  private wakePending = false;
  private wakeTimer: number | undefined;

  constructor(readonly taskId: string) {}
  getSnapshot = () => this.snapshot;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  isBlocked = () => !this.snapshot.ready || this.snapshot.busy || Boolean(this.snapshot.active);

  start = () => {
    this.running = true;
    this.generation += 1;
    this.reading = null;
    void this.refresh(true);
    return () => {
      this.running = false;
      this.generation += 1;
      window.clearTimeout(this.timer);
      window.clearTimeout(this.wakeTimer);
    };
  };

  private update(patch: Partial<CompactSnapshot>) {
    this.snapshot = { ...this.snapshot, ...patch };
    this.listeners.forEach((listener) => listener());
  }

  private merge(incoming: CompactJob[]) {
    if (incoming.some((job) => job.taskId !== this.taskId)) throw new Error('Compact job task mismatch');
    for (const job of incoming) {
      const current = this.jobs.get(job.id);
      // A slower poll may return after a POST/checkpoint response. Never regress it.
      if (current && (current.revision > job.revision || (isCompactTerminal(current) && !isCompactTerminal(job)))) continue;
      this.jobs.set(job.id, job);
    }
    const history = [...this.jobs.values()].sort(newestCompactFirst);
    const active = history.find((job) => !isCompactTerminal(job)) ?? null;
    this.update({ history, active });
  }

  private schedule() {
    window.clearTimeout(this.timer);
    if (this.running) this.timer = window.setTimeout(() => void this.refresh(), this.snapshot.active ? 2_000 : 15_000);
  }

  refresh = (recover = false): Promise<void> => {
    if (!this.running) return Promise.resolve();
    this.recoverRequested ||= recover;
    if (this.reading) return this.reading;
    const generation = this.generation;
    const active = this.snapshot.active;
    const current = () => this.running && generation === this.generation;
    const read = async () => {
      try {
        if (active && !recover) {
          const job = await api.chatGptCompactJob(active.id);
          if (!current()) return;
          this.merge([job]);
          if (isCompactTerminal(job)) {
            const result = await api.chatGptCompact(this.taskId);
            if (!current()) return;
            this.merge([...result.history, ...(result.active ? [result.active] : [])]);
          }
        } else {
          const result = await api.chatGptCompact(this.taskId);
          if (!current()) return;
          this.merge([...result.history, ...(result.active ? [result.active] : [])]);
        }
        this.update({ ready: true, error: '' });
        if (this.snapshot.active && (this.recoverRequested || this.snapshot.active.id !== active?.id)) void this.wake();
      } catch (reason) {
        if (current()) this.update({ error: `${compactText('loadError')} ${errorText(reason)}` });
      } finally {
        if (current()) {
          this.recoverRequested = false;
          this.reading = null;
          this.schedule();
        }
      }
    };
    this.reading = read();
    return this.reading;
  };

  create = async (continueAfterCompact = false): Promise<void> => {
    if (!this.running || this.isBlocked()) return;
    const generation = this.generation;
    this.update({ busy: true, error: '', extensionMessage: '' });
    try {
      const job = await api.startChatGptCompact(this.taskId, continueAfterCompact);
      if (!this.running || generation !== this.generation) return;
      this.merge([job]);
      if (!isCompactTerminal(job)) void this.wake();
    } catch (reason) {
      if (this.running && generation === this.generation) {
        this.update({ ready: false, error: errorText(reason) });
        // The POST may have succeeded even when its response was lost.
        // Keep sends locked until an authoritative read resolves that uncertainty.
        // A read started before POST is not sufficient evidence that no job exists.
        await this.reading;
        if (this.running && generation === this.generation) {
          this.update({ ready: false });
          void this.refresh(true);
        }
      }
    } finally {
      if (this.running && generation === this.generation) { this.update({ busy: false }); this.schedule(); }
    }
  };

  cancel = async (): Promise<void> => {
    const job = this.snapshot.active;
    if (!this.running || !job || this.snapshot.busy) return;
    const generation = this.generation;
    this.update({ busy: true, error: '' });
    try {
      const updated = await api.cancelChatGptCompact(job.id, job.revision);
      if (!this.running || generation !== this.generation) return;
      this.merge([updated]);
      void this.refresh();
    } catch (reason) {
      if (this.running && generation === this.generation) {
        const message = reason instanceof ApiError && reason.status === 409 ? compactText('conflict') : errorText(reason);
        await this.refresh();
        if (this.running && generation === this.generation) this.update({ error: message });
      }
    } finally {
      if (this.running && generation === this.generation) { this.update({ busy: false }); this.schedule(); }
    }
  };

  resume = () => this.refresh(true);

  private async wake() {
    const job = this.snapshot.active;
    if (!this.running || !job || this.wakePending) return;
    // Mount, reconnect, focus and explicit retry can coincide. Polling never relaunches a known job.
    const remaining = this.lastWake?.id === job.id ? 10_000 - (Date.now() - this.lastWake.at) : 0;
    if (remaining > 0) {
      window.clearTimeout(this.wakeTimer);
      this.wakeTimer = window.setTimeout(() => { void this.wake(); }, remaining);
      return;
    }
    window.clearTimeout(this.wakeTimer);
    this.lastWake = { id: job.id, at: Date.now() };
    this.wakePending = true;
    this.update({ waking: true, extensionMessage: compactText('waking') });
    try {
      await resumeChatGptCompact(job.id, this.taskId);
      if (this.running && this.snapshot.active?.id === job.id) this.update({ extensionMessage: compactText('acknowledged') });
    } catch (reason) {
      if (this.running && this.snapshot.active?.id === job.id) this.update({ extensionMessage: `${compactText('extensionMissing')} ${errorText(reason)}` });
    } finally {
      this.wakePending = false;
      if (this.running) this.update({ waking: false });
    }
  }
}

function errorText(reason: unknown) { return reason instanceof Error ? reason.message : String(reason); }
