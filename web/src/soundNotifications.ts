const STORAGE_KEY = 'chatcmd.preferences';

class SoundNotifications {
  private readonly newAgent = this.createAudio('sounds/new_agent.mp3');
  private readonly finishedTask = this.createAudio('sounds/finish_chat.mp3');
  private primed = false;

  constructor() {
    if (typeof window === 'undefined') return;
    const prime = () => void this.prime();
    window.addEventListener('pointerdown', prime, { once: true, passive: true });
    window.addEventListener('keydown', prime, { once: true });
  }

  playNewAgent(): void { this.play(this.newAgent); }
  playFinishedTask(): void { this.play(this.finishedTask); }

  private createAudio(path: string): HTMLAudioElement | null {
    if (typeof Audio === 'undefined') return null;
    const audio = new Audio(`${import.meta.env.BASE_URL}${path}`);
    audio.preload = 'auto';
    return audio;
  }

  private enabled(): boolean {
    try {
      const value = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? '{}') as { sound?: unknown };
      return value.sound !== false;
    } catch {
      return true;
    }
  }

  private play(audio: HTMLAudioElement | null): void {
    if (!audio || !this.enabled()) return;
    audio.currentTime = 0;
    void audio.play().catch(() => undefined);
  }

  private async prime(): Promise<void> {
    if (this.primed) return;
    this.primed = true;
    await Promise.all([this.unlock(this.newAgent), this.unlock(this.finishedTask)]);
  }

  private async unlock(audio: HTMLAudioElement | null): Promise<void> {
    if (!audio) return;
    const volume = audio.volume;
    audio.volume = 0;
    try { await audio.play(); } catch { /* Browser can still require another interaction. */ }
    audio.pause();
    audio.currentTime = 0;
    audio.volume = volume;
  }
}

export const soundNotifications = new SoundNotifications();
