import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowLeft, Blocks, Check, CircleAlert, ExternalLink, GitFork, LoaderCircle, Plus, RefreshCw, Search, Settings2, ShieldAlert, Trash2, X } from 'lucide-react';
import { api } from '../api';
import { tr } from '../i18n';
import type { SkillInstallPreview, SkillOptionValue, UserSkill, UserSkillOption } from '../types';

type LoadState = 'loading' | 'ready' | 'error';

export function SkillsPage() {
  const [skills, setSkills] = useState<UserSkill[]>([]);
  const [loadState, setLoadState] = useState<LoadState>('loading');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [installOpen, setInstallOpen] = useState(false);
  const [optionsSkill, setOptionsSkill] = useState<UserSkill>();
  const [deleteSkill, setDeleteSkill] = useState<UserSkill>();
  const [query, setQuery] = useState('');
  const [filter, setFilter] = useState<'all' | 'enabled' | 'disabled'>('all');

  const load = useCallback(async () => {
    setLoadState('loading'); setError(undefined);
    try { setSkills(await api.skills()); setLoadState('ready'); }
    catch (reason) { setError(message(reason, tr('Could not load skills.'))); setLoadState('error'); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const globalSkills = useMemo(() => skills.filter((skill) => skill.source === 'global'), [skills]);
  const enabledCount = useMemo(() => globalSkills.filter((skill) => skill.enabled).length, [globalSkills]);
  const configurableCount = useMemo(() => globalSkills.filter((skill) => skill.options?.length).length, [globalSkills]);
  const visibleSkills = useMemo(() => {
    const normalizedQuery = query.trim().toLocaleLowerCase();
    return globalSkills.filter((skill) => {
      if (filter === 'enabled' && !skill.enabled) return false;
      if (filter === 'disabled' && skill.enabled) return false;
      if (!normalizedQuery) return true;
      return [skill.title, skill.description, skill.sourceUrl].some((value) => value?.toLocaleLowerCase().includes(normalizedQuery));
    });
  }, [filter, globalSkills, query]);

  function replaceSkill(next: UserSkill) {
    setSkills((current) => current.map((skill) => skill.id === next.id ? next : skill));
    setOptionsSkill((current) => current?.id === next.id ? next : current);
  }
  async function toggle(skill: UserSkill) {
    setBusy(`toggle:${skill.id}`); setError(undefined);
    try { replaceSkill(await api.setSkillEnabled(skill.id, !skill.enabled)); }
    catch (reason) { setError(message(reason, tr('Could not change skill status.'))); }
    finally { setBusy(undefined); }
  }
  async function preview(repositoryUrl: string) {
    setBusy('preview'); setError(undefined);
    try { return await api.previewSkills(repositoryUrl); }
    catch (reason) { throw new Error(message(reason, tr('Could not scan skills from GitHub.')), { cause: reason }); }
    finally { setBusy(undefined); }
  }
  async function install(repositoryUrl: string, skillPaths: string[]) {
    setBusy('install'); setError(undefined);
    try {
      const { skills: installed } = await api.installSkills(repositoryUrl, skillPaths);
      const installedIds = new Set(installed.map((skill) => skill.id));
      setSkills((current) => [...installed, ...current.filter((skill) => !installedIds.has(skill.id))]);
      setInstallOpen(false);
    }
    catch (reason) { throw new Error(message(reason, tr('Could not install skills from GitHub.')), { cause: reason }); }
    finally { setBusy(undefined); }
  }
  async function saveOptions(skill: UserSkill, options: Record<string, SkillOptionValue>) {
    setBusy(`options:${skill.id}`);
    try { replaceSkill(await api.updateSkillOptions(skill.id, options)); setOptionsSkill(undefined); }
    catch (reason) { throw new Error(message(reason, tr('Could not save skill options.')), { cause: reason }); }
    finally { setBusy(undefined); }
  }
  async function remove(skill: UserSkill) {
    setBusy(`delete:${skill.id}`); setError(undefined);
    try { await api.deleteSkill(skill.id); setSkills((current) => current.filter((item) => item.id !== skill.id)); setDeleteSkill(undefined); }
    catch (reason) { setError(message(reason, tr('Could not delete skill.'))); }
    finally { setBusy(undefined); }
  }

  return <div className="skills-page">
    <header className="skills-heading page-heading">
      <span className="eyebrow"><Blocks /> {tr('AGENT SKILLS')}</span>
      <div className="skills-title-row"><div><h1>{tr('Your skills')}</h1><p>{tr('Manage global skills that Agents can choose while handling tasks.')}</p></div><button className="button primary skills-add" onClick={() => setInstallOpen(true)}><Plus /> {tr('Add skill')}</button></div>
      {loadState === 'ready' && <div className="skills-stats" aria-label={tr('Skills overview')}>
        <span><strong>{globalSkills.length}</strong>{tr('Total skills')}</span>
        <span><strong>{enabledCount}</strong>{tr('Enabled')}</span>
        <span><strong>{configurableCount}</strong>{tr('Configurable')}</span>
      </div>}
    </header>

    {error && <div className="skills-error" role="alert"><CircleAlert /><span>{error}</span>{loadState === 'error' && <button onClick={() => void load()}><RefreshCw /> {tr('Retry')}</button>}<button className="plain-icon" aria-label={tr('Close notification')} onClick={() => setError(undefined)}><X /></button></div>}

    {loadState === 'loading' ? <div className="skills-skeleton" role="status" aria-live="polite" aria-busy="true" aria-label={tr('Loading')}><span /><span /><span /><span /></div>
      : loadState === 'ready' && !globalSkills.length ? <section className="skills-empty"><span><Blocks /></span><h2>{tr('No global skills yet')}</h2><p>{tr('Install skills from GitHub to give Agents specialized guidance and workflows.')}</p><small>{tr('Skills can add focused instructions, repeatable workflows, and tool-specific expertise for Agents.')}</small><button className="button primary" onClick={() => setInstallOpen(true)}><GitFork /> {tr('Add from GitHub')}</button></section>
        : <>
          <div className="skills-toolbar">
            <label className="skills-search"><Search /><span className="sr-only">{tr('Search skills')}</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder={tr('Search skills…')} /></label>
            <div className="skills-filters" role="group" aria-label={tr('Filter skills')}>
              {(['all', 'enabled', 'disabled'] as const).map((value) => <button key={value} type="button" className={filter === value ? 'active' : ''} aria-pressed={filter === value} onClick={() => setFilter(value)}>{value === 'all' ? tr('All') : value === 'enabled' ? tr('Enabled') : tr('Disabled')}</button>)}
            </div>
          </div>
          {visibleSkills.length ? <div className="skills-list" aria-label={tr('Global skills list')}>{visibleSkills.map((skill) => {
            const toggling = busy === `toggle:${skill.id}`;
            return <article className={`skill-card ${skill.enabled ? 'enabled' : 'disabled'}`} key={skill.id}>
              <header className="skill-card-header"><SkillIcon url={skill.iconUrl} /><div className="skill-copy"><div className="skill-title"><h2>{skill.title}</h2><span>{tr('Global')}</span></div><p>{skill.description || tr('This skill has no description yet.')}</p></div></header>
              <div className="skill-status-row"><span className={`skill-state ${skill.enabled ? 'enabled' : 'disabled'}`}><i />{skill.enabled ? tr('Enabled') : tr('Disabled')}</span>{!!skill.options?.length && <span className="skill-options-count"><Settings2 />{tr('{count} options', { count: skill.options.length })}</span>}</div>
              <footer className="skill-card-footer">
                <div className="skill-source">{skill.sourceUrl ? <a href={skill.sourceUrl} target="_blank" rel="noreferrer"><GitFork /><span>{shortRepository(skill.sourceUrl)}</span><ExternalLink /></a> : <span><Blocks />{tr('Local skill')}</span>}</div>
                <div className="skill-controls">
                  {!!skill.options?.length && <button className="skill-configure" disabled={busy !== undefined} onClick={() => setOptionsSkill(skill)}><Settings2 />{tr('Configure')}</button>}
                  {skill.canDelete !== false && <button className="plain-icon danger-icon" aria-label={tr('Delete skill {name}', { name: skill.title })} disabled={busy !== undefined} onClick={() => setDeleteSkill(skill)}><Trash2 /></button>}
                  <label className="skill-toggle-control"><span>{skill.enabled ? tr('On') : tr('Off')}</span><span className="agent-switch"><span className="sr-only">{tr('Enable or disable skill {name}', { name: skill.title })}</span><input type="checkbox" role="switch" aria-checked={skill.enabled} checked={skill.enabled} disabled={busy !== undefined} onChange={() => void toggle(skill)} /><i>{toggling && <LoaderCircle className="spin" />}</i></span></label>
                </div>
              </footer>
            </article>;
          })}</div> : <section className="skills-no-results"><Search /><h2>{tr('No matching skills')}</h2><p>{tr('Try another search term or change the current filter.')}</p><button type="button" className="button secondary" onClick={() => { setQuery(''); setFilter('all'); }}>{tr('Clear filters')}</button></section>}
        </>}

    {installOpen && <InstallSkillModal busy={busy === 'preview' ? 'preview' : busy === 'install' ? 'install' : undefined} onPreview={preview} onInstall={install} onClose={() => setInstallOpen(false)} />}
    {optionsSkill && <SkillOptionsModal skill={optionsSkill} busy={busy === `options:${optionsSkill.id}`} onSave={saveOptions} onClose={() => setOptionsSkill(undefined)} />}
    {deleteSkill && <ConfirmDeleteModal skill={deleteSkill} busy={busy === `delete:${deleteSkill.id}`} onConfirm={() => void remove(deleteSkill)} onClose={() => setDeleteSkill(undefined)} />}
  </div>;
}

function InstallSkillModal({ busy, onPreview, onInstall, onClose }: {
  busy?: 'preview' | 'install';
  onPreview: (url: string) => Promise<SkillInstallPreview>;
  onInstall: (url: string, paths: string[]) => Promise<void>;
  onClose: () => void;
}) {
  const [url, setUrl] = useState('');
  const [preview, setPreview] = useState<SkillInstallPreview>();
  const [selectedPaths, setSelectedPaths] = useState<Set<string>>(() => new Set());
  const [touched, setTouched] = useState(false);
  const [error, setError] = useState<string>();
  const normalizedUrl = normalizeGitHubRepositoryUrl(url);
  const isBusy = busy !== undefined;
  const availableSkills = preview?.skills.filter((skill) => !skill.installed) ?? [];
  const installedCount = (preview?.skills.length ?? 0) - availableSkills.length;
  const allSelected = availableSkills.length > 0 && availableSkills.every((skill) => selectedPaths.has(skill.path));
  const dialogRef = useDialogFocus<HTMLFormElement>(onClose, isBusy);
  const repositoryInputRef = useRef<HTMLInputElement>(null);
  const selectionHeadingRef = useRef<HTMLHeadingElement>(null);

  useEffect(() => {
    if (!preview) return;
    const frame = window.requestAnimationFrame(() => selectionHeadingRef.current?.focus());
    return () => window.cancelAnimationFrame(frame);
  }, [preview]);

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (isBusy) return;
    setError(undefined);
    if (!preview) {
      setTouched(true);
      if (!normalizedUrl) return;
      try {
        const result = await onPreview(normalizedUrl);
        setPreview(result);
        setUrl(result.repositoryUrl);
        setSelectedPaths(new Set(result.skills.filter((skill) => !skill.installed).map((skill) => skill.path)));
      } catch (reason) {
        setError(message(reason, tr('Could not scan skills from GitHub.')));
      }
      return;
    }
    if (!selectedPaths.size) return;
    try { await onInstall(preview.repositoryUrl, [...selectedPaths]); }
    catch (reason) { setError(message(reason, tr('Could not install skills from GitHub.'))); }
  }

  function changeRepository() {
    setPreview(undefined);
    setSelectedPaths(new Set());
    setError(undefined);
    setTouched(false);
    window.requestAnimationFrame(() => repositoryInputRef.current?.focus());
  }

  function toggleAll() {
    setSelectedPaths(allSelected ? new Set() : new Set(availableSkills.map((skill) => skill.path)));
  }

  function togglePath(path: string, selected: boolean) {
    setSelectedPaths((current) => {
      const next = new Set(current);
      if (selected) next.add(path); else next.delete(path);
      return next;
    });
  }

  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) onClose(); }}><form ref={dialogRef} className={`skill-modal ${preview ? 'skill-install-review' : ''}`} role="dialog" aria-modal="true" aria-labelledby="install-skill-title" onSubmit={(event) => void submit(event)}>
    <header><span><GitFork /></span><div><h2 id="install-skill-title">{preview ? tr('Choose skills to install') : tr('Add from GitHub')}</h2><p>{preview ? tr('Review the skills found in this repository before installing them globally.') : tr('Paste a GitHub repository or skill-directory link. Repositories may contain one or many skills.')}</p></div><button type="button" className="icon-button" aria-label={tr('Close')} disabled={isBusy} onClick={onClose}><X /></button></header>
    {!preview ? <div className="skill-modal-body">
      <label htmlFor="skill-repository">{tr('GitHub repository')}</label>
      <input ref={repositoryInputRef} id="skill-repository" autoFocus type="text" inputMode="url" spellCheck={false} value={url} onBlur={() => setTouched(true)} onChange={(event) => { setUrl(event.target.value); setError(undefined); }} placeholder="https://github.com/owner/repository" aria-invalid={touched && !!url && !normalizedUrl} aria-describedby={touched && url && !normalizedUrl ? 'repository-hint repository-error' : 'repository-hint'} />
      <small id="repository-hint">{tr('ChatCMD will scan the repository and let you choose which valid skills to install.')}</small>
      {touched && url && !normalizedUrl && <p id="repository-error" className="skill-form-error" role="alert">{tr('Enter an HTTPS github.com repository or /tree/{ref}/{path} URL.')}</p>}
      <div className="skill-security"><ShieldAlert /><span><strong>{tr('Only install sources you trust.')}</strong> {tr('Skills may contain instructions and scripts that Agents will execute.')}</span></div>
      {error && <p className="skill-form-error" role="alert">{error}</p>}
    </div> : <div className="skill-modal-body skill-install-body">
      <div className="skill-repository-summary"><GitFork /><div><strong>{shortRepository(preview.repositoryUrl)}</strong><code>{preview.repositoryUrl}</code></div><button type="button" className="button secondary" disabled={isBusy} onClick={changeRepository}>{tr('Change')}</button></div>
      <div className="skill-selection-heading"><div><h3 ref={selectionHeadingRef} tabIndex={-1}>{tr('{count} skills found', { count: preview.skills.length })}</h3><p role="status" aria-live="polite">{tr('{selected} of {available} available selected', { selected: selectedPaths.size, available: availableSkills.length })}</p></div>{availableSkills.length > 0 && <button type="button" className="skill-selection-toggle" disabled={isBusy} onClick={toggleAll}>{allSelected ? tr('Clear all') : tr('Select all')}</button>}</div>
      <fieldset className="skill-candidate-list" disabled={isBusy}><legend className="sr-only">{tr('Skills available to install')}</legend>{preview.skills.map((skill) => {
        const selected = selectedPaths.has(skill.path);
        return <label className={`skill-candidate ${skill.installed ? 'installed' : ''}`} key={skill.path}>
          <input type="checkbox" checked={selected} disabled={skill.installed || isBusy} onChange={(event) => togglePath(skill.path, event.target.checked)} />
          <span className="skill-candidate-check" aria-hidden="true">{selected && <Check />}</span>
          <span className="skill-candidate-copy"><span><strong>{skill.title}</strong>{skill.installed && <em>{tr('Installed')}</em>}</span><small>{skill.description}</small><code>{skill.path}</code></span>
        </label>;
      })}</fieldset>
      {installedCount > 0 && <p className="skill-install-note">{tr('{count} skills are already installed and were left unselected.', { count: installedCount })}</p>}
      {preview.skippedInvalid > 0 && <p className="skill-install-note warning"><CircleAlert />{tr('{count} invalid skill folders were skipped.', { count: preview.skippedInvalid })}</p>}
      <div className="skill-security"><ShieldAlert /><span><strong>{tr('Only install sources you trust.')}</strong> {tr('Skills may contain instructions and scripts that Agents will execute.')}</span></div>
      {error && <p className="skill-form-error" role="alert">{error}</p>}
    </div>}
    <footer>{preview ? <>
      <button type="button" className="button secondary" disabled={isBusy} onClick={changeRepository}><ArrowLeft /> {tr('Back')}</button>
      <button className="button primary" disabled={!selectedPaths.size || isBusy}>{busy === 'install' ? <LoaderCircle className="spin" /> : <Plus />} {busy === 'install' ? tr('Installing…') : selectedPaths.size === 1 ? tr('Install 1 skill') : tr('Install {count} skills', { count: selectedPaths.size })}</button>
    </> : <>
      <button type="button" className="button secondary" disabled={isBusy} onClick={onClose}>{tr('Cancel')}</button>
      <button className="button primary" disabled={!normalizedUrl || isBusy}>{busy === 'preview' ? <LoaderCircle className="spin" /> : <Search />} {busy === 'preview' ? tr('Scanning repository…') : tr('Find skills')}</button>
    </>}</footer>
  </form></div>;
}

