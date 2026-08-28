import { AlertTriangle, MonitorCog, Save, ServerCog, SlidersHorizontal, Volume2 } from 'lucide-react';
import { FormEvent, useEffect, useState } from 'react';
import { api } from '../api';
import { ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import type { LocalSettings } from '../types';
import { useLoad } from '../useLoad';

type SettingsTab = 'network' | 'execution' | 'display' | 'sound';

const SETTINGS_TABS: Array<{ id: SettingsTab; label: string; icon: typeof ServerCog }> = [
  { id: 'network', label: 'Network & storage', icon: ServerCog },
  { id: 'execution', label: 'Execution', icon: SlidersHorizontal },
  { id: 'display', label: 'Display', icon: MonitorCog },
  { id: 'sound', label: 'Âm thanh', icon: Volume2 },
];

export function SettingsPage() {
  const result = useLoad(api.settings, []);
  const [value, setValue] = useState<LocalSettings>();
  const [problem, setProblem] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [saved, setSaved] = useState(false);
  const [activeTab, setActiveTab] = useState<SettingsTab>('network');

  useEffect(() => setValue(result.data), [result.data]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!value) return;
    const dangerous = result.data && (
      value.bindAddress !== result.data.bindAddress ||
      value.workspaceRoots.join('\n') !== result.data.workspaceRoots.join('\n') ||
      value.executionMode === 'allowAll' && result.data.executionMode !== 'allowAll'
    );
    if (dangerous) setConfirming(true);
    else void save();
  };

  const save = async () => {
    if (!value) return;
    setConfirming(false);
    setProblem('');
    try {
      const next = await api.saveSettings(value);
      result.setData(next);
      setValue(next);
      localStorage.setItem('chatcmd.preferences', JSON.stringify({
        theme: next.theme,
        language: next.language,
        sound: next.sound,
        newAgentSound: next.newAgentSound,
        finishedTaskSound: next.finishedTaskSound,
      }));
      document.documentElement.dataset.theme = next.theme;
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2500);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : 'Save failed');
    }
  };

  if (result.loading) return <Loading label="Loading settings" />;
  if (result.error || !value) return <ErrorState message={result.error} retry={() => void result.reload()} />;

  const update = <K extends keyof LocalSettings>(key: K, next: LocalSettings[K]) => setValue({ ...value, [key]: next });

  return <div>
    <PageHeading
      eyebrow="LOCAL CONFIGURATION"
      title="Settings"
      body="Runtime, execution, workspace, and display preferences."
      actions={<StatusBadge state={value.databaseState ?? 'unknown'} label={`Database ${value.databaseState ?? 'unknown'}`} />}
    />
    <ProblemBanner message={problem} clear={() => setProblem('')} />

    <form className="settings-form" onSubmit={submit}>
      <div className="settings-tabs" role="tablist" aria-label="Settings categories">
        {SETTINGS_TABS.map(({ id, label, icon: Icon }) => <button key={id} type="button" role="tab" aria-selected={activeTab === id} className={`settings-tab ${activeTab === id ? 'active' : ''}`} onClick={() => setActiveTab(id)}>
          <Icon aria-hidden="true" /><span>{label}</span>
        </button>)}
      </div>

      <div className="settings-tab-panel" role="tabpanel">
        {activeTab === 'network' && <SettingsSection title="Network & storage" body="Listener and SQLite paths reported by local host."><div className="form-grid"><label>Bind address<input value={value.bindAddress} onChange={(event) => update('bindAddress', event.target.value)} /></label><label>UI port<input type="number" min="1" max="65535" value={value.port} onChange={(event) => update('port', Number(event.target.value))} /></label><label className="span-2">MCP endpoint<input value={value.mcpEndpoint} onChange={(event) => update('mcpEndpoint', event.target.value)} /></label><label className="span-2">Database path<input value={value.databasePath} onChange={(event) => update('databasePath', event.target.value)} /></label></div></SettingsSection>}
        {activeTab === 'execution' && <SettingsSection title="Execution" body="Defaults for new tasks and terminal processes."><div className="form-grid"><label>Default execution mode<select value={value.executionMode} onChange={(event) => update('executionMode', event.target.value as LocalSettings['executionMode'])}><option value="approval">Ask for approval</option><option value="allowAll">Allow all</option></select></label><label>Terminal executable<input value={value.terminalExecutable} onChange={(event) => update('terminalExecutable', event.target.value)} /></label><label>Task concurrency<input type="number" min="1" max="64" value={value.taskConcurrency} onChange={(event) => update('taskConcurrency', Number(event.target.value))} /></label><label>Session concurrency<input type="number" min="1" max="64" value={value.sessionConcurrency} onChange={(event) => update('sessionConcurrency', Number(event.target.value))} /></label><label className="span-2">Workspace roots<textarea rows={4} value={value.workspaceRoots.join('\n')} onChange={(event) => update('workspaceRoots', event.target.value.split('\n').map((line) => line.trim()).filter(Boolean))} /></label></div></SettingsSection>}
        {activeTab === 'display' && <SettingsSection title="Display" body="Stored locally for this browser."><div className="form-grid"><label>Theme<select value={value.theme} onChange={(event) => update('theme', event.target.value as LocalSettings['theme'])}><option value="system">System</option><option value="light">Light</option><option value="dark">Dark</option></select></label><label>Language<select value={value.language} onChange={(event) => update('language', event.target.value as LocalSettings['language'])}><option value="en">English</option><option value="vi">Tiếng Việt</option></select></label></div></SettingsSection>}
        {activeTab === 'sound' && <SettingsSection title="Âm thanh" body="Chọn riêng âm báo cho từng loại sự kiện của Agent."><div className="form-grid"><label className="check-row span-2"><input type="checkbox" checked={value.newAgentSound} onChange={(event) => update('newAgentSound', event.target.checked)} /><span><strong>Âm thanh khi Agent mới hoạt động</strong><small>Phát âm báo khi một công việc mới xuất hiện.</small></span></label><label className="check-row span-2"><input type="checkbox" checked={value.finishedTaskSound} onChange={(event) => update('finishedTaskSound', event.target.checked)} /><span><strong>Âm thanh báo khi hoàn thành công việc</strong><small>Phát âm báo khi Agent gửi phản hồi cuối.</small></span></label></div></SettingsSection>}
      </div>

      <div className="save-bar"><span role="status">{saved ? 'Settings saved.' : ''}</span><button className="button primary"><Save />Save settings</button></div>
    </form>

    {confirming && <Modal title="Confirm sensitive setting changes" description="Listener, workspace, or unrestricted execution changes can expand local access. Verify every value before saving." close={() => setConfirming(false)} dangerous><div className="warning-block"><AlertTriangle /><p>New tasks may inherit these settings immediately. Existing connections may need restart.</p></div><div className="modal-actions"><button className="button secondary" onClick={() => setConfirming(false)}>Cancel</button><button className="button danger" onClick={() => void save()}>Apply changes</button></div></Modal>}
  </div>;
}

function SettingsSection({ title, body, children }: { title: string; body: string; children: React.ReactNode }) {
  return <section className="settings-section"><header><h2>{title}</h2><p>{body}</p></header>{children}</section>;
}
