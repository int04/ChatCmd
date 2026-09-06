import { Check, Copy } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import type { RichTextLabels } from './labels';

export function CopyTextButton({ text, labels }: { text: string; labels: RichTextLabels }) {
  const [status, setStatus] = useState<'idle' | 'copied' | 'failed'>('idle');
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  const copy = async () => {
    try {
      if (!navigator.clipboard?.writeText) throw new Error('Clipboard unavailable');
      await navigator.clipboard.writeText(text);
      setStatus('copied');
    } catch { setStatus('failed'); }
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setStatus('idle'), 4000);
  };
  return <span className="chat-copy-control">
    <button type="button" className="chat-copy-button" onClick={() => void copy()} aria-label={labels.copy}>
      {status === 'copied' ? <Check aria-hidden="true" /> : <Copy aria-hidden="true" />}
      <span>{status === 'copied' ? labels.copied : labels.copy}</span>
    </button>
    <span className="chat-copy-status" role="status">{status === 'failed' ? labels.copyFailed : ''}</span>
  </span>;
}
