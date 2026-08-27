import { Check, LoaderCircle, ShieldCheck } from 'lucide-react';
import { useEffect, useState } from 'react';

import { api } from '../api';
import type { CommandExecutionMode } from '../types';

const labels = {
  title: 'Quyền truy cập',
  hint: 'Chỉ áp dụng cho đoạn trò chuyện này và các lệnh được yêu cầu tiếp theo.',
  approval: 'Phê duyệt',
  approvalHint: 'Hỏi bạn trước khi chạy mỗi lệnh chưa được cho phép.',
  allowAll: 'Cho phép tất cả',
  allowAllHint: 'Chạy lệnh ngay, không yêu cầu phê duyệt trước.',
  saving: 'Đang áp dụng…',
  error: 'Không thể đổi quyền truy cập. Hãy thử lại.',
} as const;

export function TaskAccessCard({ taskId, defaultMode }: { taskId: string; defaultMode: CommandExecutionMode }) {
  const [mode, setMode] = useState<CommandExecutionMode>(defaultMode);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState('');

  useEffect(() => {
    setMode(defaultMode);
    setError('');
    const controller = new AbortController();
    void api.taskExecutionMode(taskId, controller.signal)
      .then((current) => setMode(current.mode))
      .catch((reason: unknown) => {
        if (!(reason instanceof DOMException && reason.name === 'AbortError')) setError(labels.error);
      });
    return () => controller.abort();
  }, [defaultMode, taskId]);

  async function changeMode(next: CommandExecutionMode) {
    if (next === mode || saving) return;
    const previous = mode;
    setMode(next);
    setSaving(true);
    setError('');
    try {
      const current = await api.setTaskExecutionMode(taskId, next);
      setMode(current.mode);
    } catch {
      setMode(previous);
      setError(labels.error);
    } finally {
      setSaving(false);
    }
  }

  const options: { value: CommandExecutionMode; title: string; hint: string }[] = [
    { value: 'approval', title: labels.approval, hint: labels.approvalHint },
    { value: 'allowAll', title: labels.allowAll, hint: labels.allowAllHint },
  ];

  return <section className="task-access-card" aria-labelledby={`task-access-title-${taskId}`}>
    <header><ShieldCheck aria-hidden="true" /><div><h3 id={`task-access-title-${taskId}`}>{labels.title}</h3><p>{labels.hint}</p></div></header>
    <fieldset disabled={saving} aria-describedby={error ? `task-access-error-${taskId}` : undefined}>
      <legend className="sr-only">{labels.title}</legend>
      {options.map((option) => <label className={mode === option.value ? 'active' : ''} key={option.value}>
        <input type="radio" name={`task-access-${taskId}`} value={option.value} checked={mode === option.value} onChange={() => void changeMode(option.value)} />
        <span><strong>{option.title}</strong><small>{option.hint}</small></span>
        {mode === option.value && <Check aria-hidden="true" />}
      </label>)}
    </fieldset>
    {saving && <p className="task-access-feedback" role="status"><LoaderCircle className="spin" aria-hidden="true" />{labels.saving}</p>}
    {error && <p className="task-access-error" id={`task-access-error-${taskId}`} role="alert">{error}</p>}
  </section>;
}
