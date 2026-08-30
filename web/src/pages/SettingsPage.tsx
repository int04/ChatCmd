import { AlertTriangle, MonitorCog, Save, SlidersHorizontal, UserRound, Volume2 } from 'lucide-react';
import { FormEvent, useEffect, useState } from 'react';
import { api } from '../api';
import { ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import { getAppLanguage, setAppLanguage, tr, translatedStatus } from '../i18n';
import { AccountSettings } from '../settings/AccountSettings';
import type { LocalSettings } from '../types';
import { useLoad } from '../useLoad';

type SettingsTab = 'account' | 'execution' | 'display' | 'sound';

const SETTINGS_TABS: Array<{ id: SettingsTab; label: string; description: string; icon: typeof UserRound }> = [
  { id: 'account', label: 'Account', description: 'Profile and security', icon: UserRound },
  { id: 'execution', label: 'Execution', description: 'Agent and terminal rules', icon: SlidersHorizontal },
  { id: 'display', label: 'Display', description: 'Theme and language', icon: MonitorCog },
  { id: 'sound', label: 'Sound', description: 'Agent notifications', icon: Volume2 },
];

export function SettingsPage() {
  const result = useLoad(api.settings, []);
  const [value, setValue] = useState<LocalSettings>();
  const [problem, setProblem] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [saved, setSaved] = useState(false);
  const [activeTab, setActiveTab] = useState<SettingsTab>('account');

  useEffect(() => {
    if (!result.data) return;
    setValue({ ...result.data, language: getAppLanguage() });
  }, [result.data]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!value) return;
    const dangerous = result.data && (
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
  const activeMeta = SETTINGS_TABS.find((tab) => tab.id === activeTab) ?? SETTINGS_TABS[0];
  const ActiveIcon = activeMeta.icon;

  return <div className="settings-page">
    <PageHeading eyebrow={tr('LOCAL CONFIGURATION')} title={tr('Settings')} body={tr('Runtime, execution, workspace, and display preferences.')} actions={<StatusBadge state={value.databaseState ?? 'unknown'} label={tr('Database {state}', { state: translatedStatus(value.databaseState ?? 'unknown') })} />} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />

    <form className="settings-workspace-form" onSubmit={submit}>
      <div className="settings-category-grid" role="tablist" aria-label={tr('Settings categories')}>
        {SETTINGS_TABS.map(({ id, label }) => <button key={id} type="button" role="tab" aria-selected={activeTab === id} className={`settings-category-card ${activeTab === id ? 'active' : ''}`} onClick={() => setActiveTab(id)}>
          {tr(label)}
        </button>)}
      </div>

      <section className="settings-workspace-card" role="tabpanel">
        <header className="settings-workspace-header">
          <div className="settings-workspace-title">
            <span><ActiveIcon aria-hidden="true" /></span>
            <div><p>{tr('SETTINGS CATEGORY')}</p><h2>{tr(activeMeta.label)}</h2><small>{sectionDescription(activeTab)}</small></div>
          </div>
          {activeTab !== 'account' && <div className="settings-workspace-status">{saved ? <strong>{tr('Settings saved.')}</strong> : <span>{tr('Changes are stored locally after saving.')}</span>}</div>}
        </header>

        <div className="settings-workspace-body">
          {activeTab === 'account' && <AccountSettings />}
          {activeTab === 'execution' && <div className="settings-control-grid">
            <SettingField label={tr('Default execution mode')} hint={tr('Applied to newly created conversations.')}><select value={value.executionMode} onChange={(event) => update('executionMode', event.target.value as LocalSettings['executionMode'])}><option value="approval">{tr('Ask for approval')}</option><option value="allowAll">{tr('Allow all')}</option></select></SettingField>
            <label className="settings-toggle-card"><input type="checkbox" checked={value.approveNewConversations} onChange={(event) => update('approveNewConversations', event.target.checked)} /><span><strong>{tr('Approve new conversations')}</strong><small>{tr('Every new conversation from the ChatGPT website must be approved before the Agent can execute anything.')}</small></span></label>
            <SettingField label={tr('Terminal executable')} hint={tr('Shell launched for terminal sessions.')}><input value={value.terminalExecutable} onChange={(event) => update('terminalExecutable', event.target.value)} /></SettingField>
            <SettingField label={tr('Task concurrency')} hint={tr('Maximum tasks allowed to run at once.')}><input type="number" min="1" max="64" value={value.taskConcurrency} onChange={(event) => update('taskConcurrency', Number(event.target.value))} /></SettingField>
            <SettingField label={tr('Session concurrency')} hint={tr('Maximum terminal sessions allowed at once.')}><input type="number" min="1" max="64" value={value.sessionConcurrency} onChange={(event) => update('sessionConcurrency', Number(event.target.value))} /></SettingField>
            <SettingField wide label={tr('Workspace roots')} hint={tr('One allowed project root per line.')}><textarea rows={5} value={value.workspaceRoots.join('\n')} onChange={(event) => update('workspaceRoots', event.target.value.split('\n').map((line) => line.trim()).filter(Boolean))} /></SettingField>
          </div>}
          {activeTab === 'display' && <div className="settings-control-grid">
            <SettingField label={tr('Theme')} hint={tr('Controls the appearance of the management UI.')}><select value={value.theme} onChange={(event) => update('theme', event.target.value as LocalSettings['theme'])}><option value="system">{tr('System')}</option><option value="light">{tr('Light')}</option><option value="dark">{tr('Dark')}</option></select></SettingField>
            <SettingField label={tr('Language')} hint={tr('Applied immediately to the current browser.')}><select value={value.language} onChange={(event) => updateLanguage(event.target.value as LocalSettings['language'])}><option value="en">English</option><option value="vi">Tiếng Việt</option></select></SettingField>
          </div>}
          {activeTab === 'sound' && <div className="settings-control-grid one-column">
            <label className="settings-toggle-card"><input type="checkbox" checked={value.newAgentSound} onChange={(event) => update('newAgentSound', event.target.checked)} /><span><strong>{tr('Sound when a new Agent becomes active')}</strong><small>{tr('Play a sound when a new task appears.')}</small></span></label>
            <label className="settings-toggle-card"><input type="checkbox" checked={value.finishedTaskSound} onChange={(event) => update('finishedTaskSound', event.target.checked)} /><span><strong>{tr('Sound when a task finishes')}</strong><small>{tr('Play a sound when the Agent sends the final response.')}</small></span></label>
          </div>}
        </div>

        {activeTab !== 'account' && <footer className="settings-workspace-footer"><span>{saved ? tr('Saved successfully') : tr('Review changes before applying them.')}</span><button className="button primary"><Save />{tr('Save settings')}</button></footer>}
      </section>
    </form>

    {confirming && <Modal title={tr('Confirm sensitive setting changes')} description={tr('Workspace or unrestricted execution changes can expand local access. Verify every value before saving.')} close={() => setConfirming(false)} dangerous><div className="warning-block"><AlertTriangle /><p>{tr('New tasks may inherit these settings immediately.')}</p></div><div className="modal-actions"><button className="button secondary" onClick={() => setConfirming(false)}>{tr('Cancel')}</button><button className="button danger" onClick={() => void save()}>{tr('Apply changes')}</button></div></Modal>}
  </div>;
}

function SettingField({ label, hint, wide, children }: { label: string; hint: string; wide?: boolean; children: React.ReactNode }) {
  return <label className={`settings-field-card ${wide ? 'wide' : ''}`}><span><strong>{label}</strong><small>{hint}</small></span>{children}</label>;
}

function sectionDescription(tab: SettingsTab) {
  if (tab === 'account') return tr('Manage your account information and password.');
  if (tab === 'execution') return tr('Defaults for new tasks and terminal processes.');
  if (tab === 'display') return tr('Stored locally for this browser.');
  return tr('Choose a separate notification sound for each Agent event type.');
}
