import { AlertTriangle, Download, Info, MonitorCog, Save, ShieldCheck, SlidersHorizontal, TerminalSquare, UserRound, Volume2 } from 'lucide-react';
import { FormEvent, useEffect, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api } from '../api';
import { ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import { getAppLanguage, setAppLanguage, tr, translatedStatus } from '../i18n';
import { AccountSettings } from '../settings/AccountSettings';
import type { LocalSettings } from '../types';
import { useLoad } from '../useLoad';
import { UpdateSettings } from '../updates/UpdateSettings';
import { updateCopy } from '../updates/copy';

type SettingsTab = 'account' | 'execution' | 'display' | 'sound' | 'update';

const SETTINGS_TABS: Array<{ id: SettingsTab; label: string; description: string; icon: typeof UserRound }> = [
  { id: 'account', label: 'Account', description: 'Profile and security', icon: UserRound },
  { id: 'execution', label: 'Execution', description: 'Agent and terminal rules', icon: SlidersHorizontal },
  { id: 'display', label: 'Display', description: 'Theme and language', icon: MonitorCog },
  { id: 'sound', label: 'Sound', description: 'Agent notifications', icon: Volume2 },
  { id: 'update', label: 'Update', description: 'Version and updater', icon: Download },
];

export function SettingsPage() {
  const copy = updateCopy();
  const [searchParams, setSearchParams] = useSearchParams();
  const result = useLoad(api.settings, []);
  const [value, setValue] = useState<LocalSettings>();
  const [problem, setProblem] = useState('');
  const [confirming, setConfirming] = useState(false);
  const [saved, setSaved] = useState(false);
  const [activeTab, setActiveTab] = useState<SettingsTab>(searchParams.get('tab') === 'update' ? 'update' : 'account');

  useEffect(() => {
    if (searchParams.get('tab') === 'update') setActiveTab('update');
  }, [searchParams]);

  useEffect(() => {
    if (!result.data) return;
    setValue({ ...result.data, language: getAppLanguage() });
  }, [result.data]);

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!value) return;
    const dangerous = result.data && value.executionMode === 'allowAll' && result.data.executionMode !== 'allowAll';
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
    <PageHeading eyebrow={tr('LOCAL CONFIGURATION')} title={tr('Settings')} body={tr('Runtime, execution, and display preferences.')} actions={<StatusBadge state={value.databaseState ?? 'unknown'} label={tr('Database {state}', { state: translatedStatus(value.databaseState ?? 'unknown') })} />} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />

    <form className="settings-workspace-form" onSubmit={submit}>
      <div className="settings-category-grid" role="tablist" aria-label={tr('Settings categories')}>
        {SETTINGS_TABS.map(({ id, label, description, icon: Icon }) => <button key={id} type="button" role="tab" aria-selected={activeTab === id} className={`settings-category-card ${activeTab === id ? 'active' : ''}`} onClick={() => { setActiveTab(id); setSearchParams(id === 'update' ? { tab: 'update' } : {}); }}>
          <span className="settings-category-icon"><Icon aria-hidden="true" /></span>
          <span><strong>{id === 'update' ? copy.tabLabel : tr(label)}</strong><small>{id === 'update' ? copy.tabDescription : tr(description)}</small></span>
        </button>)}
      </div>

      <section className="settings-workspace-card" role="tabpanel">
        <header className="settings-workspace-header">
          <div className="settings-workspace-title">
            <span><ActiveIcon aria-hidden="true" /></span>
            <div><p>{tr('SETTINGS CATEGORY')}</p><h2>{activeTab === 'update' ? copy.tabLabel : tr(activeMeta.label)}</h2><small>{sectionDescription(activeTab)}</small></div>
          </div>
          {activeTab !== 'account' && activeTab !== 'update' && <div className="settings-workspace-status">{saved ? <strong>{tr('Settings saved.')}</strong> : <span>{tr('Changes are stored locally after saving.')}</span>}</div>}
        </header>

        <div className="settings-workspace-body">
          {activeTab === 'account' && <AccountSettings />}
          {activeTab === 'execution' && <>
            <SettingsIntro icon={<ShieldCheck />} title={tr('Execution safety and limits')} description={tr('These settings control how much access Agents receive and how many tasks can run at the same time. Use restrictive values when you are unsure.')} />
            <div className="settings-section-block">
              <SectionHeading title={tr('Approval and permissions')} description={tr('Choose when the Agent must stop and ask you before performing local actions.')} />
              <div className="settings-control-grid">
                <SettingField label={tr('Default execution mode')} hint={tr('Applied to newly created conversations.')} detail={tr('Ask for approval is safer because sensitive actions require confirmation. Allow all reduces interruptions but gives new conversations broader local execution access.')}><select value={value.executionMode} onChange={(event) => update('executionMode', event.target.value as LocalSettings['executionMode'])}><option value="approval">{tr('Ask for approval')}</option><option value="allowAll">{tr('Allow all')}</option></select></SettingField>
                <ToggleSetting checked={value.approveNewConversations} onChange={(checked) => update('approveNewConversations', checked)} label={tr('Approve new conversations')} hint={tr('Every new conversation from the ChatGPT website must be approved before the Agent can execute anything.')} detail={tr('Recommended when multiple people can access this computer or when you want to review every new Agent session before it starts.')} />
              </div>
            </div>
            <div className="settings-section-block">
              <SectionHeading icon={<TerminalSquare />} title={tr('Terminal and concurrency')} description={tr('Configure the shell used by Agents and limit parallel work to protect CPU, memory, and terminal stability.')} />
              <div className="settings-control-grid">
                <SettingField label={tr('Terminal executable')} hint={tr('Shell launched for terminal sessions.')} detail={tr('Use an executable available on this device, for example PowerShell, cmd, bash, or zsh. An invalid path can prevent terminal sessions from starting.')}><input value={value.terminalExecutable} onChange={(event) => update('terminalExecutable', event.target.value)} /></SettingField>
                <SettingField label={tr('Task concurrency')} hint={tr('Maximum tasks allowed to run at once.')} detail={tr('Higher values improve parallel throughput but can increase CPU, memory, disk, and network usage. Lower this value on weaker devices.')}><input type="number" min="1" max="64" value={value.taskConcurrency} onChange={(event) => update('taskConcurrency', Number(event.target.value))} /></SettingField>
                <SettingField label={tr('Session concurrency')} hint={tr('Maximum terminal sessions allowed at once.')} detail={tr('Limits how many terminal processes can remain active simultaneously. Too many sessions may consume memory even when they are idle.')}><input type="number" min="1" max="64" value={value.sessionConcurrency} onChange={(event) => update('sessionConcurrency', Number(event.target.value))} /></SettingField>
              </div>
            </div>
          </>}
          {activeTab === 'display' && <>
            <SettingsIntro icon={<MonitorCog />} title={tr('Appearance and language')} description={tr('Personalize how ChatCMD looks and which language is used in the management interface. These choices do not change Agent permissions.')} />
            <div className="settings-section-block"><div className="settings-control-grid">
              <SettingField label={tr('Theme')} hint={tr('Controls the appearance of the management UI.')} detail={tr('System follows your operating system. Light and Dark keep a fixed appearance until you change this setting again.')}><select value={value.theme} onChange={(event) => update('theme', event.target.value as LocalSettings['theme'])}><option value="system">{tr('System')}</option><option value="light">{tr('Light')}</option><option value="dark">{tr('Dark')}</option></select></SettingField>
              <SettingField label={tr('Language')} hint={tr('Applied immediately to the current browser.')} detail={tr('Changes interface labels and descriptions. Technical output from tools, terminals, or external services may still use its original language.')}><select value={value.language} onChange={(event) => updateLanguage(event.target.value as LocalSettings['language'])}><option value="en">English</option><option value="vi">Tiếng Việt</option></select></SettingField>
            </div></div>
          </>}
          {activeTab === 'sound' && <>
            <SettingsIntro icon={<Volume2 />} title={tr('Notification sounds')} description={tr('Use sounds to notice important Agent events when ChatCMD is open in the background or another browser tab.')} />
            <div className="settings-section-block"><div className="settings-control-grid one-column">
              <ToggleSetting checked={value.newAgentSound} onChange={(checked) => update('newAgentSound', checked)} label={tr('Sound when a new Agent becomes active')} hint={tr('Play a sound when a new task appears.')} detail={tr('Useful when approval is enabled so you can notice a newly waiting conversation without constantly watching the page.')} />
              <ToggleSetting checked={value.finishedTaskSound} onChange={(checked) => update('finishedTaskSound', checked)} label={tr('Sound when a task finishes')} hint={tr('Play a sound when the Agent sends the final response.')} detail={tr('Useful for long-running tasks so you can switch to other work and return after the Agent finishes.')} />
            </div></div>
          </>}
          {activeTab === 'update' && <UpdateSettings />}
        </div>

        {activeTab !== 'account' && activeTab !== 'update' && <footer className="settings-workspace-footer"><span>{saved ? tr('Saved successfully') : tr('Review changes before applying them.')}</span><button className="button primary"><Save />{tr('Save settings')}</button></footer>}
      </section>
    </form>

    {confirming && <Modal title={tr('Confirm sensitive setting changes')} description={tr('Unrestricted execution can expand local access. Verify this setting before saving.')} close={() => setConfirming(false)} dangerous><div className="warning-block"><AlertTriangle /><p>{tr('New tasks may inherit these settings immediately.')}</p></div><div className="modal-actions"><button className="button secondary" onClick={() => setConfirming(false)}>{tr('Cancel')}</button><button className="button danger" onClick={() => void save()}>{tr('Apply changes')}</button></div></Modal>}
  </div>;
}

