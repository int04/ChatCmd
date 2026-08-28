import { FormEvent, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Blocks, Check, CircleAlert, ExternalLink, GitFork, LoaderCircle, Plus, RefreshCw, Settings2, ShieldAlert, Trash2, X } from 'lucide-react';
import { api } from '../api';
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
    setLoadState('loading');
    setError(undefined);
    try {
      setSkills(await api.skills());
      setLoadState('ready');
    } catch (reason) {
      setError(message(reason, 'Không thể tải danh sách kỹ năng.'));
      setLoadState('error');
    }
  }, []);
  useEffect(() => { void load(); }, [load]);
  const globalSkills = useMemo(() => skills.filter((skill) => skill.source === 'global'), [skills]);

  function replaceSkill(next: UserSkill) {
    setSkills((current) => current.map((skill) => skill.id === next.id ? next : skill));
    setOptionsSkill((current) => current?.id === next.id ? next : current);
  }
  async function toggle(skill: UserSkill) {
    setBusy(`toggle:${skill.id}`);
    setError(undefined);
    try { replaceSkill(await api.setSkillEnabled(skill.id, !skill.enabled)); }
    catch (reason) { setError(message(reason, 'Không thể thay đổi trạng thái kỹ năng.')); }
    finally { setBusy(undefined); }
  }
  async function install(repositoryUrl: string) {
    setBusy('install');
    setError(undefined);
    try {
      const installed = await api.installSkill(repositoryUrl);
      setSkills((current) => [installed, ...current.filter((skill) => skill.id !== installed.id)]);
      setInstallOpen(false);
    } catch (reason) {
      throw new Error(message(reason, 'Không thể cài kỹ năng từ GitHub.'), { cause: reason });
    } finally { setBusy(undefined); }
  }
  async function saveOptions(skill: UserSkill, options: Record<string, SkillOptionValue>) {
    setBusy(`options:${skill.id}`);
    try {
      replaceSkill(await api.updateSkillOptions(skill.id, options));
      setOptionsSkill(undefined);
    } catch (reason) {
      throw new Error(message(reason, 'Không thể lưu tùy chọn kỹ năng.'), { cause: reason });
    } finally { setBusy(undefined); }
  }
  async function remove(skill: UserSkill) {
    setBusy(`delete:${skill.id}`);
    setError(undefined);
    try {
      await api.deleteSkill(skill.id);
      setSkills((current) => current.filter((item) => item.id !== skill.id));
      setDeleteSkill(undefined);
    } catch (reason) {
      setError(message(reason, 'Không thể xóa kỹ năng.'));
    } finally { setBusy(undefined); }
  }

  return <div className="skills-page">
    <header className="skills-heading page-heading">
      <span className="eyebrow"><Blocks /> KỸ NĂNG AGENT</span>
      <div className="skills-title-row">
        <div><h1>Kỹ năng của bạn</h1><p>Quản lý các kỹ năng global mà Agent có thể chọn khi xử lý công việc.</p></div>
        <button className="button primary skills-add" onClick={() => setInstallOpen(true)}><Plus /> Thêm kỹ năng</button>
      </div>
    </header>

    {error && <div className="skills-error" role="alert"><CircleAlert /><span>{error}</span>{loadState === 'error' && <button onClick={() => void load()}><RefreshCw /> Thử lại</button>}<button className="plain-icon" aria-label="Đóng thông báo" onClick={() => setError(undefined)}><X /></button></div>}

    {loadState === 'loading' ? <div className="skills-skeleton" role="status" aria-live="polite" aria-busy="true" aria-label="Đang tải"><span /><span /><span /></div>
      : loadState === 'ready' && !globalSkills.length ? <section className="skills-empty"><span><Blocks /></span><h2>Chưa có kỹ năng global</h2><p>Cài kỹ năng từ GitHub để Agent có thêm hướng dẫn và quy trình chuyên biệt.</p><button className="button primary" onClick={() => setInstallOpen(true)}><GitFork /> Thêm từ GitHub</button></section>
        : <div className="skills-list" aria-label="Danh sách kỹ năng global">{globalSkills.map((skill) => {
          const toggling = busy === `toggle:${skill.id}`;
          return <article className={`skill-card ${skill.enabled ? 'enabled' : 'disabled'}`} key={skill.id}>
            <SkillIcon url={skill.iconUrl} />
            <div className="skill-copy"><div className="skill-title"><h2>{skill.title}</h2><span>Global</span></div><p>{skill.description || 'Kỹ năng này chưa có mô tả.'}</p>{skill.sourceUrl && <a href={skill.sourceUrl} target="_blank" rel="noreferrer"><GitFork />{shortRepository(skill.sourceUrl)}<ExternalLink /></a>}</div>
            <div className="skill-controls">
              <label className="agent-switch"><span className="sr-only">Bật hoặc tắt kỹ năng {skill.title}</span><input type="checkbox" role="switch" aria-checked={skill.enabled} checked={skill.enabled} disabled={busy !== undefined} onChange={() => void toggle(skill)} /><i>{toggling && <LoaderCircle className="spin" />}</i></label>
              {!!skill.options?.length && <button className="plain-icon" aria-label={`Tùy chỉnh kỹ năng ${skill.title}`} disabled={busy !== undefined} onClick={() => setOptionsSkill(skill)}><Settings2 /></button>}
              {skill.canDelete !== false && <button className="plain-icon danger-icon" aria-label={`Xóa kỹ năng ${skill.title}`} disabled={busy !== undefined} onClick={() => setDeleteSkill(skill)}><Trash2 /></button>}
            </div>
          </article>;
        })}</div>}

    {installOpen && <InstallSkillModal busy={busy === 'install'} onInstall={install} onClose={() => setInstallOpen(false)} />}
    {optionsSkill && <SkillOptionsModal skill={optionsSkill} busy={busy === `options:${optionsSkill.id}`} onSave={saveOptions} onClose={() => setOptionsSkill(undefined)} />}
    {deleteSkill && <ConfirmDeleteModal skill={deleteSkill} busy={busy === `delete:${deleteSkill.id}`} onConfirm={() => void remove(deleteSkill)} onClose={() => setDeleteSkill(undefined)} />}
  </div>;
}