function SkillOptionsModal({ skill, busy, onSave, onClose }: { skill: UserSkill; busy: boolean; onSave: (skill: UserSkill, values: Record<string, SkillOptionValue>) => Promise<void>; onClose: () => void }) {
  const [values, setValues] = useState<Record<string, SkillOptionValue>>(() => Object.fromEntries(skill.options.map((option) => [option.key, option.value]))); const [error, setError] = useState<string>();
  const dialogRef = useDialogFocus<HTMLFormElement>(onClose, busy);
  async function submit(event: FormEvent) { event.preventDefault(); setError(undefined); try { await onSave(skill, values); } catch (reason) { setError(message(reason, tr('Could not save skill options.'))); } }
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><form ref={dialogRef} className="skill-modal" role="dialog" aria-modal="true" aria-labelledby="skill-options-title" onSubmit={(event) => void submit(event)}>
    <header><span><Settings2 /></span><div><h2 id="skill-options-title">{tr('Configure: {name}', { name: skill.title })}</h2><p>{tr('Agents use these values when applying the skill.')}</p></div><button type="button" className="icon-button" aria-label={tr('Close')} disabled={busy} onClick={onClose}><X /></button></header>
    <div className="skill-modal-body option-fields">{skill.options.map((option) => <OptionField key={option.key} option={option} value={values[option.key]} onChange={(value) => setValues((current) => ({ ...current, [option.key]: value }))} />)}{error && <p className="skill-form-error" role="alert">{error}</p>}</div>
    <footer><button type="button" className="button secondary" disabled={busy} onClick={onClose}>{tr('Cancel')}</button><button className="button primary" disabled={busy}>{busy ? <LoaderCircle className="spin" /> : <Check />} {tr('Save options')}</button></footer>
  </form></div>;
}