function SettingsIntro({ icon, title, description }: { icon: React.ReactNode; title: string; description: string }) {
  return <div className="settings-intro"><span>{icon}</span><div><strong>{title}</strong><p>{description}</p></div></div>;
}

function SectionHeading({ icon, title, description }: { icon?: React.ReactNode; title: string; description: string }) {
  return <div className="settings-section-heading"><div><strong>{icon}{title}</strong><p>{description}</p></div></div>;
}

function SettingField({ label, hint, detail, wide, children }: { label: string; hint: string; detail: string; wide?: boolean; children: React.ReactNode }) {
  return <label className={`settings-field-card ${wide ? 'wide' : ''}`}><span className="settings-field-copy"><strong>{label}</strong><small>{hint}</small><span className="settings-field-detail"><Info />{detail}</span></span><div className="settings-field-control">{children}</div></label>;
}

function ToggleSetting({ checked, onChange, label, hint, detail }: { checked: boolean; onChange: (checked: boolean) => void; label: string; hint: string; detail: string }) {
  return <label className="settings-toggle-card"><input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} /><span className="settings-field-copy"><strong>{label}</strong><small>{hint}</small><span className="settings-field-detail"><Info />{detail}</span></span></label>;
}

function sectionDescription(tab: SettingsTab) {
  if (tab === 'update') return updateCopy().categoryDescription;
  if (tab === 'account') return tr('Manage your account information and password.');
  if (tab === 'execution') return tr('Defaults for new tasks and terminal processes.');
  if (tab === 'display') return tr('Stored locally for this browser.');
  return tr('Choose a separate notification sound for each Agent event type.');
}