function InstallSkillModal({ busy, onInstall, onClose }: { busy: boolean; onInstall: (url: string) => Promise<void>; onClose: () => void }) {
  const [url, setUrl] = useState('');
  const [error, setError] = useState<string>();
  const valid = /^https:\/\/github\.com\/[^/]+\/[^/]+(?:\/.*)?$/i.test(url.trim());
  const dialogRef = useDialogFocus<HTMLFormElement>(onClose, busy);
  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!valid || busy) return;
    setError(undefined);
    try { await onInstall(url.trim()); }
    catch (reason) { setError(message(reason, 'Không thể cài kỹ năng từ GitHub.')); }
  }
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><form ref={dialogRef} className="skill-modal" role="dialog" aria-modal="true" aria-labelledby="install-skill-title" onSubmit={(event) => void submit(event)}>
    <header><span><GitFork /></span><div><h2 id="install-skill-title">Thêm từ GitHub</h2><p>Dán liên kết repository GitHub chứa cấu trúc SKILL.md chuẩn.</p></div><button type="button" className="icon-button" aria-label="Đóng" disabled={busy} onClick={onClose}><X /></button></header>
    <div className="skill-modal-body"><label htmlFor="skill-repository">Repository GitHub</label><input id="skill-repository" autoFocus type="url" inputMode="url" spellCheck={false} value={url} onChange={(event) => setUrl(event.target.value)} placeholder="https://github.com/owner/repository" aria-describedby="repository-hint" /><small id="repository-hint">ChatCMD sẽ tải và kiểm tra cấu trúc kỹ năng trước khi cài vào vùng global.</small><div className="skill-security"><ShieldAlert /><span><strong>Chỉ cài nguồn bạn tin cậy.</strong> Kỹ năng có thể chứa hướng dẫn và script mà Agent sẽ thực thi.</span></div>{error && <p className="skill-form-error" role="alert">{error}</p>}</div>
    <footer><button type="button" className="button secondary" disabled={busy} onClick={onClose}>Hủy</button><button className="button primary" disabled={!valid || busy}>{busy ? <LoaderCircle className="spin" /> : <Plus />} Cài kỹ năng</button></footer>
  </form></div>;
}