function OptionField({ option, value, onChange }: { option: UserSkillOption; value: SkillOptionValue; onChange: (value: SkillOptionValue) => void }) {
  if (option.type === 'boolean') return <label className="skill-option-toggle"><span><strong>{option.label}</strong>{option.description && <small>{option.description}</small>}</span><span className="agent-switch"><input type="checkbox" role="switch" aria-checked={Boolean(value)} checked={Boolean(value)} onChange={(event) => onChange(event.target.checked)} /><i /></span></label>;
  const id = `skill-option-${option.key}`;
  return <label className="skill-option-field" htmlFor={id}><strong>{option.label}</strong>{option.type === 'select' ? <select id={id} value={String(value)} onChange={(event) => onChange(event.target.value)}>{(option.choices ?? []).map((choice) => <option key={choice.value} value={choice.value}>{choice.label}</option>)}</select> : <input id={id} type={option.type === 'number' ? 'number' : 'text'} value={String(value)} onChange={(event) => onChange(option.type === 'number' ? Number(event.target.value) : event.target.value)} />}{option.description && <small>{option.description}</small>}</label>;
}

function ConfirmDeleteModal({ skill, busy, onConfirm, onClose }: { skill: UserSkill; busy: boolean; onConfirm: () => void; onClose: () => void }) {
  const dialogRef = useDialogFocus<HTMLElement>(onClose, busy);
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><section ref={dialogRef} className="skill-modal compact" role="alertdialog" aria-modal="true" aria-labelledby="delete-skill-title"><header><span className="danger"><Trash2 /></span><div><h2 id="delete-skill-title">{tr('Delete skill?')}</h2><p>{tr('Skill {name} will be removed from this machine.', { name: skill.title })}</p></div><button className="icon-button" aria-label={tr('Close')} disabled={busy} onClick={onClose}><X /></button></header><footer><button autoFocus className="button secondary" disabled={busy} onClick={onClose}>{tr('Cancel')}</button><button className="button danger" disabled={busy} onClick={onConfirm}>{busy ? <LoaderCircle className="spin" /> : <Trash2 />} {tr('Delete')}</button></footer></section></div>;
}

