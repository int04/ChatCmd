import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Blocks, Check, CircleAlert, ExternalLink, GitFork, LoaderCircle, Plus, RefreshCw, Settings2, ShieldAlert, Trash2, X } from 'lucide-react';
import { api } from '../api';
import { tr } from '../i18n';
import type { SkillOptionValue, UserSkill, UserSkillOption } from '../types';

type LoadState = 'loading' | 'ready' | 'error';

export function SkillsPage() {
  const [skills, setSkills] = useState<UserSkill[]>([]);
  const [loadState, setLoadState] = useState<LoadState>('loading');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState<string>();
  const [installOpen, setInstallOpen] = useState(false);
  const [optionsSkill, setOptionsSkill] = useState<UserSkill>();
  const [deleteSkill, setDeleteSkill] = useState<UserSkill>();

  const load = useCallback(async () => {
    setLoadState('loading'); setError(undefined);
    try { setSkills(await api.skills()); setLoadState('ready'); }
    catch (reason) { setError(message(reason, tr('Could not load skills.'))); setLoadState('error'); }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const globalSkills = useMemo(() => skills.filter((skill) => skill.source === 'global'), [skills]);

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
  async function install(repositoryUrl: string) {
    setBusy('install'); setError(undefined);
    try { const installed = await api.installSkill(repositoryUrl); setSkills((current) => [installed, ...current.filter((skill) => skill.id !== installed.id)]); setInstallOpen(false); }
    catch (reason) { throw new Error(message(reason, tr('Could not install skill from GitHub.')), { cause: reason }); }
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
    </header>

    {error && <div className="skills-error" role="alert"><CircleAlert /><span>{error}</span>{loadState === 'error' && <button onClick={() => void load()}><RefreshCw /> {tr('Retry')}</button>}<button className="plain-icon" aria-label={tr('Close notification')} onClick={() => setError(undefined)}><X /></button></div>}

    {loadState === 'loading' ? <div className="skills-skeleton" role="status" aria-live="polite" aria-busy="true" aria-label={tr('Loading')}><span /><span /><span /></div>
      : loadState === 'ready' && !globalSkills.length ? <section className="skills-empty"><span><Blocks /></span><h2>{tr('No global skills yet')}</h2><p>{tr('Install skills from GitHub to give Agents specialized guidance and workflows.')}</p><button className="button primary" onClick={() => setInstallOpen(true)}><GitFork /> {tr('Add from GitHub')}</button></section>
        : <div className="skills-list" aria-label={tr('Global skills list')}>{globalSkills.map((skill) => {
          const toggling = busy === `toggle:${skill.id}`;
          return <article className={`skill-card ${skill.enabled ? 'enabled' : 'disabled'}`} key={skill.id}>
            <SkillIcon url={skill.iconUrl} />
            <div className="skill-copy"><div className="skill-title"><h2>{skill.title}</h2><span>{tr('Global')}</span></div><p>{skill.description || tr('This skill has no description yet.')}</p>{skill.sourceUrl && <a href={skill.sourceUrl} target="_blank" rel="noreferrer"><GitFork />{shortRepository(skill.sourceUrl)}<ExternalLink /></a>}</div>
            <div className="skill-controls">
              <label className="agent-switch"><span className="sr-only">{tr('Enable or disable skill {name}', { name: skill.title })}</span><input type="checkbox" role="switch" aria-checked={skill.enabled} checked={skill.enabled} disabled={busy !== undefined} onChange={() => void toggle(skill)} /><i>{toggling && <LoaderCircle className="spin" />}</i></label>
              {!!skill.options?.length && <button className="plain-icon" aria-label={tr('Configure skill {name}', { name: skill.title })} disabled={busy !== undefined} onClick={() => setOptionsSkill(skill)}><Settings2 /></button>}
              {skill.canDelete !== false && <button className="plain-icon danger-icon" aria-label={tr('Delete skill {name}', { name: skill.title })} disabled={busy !== undefined} onClick={() => setDeleteSkill(skill)}><Trash2 /></button>}
            </div>
          </article>;
        })}</div>}

    {installOpen && <InstallSkillModal busy={busy === 'install'} onInstall={install} onClose={() => setInstallOpen(false)} />}
    {optionsSkill && <SkillOptionsModal skill={optionsSkill} busy={busy === `options:${optionsSkill.id}`} onSave={saveOptions} onClose={() => setOptionsSkill(undefined)} />}
    {deleteSkill && <ConfirmDeleteModal skill={deleteSkill} busy={busy === `delete:${deleteSkill.id}`} onConfirm={() => void remove(deleteSkill)} onClose={() => setDeleteSkill(undefined)} />}
  </div>;
}

function InstallSkillModal({ busy, onInstall, onClose }: { busy: boolean; onInstall: (url: string) => Promise<void>; onClose: () => void }) {
  const [url, setUrl] = useState(''); const [error, setError] = useState<string>();
  const valid = /^https:\/\/github\.com\/[^/]+\/[^/]+(?:\/.*)?$/i.test(url.trim());
  const dialogRef = useDialogFocus<HTMLFormElement>(onClose, busy);
  async function submit(event: FormEvent) { event.preventDefault(); if (!valid || busy) return; setError(undefined); try { await onInstall(url.trim()); } catch (reason) { setError(message(reason, tr('Could not install skill from GitHub.'))); } }
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><form ref={dialogRef} className="skill-modal" role="dialog" aria-modal="true" aria-labelledby="install-skill-title" onSubmit={(event) => void submit(event)}>
    <header><span><GitFork /></span><div><h2 id="install-skill-title">{tr('Add from GitHub')}</h2><p>{tr('Paste the GitHub repository link containing a valid SKILL.md structure.')}</p></div><button type="button" className="icon-button" aria-label={tr('Close')} disabled={busy} onClick={onClose}><X /></button></header>
    <div className="skill-modal-body"><label htmlFor="skill-repository">{tr('GitHub repository')}</label><input id="skill-repository" autoFocus type="url" inputMode="url" spellCheck={false} value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://github.com/owner/repository" aria-describedby="repository-hint" /><small id="repository-hint">{tr('ChatCMD will download and validate the skill structure before installing it globally.')}</small><div className="skill-security"><ShieldAlert /><span><strong>{tr('Only install sources you trust.')}</strong> {tr('Skills may contain instructions and scripts that Agents will execute.')}</span></div>{error && <p className="skill-form-error" role="alert">{error}</p>}</div>
    <footer><button type="button" className="button secondary" disabled={busy} onClick={onClose}>{tr('Cancel')}</button><button className="button primary" disabled={!valid || busy}>{busy ? <LoaderCircle className="spin" /> : <Plus />} {tr('Install skill')}</button></footer>
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
function shortRepository(url: string) { try { const path = new URL(url).pathname.replace(/^\//, '').replace(/\.git$/, ''); return path || url; } catch { return url; } }
function message(reason: unknown, fallback: string) { return reason instanceof Error ? reason.message : fallback; }