function SkillOptionsModal({ skill, busy, onSave, onClose }: { skill: UserSkill; busy: boolean; onSave: (skill: UserSkill, values: Record<string, SkillOptionValue>) => Promise<void>; onClose: () => void }) {
  const [values, setValues] = useState<Record<string, SkillOptionValue>>(() => Object.fromEntries(skill.options.map((option) => [option.key, option.value])));
  const [error, setError] = useState<string>();
  const dialogRef = useDialogFocus<HTMLFormElement>(onClose, busy);
  async function submit(event: FormEvent) {
    event.preventDefault();
    setError(undefined);
    try { await onSave(skill, values); }
    catch (reason) { setError(message(reason, 'Không thể lưu tùy chọn kỹ năng.')); }
  }
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><form ref={dialogRef} className="skill-modal" role="dialog" aria-modal="true" aria-labelledby="skill-options-title" onSubmit={(event) => void submit(event)}>
    <header><span><Settings2 /></span><div><h2 id="skill-options-title">Tùy chỉnh: {skill.title}</h2><p>Các giá trị này được Agent dùng khi áp dụng kỹ năng.</p></div><button type="button" className="icon-button" aria-label="Đóng" disabled={busy} onClick={onClose}><X /></button></header>
    <div className="skill-modal-body option-fields">{skill.options.map((option) => <OptionField key={option.key} option={option} value={values[option.key]} onChange={(value) => setValues((current) => ({ ...current, [option.key]: value }))} />)}{error && <p className="skill-form-error" role="alert">{error}</p>}</div>
    <footer><button type="button" className="button secondary" disabled={busy} onClick={onClose}>Hủy</button><button className="button primary" disabled={busy}>{busy ? <LoaderCircle className="spin" /> : <Check />} Lưu tùy chọn</button></footer>
  </form></div>;
}

function OptionField({ option, value, onChange }: { option: UserSkillOption; value: SkillOptionValue; onChange: (value: SkillOptionValue) => void }) {
  if (option.type === 'boolean') return <label className="skill-option-toggle"><span><strong>{option.label}</strong>{option.description && <small>{option.description}</small>}</span><span className="agent-switch"><input type="checkbox" role="switch" aria-checked={Boolean(value)} checked={Boolean(value)} onChange={(event) => onChange(event.target.checked)} /><i /></span></label>;
  const id = `skill-option-${option.key}`;
  return <label className="skill-option-field" htmlFor={id}><strong>{option.label}</strong>{option.type === 'select' ? <select id={id} value={String(value)} onChange={(event) => onChange(event.target.value)}>{(option.choices ?? []).map((choice) => <option key={choice.value} value={choice.value}>{choice.label}</option>)}</select> : <input id={id} type={option.type === 'number' ? 'number' : 'text'} value={String(value)} onChange={(event) => onChange(option.type === 'number' ? Number(event.target.value) : event.target.value)} />}{option.description && <small>{option.description}</small>}</label>;
}

function ConfirmDeleteModal({ skill, busy, onConfirm, onClose }: { skill: UserSkill; busy: boolean; onConfirm: () => void; onClose: () => void }) {
  const dialogRef = useDialogFocus<HTMLElement>(onClose, busy);
  return <div className="skill-modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !busy) onClose(); }}><section ref={dialogRef} className="skill-modal compact" role="alertdialog" aria-modal="true" aria-labelledby="delete-skill-title"><header><span className="danger"><Trash2 /></span><div><h2 id="delete-skill-title">Xóa kỹ năng?</h2><p>Kỹ năng {skill.title} sẽ bị xóa khỏi máy này.</p></div><button className="icon-button" aria-label="Đóng" disabled={busy} onClick={onClose}><X /></button></header><footer><button autoFocus className="button secondary" disabled={busy} onClick={onClose}>Hủy</button><button className="button danger" disabled={busy} onClick={onConfirm}>{busy ? <LoaderCircle className="spin" /> : <Trash2 />} Xóa</button></footer></section></div>;
}

function SkillIcon({ url }: { url?: string | null }) {
  const [failed, setFailed] = useState(false);
  const safeUrl = safeSkillIconUrl(url);
  return <div className="skill-icon">{safeUrl && !failed ? <img src={safeUrl} alt="" loading="lazy" onError={() => setFailed(true)} /> : <Blocks />}</div>;
}

function useDialogFocus<T extends HTMLElement>(onClose: () => void, busy: boolean) {
  const dialogRef = useRef<T>(null);
  const closeRef = useRef(onClose);
  const busyRef = useRef(busy);
  useEffect(() => { closeRef.current = onClose; busyRef.current = busy; }, [onClose, busy]);
  useEffect(() => {
    const previousFocus = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const dialog = dialogRef.current;
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