function SkillIcon({ url }: { url?: string | null }) { const [failed, setFailed] = useState(false); const safeUrl = safeSkillIconUrl(url); return <div className="skill-icon">{safeUrl && !failed ? <img src={safeUrl} alt="" loading="lazy" onError={() => setFailed(true)} /> : <Blocks />}</div>; }
function useDialogFocus<T extends HTMLElement>(onClose: () => void, busy: boolean) {
  const dialogRef = useRef<T>(null); const closeRef = useRef(onClose); const busyRef = useRef(busy);
  useEffect(() => { closeRef.current = onClose; busyRef.current = busy; }, [onClose, busy]);
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null; const dialog = dialogRef.current;
    const frame = window.requestAnimationFrame(() => (dialog?.querySelector<HTMLElement>('[autofocus]') ?? focusableElements(dialog)[0])?.focus());
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape' && !busyRef.current) { event.preventDefault(); closeRef.current(); return; }
      if (event.key !== 'Tab') return;
      const focusable = focusableElements(dialogRef.current);
      if (!focusable.length) { event.preventDefault(); dialogRef.current?.focus(); return; }
      const first = focusable[0]; const last = focusable[focusable.length - 1];
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus(); }
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => { window.cancelAnimationFrame(frame); window.removeEventListener('keydown', handleKeyDown); if (previousFocus?.isConnected) previousFocus.focus(); };
  }, []);
  return dialogRef;
}
function focusableElements(root: HTMLElement | null) { return root ? Array.from(root.querySelectorAll<HTMLElement>('button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),a[href],[tabindex]:not([tabindex="-1"])')).filter((element) => !element.hidden && element.getClientRects().length > 0) : []; }
function safeSkillIconUrl(value?: string | null) { if (!value) return null; if (/^(data:image\/|blob:)/i.test(value)) return value; try { const url = new URL(value, location.origin); return url.origin === location.origin ? url.href : null; } catch { return null; } }
function normalizeGitHubRepositoryUrl(value: string) {
  const pastedUrl = value.trim().match(/https:\/\/(?:www\.)?github\.com\/[^\s\])]+/i)?.[0] ?? value.trim();
  const candidate = pastedUrl.replace(/[,;\])]+$/, '').replace(/\/$/, '');
  try {
    const url = new URL(candidate);
    if (url.protocol !== 'https:' || !['github.com', 'www.github.com'].includes(url.hostname.toLowerCase()) || url.username || url.password || url.port || url.search || url.hash) return null;
    const parts = url.pathname.split('/').filter(Boolean);
    if (parts.length < 2 || (parts.length > 2 && (parts.length < 4 || parts[2] !== 'tree'))) return null;
    return `https://github.com/${parts.join('/')}`;
  } catch { return null; }
}
function shortRepository(url: string) { try { const path = new URL(url).pathname.replace(/^\//, '').replace(/\.git$/, ''); return path || url; } catch { return url; } }
function message(reason: unknown, fallback: string) { return reason instanceof Error ? reason.message : fallback; }
