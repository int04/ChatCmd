import { AlertTriangle, MonitorCog, Save, ServerCog, SlidersHorizontal, Volume2 } from 'lucide-react';
import { FormEvent, useEffect, useState } from 'react';
import { api } from '../api';
import { ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import { getAppLanguage, setAppLanguage, tr, translatedStatus } from '../i18n';
import type { LocalSettings } from '../types';
import { useLoad } from '../useLoad';

type SettingsTab = 'network' | 'execution' | 'display' | 'sound';

const SETTINGS_TABS: Array<{ id: SettingsTab; label: string; icon: typeof ServerCog }> = [
  { id: 'network', label: 'Network & storage', icon: ServerCog },
  { id: 'execution', label: 'Execution', icon: SlidersHorizontal },
  { id: 'display', label: 'Display', icon: MonitorCog },
  { id: 'sound', label: 'Sound', icon: Volume2 },
];

export function SettingsPage() {
  const result = useLoad(api.settings, []);
  const [value, setValue] = useState<LocalSettings>();
  const [problem, setProblem] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [saved, setSaved] = useState(false);
  const [activeTab, setActiveTab] = useState<SettingsTab>('network');

  useEffect(() => {
    if (!result.data) return;
    setValue({ ...result.data, language: getAppLanguage() });
  }, [result.data]);

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
      localStorage.setItem('chatcmd.preferences', JSON.stringify({ theme: next.theme, language: next.language, sound: next.sound, newAgentSound: next.newAgentSound, finishedTaskSound: next.finishedTaskSound }));
      document.documentElement.dataset.theme = next.theme;
      setAppLanguage(next.language, true);
      setSaved(true);
      window.setTimeout(() => setSaved(false), 2500);
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Save failed')); }
  };

  if (result.loading) return <Loading label={tr('Loading settings')} />;
  if (result.error || !value) return <ErrorState message={result.error} retry={() => void result.reload()} />;

  const update = <K extends keyof LocalSettings>(key: K, next: LocalSettings[K]) => setValue({ ...value, [key]: next });
  const updateLanguage = (language: LocalSettings['language']) => { update('language', language); setAppLanguage(language, true); };

  return <div>
    <PageHeading eyebrow={tr('LOCAL CONFIGURATION')} title={tr('Settings')} body={tr('Runtime, execution, workspace, and display preferences.')} actions={<StatusBadge state={value.databaseState ?? 'unknown'} label={tr('Database {state}', { state: translatedStatus(value.databaseState ?? 'unknown') })} />} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />

    <form className="settings-form" onSubmit={submit}>
      <div className="settings-tabs" role="tablist" aria-label={tr('Settings categories')}>
        {SETTINGS_TABS.map(({ id, label, icon: Icon }) => <button key={id} type="button" role="tab" aria-selected={activeTab === id} className={`settings-tab ${activeTab === id ? 'active' : ''}`} onClick={() => setActiveTab(id)}><Icon aria-hidden="true" /><span>{tr(label)}</span></button>)}
      </div>

      <div className="settings-tab-panel" role="tabpanel">
        {activeTab === 'network' && <SettingsSection title={tr('Network & storage')} body={tr('Listener and SQLite paths reported by local host.')}><div className="form-grid"><label>{tr('Bind address')}<input value={value.bindAddress} onChange={(event) => update('bindAddress', event.target.value)} /></label><label>{tr('UI port')}<input type="number" min="1" max="65535" value={value.port} onChange={(event) => update('port', Number(event.target.value))} /></label><label className="span-2">{tr('MCP endpoint')}<input value={value.mcpEndpoint} onChange={(event) => update('mcpEndpoint', event.target.value)} /></label><label className="span-2">{tr('Database path')}<input value={value.databasePath} onChange={(event) => update('databasePath', event.target.value)} /></label></div></SettingsSection>}
        {activeTab === 'execution' && <SettingsSection title={tr('Execution')} body={tr('Defaults for new tasks and terminal processes.')}><div className="form-grid"><label>{tr('Default execution mode')}<select value={value.executionMode} onChange={(event) => update('executionMode', event.target.value as LocalSettings['executionMode'])}><option value="approval">{tr('Ask for approval')}</option><option value="allowAll">{tr('Allow all')}</option></select></label><label>{tr('Terminal executable')}<input value={value.terminalExecutable} onChange={(event) => update('terminalExecutable', event.target.value)} /></label><label>{tr('Task concurrency')}<input type="number" min="1" max="64" value={value.taskConcurrency} onChange={(event) => update('taskConcurrency', Number(event.target.value))} /></label><label>{tr('Session concurrency')}<input type="number" min="1" max="64" value={value.sessionConcurrency} onChange={(event) => update('sessionConcurrency', Number(event.target.value))} /></label><label className="span-2">{tr('Workspace roots')}<textarea rows={4} value={value.workspaceRoots.join('\n')} onChange={(event) => update('workspaceRoots', event.target.value.split('\n').map((line) => line.trim()).filter(Boolean))} /></label></div></SettingsSection>}
        {activeTab === 'display' && <SettingsSection title={tr('Display')} body={tr('Stored locally for this browser.')}><div className="form-grid"><label>{tr('Theme')}<select value={value.theme} onChange={(event) => update('theme', event.target.value as LocalSettings['theme'])}><option value="system">{tr('System')}</option><option value="light">{tr('Light')}</option><option value="dark">{tr('Dark')}</option></select></label><label>{tr('Language')}<select value={value.language} onChange={(event) => updateLanguage(event.target.value as LocalSettings['language'])}><option value="en">English</option><option value="vi">Tiếng Việt</option></select></label></div></SettingsSection>}
        {activeTab === 'sound' && <SettingsSection title={tr('Sound')} body={tr('Choose a separate notification sound for each Agent event type.')}><div className="form-grid"><label className="check-row span-2"><input type="checkbox" checked={value.newAgentSound} onChange={(event) => update('newAgentSound', event.target.checked)} /><span><strong>{tr('Sound when a new Agent becomes active')}</strong><small>{tr('Play a sound when a new task appears.')}</small></span></label><label className="check-row span-2"><input type="checkbox" checked={value.finishedTaskSound} onChange={(event) => update('finishedTaskSound', event.target.checked)} /><span><strong>{tr('Sound when a task finishes')}</strong><small>{tr('Play a sound when the Agent sends the final response.')}</small></span></label></div></SettingsSection>}
      </div>

      <div className="save-bar"><span role="status">{saved ? tr('Settings saved.') : ''}</span><button className="button primary"><Save />{tr('Save settings')}</button></div>
    </form>

    {confirming && <Modal title={tr('Confirm sensitive setting changes')} description={tr('Listener, workspace, or unrestricted execution changes can expand local access. Verify every value before saving.')} close={() => setConfirming(false)} dangerous><div className="warning-block"><AlertTriangle /><p>{tr('New tasks may inherit these settings immediately. Existing connections may need restart.')}</p></div><div className="modal-actions"><button className="button secondary" onClick={() => setConfirming(false)}>{tr('Cancel')}</button><button className="button danger" onClick={() => void save()}>{tr('Apply changes')}</button></div></Modal>}
  </div>;
}

function SettingsSection({ title, body, children }: { title: string; body: string; children: React.ReactNode }) { return <section className="settings-section"><header><h2>{title}</h2><p>{body}</p></header>{children}</section>; }
