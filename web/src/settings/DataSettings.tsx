import { AlertTriangle, Database, FileText, LoaderCircle, RefreshCw, ScrollText, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { api, type DatabaseDiagnostics, type DiagnosticLogs } from '../api';
import { getChatGptExtensionLogs, type ChatGptExtensionLog } from '../chatgptBridge';
import { tr } from '../i18n';

type DataTab = 'data' | 'logs' | 'extension';

export function DataSettings() {
  const [searchParams, setSearchParams] = useSearchParams();
  const section = searchParams.get('section');
  const [activeTab, setActiveTab] = useState<DataTab>(section === 'logs' || section === 'extension' ? section : 'data');
  useEffect(() => { const next = searchParams.get('section'); if (next === 'data' || next === 'logs' || next === 'extension') setActiveTab(next); }, [searchParams]);
  const select = (tab: DataTab) => { setActiveTab(tab); setSearchParams({ tab: 'data', section: tab }); };
  return <div className="data-settings">
    <div className="data-settings-tabs" role="tablist">
      <button type="button" className={activeTab === 'data' ? 'active' : ''} onClick={() => select('data')}><Database />{tr('Data')}</button>
      <button type="button" className={activeTab === 'logs' ? 'active' : ''} onClick={() => select('logs')}><FileText />{tr('Diagnostic logs')}</button>
      <button type="button" className={activeTab === 'extension' ? 'active' : ''} onClick={() => select('extension')}><ScrollText />{tr('Extension logs')}</button>
    </div>
    {activeTab === 'data' && <DatabasePanel />}
    {activeTab === 'logs' && <DiagnosticLogPanel />}
    {activeTab === 'extension' && <ExtensionLogPanel />}
  </div>;
}

type DataRetention = '1h' | '5h' | '10h' | '1d' | '3d' | '5d' | '10d' | 'off';
const DATA_RETENTION_OPTIONS: Array<{ value: DataRetention; label: string }> = [
  { value: '1h', label: '1 giờ' }, { value: '5h', label: '5 giờ' }, { value: '10h', label: '10 giờ' },
  { value: '1d', label: '1 ngày' }, { value: '3d', label: '3 ngày' }, { value: '5d', label: '5 ngày' },
  { value: '10d', label: '10 ngày' }, { value: 'off', label: 'Tắt' },
];

function DatabasePanel() {
  const [value, setValue] = useState<DatabaseDiagnostics>(); const [loading, setLoading] = useState(true); const [deleting, setDeleting] = useState(false); const [retention, setRetention] = useState<DataRetention>('1d'); const [savingRetention, setSavingRetention] = useState(false); const [error, setError] = useState('');
  const refresh = useCallback(async () => { setLoading(true); setError(''); try { const [diagnostics, settings] = await Promise.all([api.databaseDiagnostics(), api.settings()]); setValue(diagnostics); setRetention(settings.dataRetention ?? '1d'); } catch (reason) { setError(reason instanceof Error ? reason.message : tr('Could not load database information.')); } finally { setLoading(false); } }, []);
  const deleteAll = useCallback(async () => {
    if (!window.confirm('Bạn có chắc chắn muốn xóa toàn bộ dữ liệu phát sinh trong quá trình sử dụng không?')) return;
    setDeleting(true); setError('');
    try {
      await api.deleteAllUserData();
      await refresh();
      window.dispatchEvent(new Event('chatcmd:conversations-cleared'));
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Không thể xóa toàn bộ dữ liệu.'); }
    finally { setDeleting(false); }
  }, [refresh]);
  const changeRetention = useCallback(async (next: DataRetention) => {
    const previous = retention; setRetention(next); setSavingRetention(true); setError('');
    try { const settings = await api.settings(); await api.saveSettings({ ...settings, dataRetention: next }); }
    catch (reason) { setRetention(previous); setError(reason instanceof Error ? reason.message : 'Không thể lưu thời hạn tự động xóa dữ liệu.'); }
    finally { setSavingRetention(false); }
  }, [retention]);
  useEffect(() => { void refresh(); }, [refresh]);
  if (loading && !value) return <PanelState loading label={tr('Loading database information')} />;
  if (error && !value) return <PanelState error label={error} />;
  if (!value) return null;
  return <div className="data-settings-panel">
    <PanelHeader title="SQLite" subtitle={value.path} loading={loading} refresh={refresh} />
    <div className="data-metrics"><Metric label={tr('Tables')} value={formatNumber(value.tableCount)} /><Metric label={tr('Rows')} value={formatNumber(value.totalRows)} /><Metric label={tr('Database size')} value={formatBytes(value.fileSizeBytes)} /><Metric label={tr('Used pages')} value={`${formatBytes(value.usedSizeBytes)} / ${formatNumber(value.pageCount)} pages`} /></div>
    {error && <div className="data-panel-error"><AlertTriangle />{error}</div>}
    <div className="data-danger-zone"><div><strong>Xóa toàn bộ dữ liệu ngay</strong><span>Xóa toàn bộ dữ liệu phát sinh trong quá trình sử dụng, giữ nguyên agent, thư mục dự án và cấu hình hệ thống.</span></div><button className="button danger" type="button" disabled={deleting} onClick={() => void deleteAll()}>{deleting ? <LoaderCircle className="spin" /> : <Trash2 />}{deleting ? tr('Deleting…') : 'Xóa toàn bộ dữ liệu ngay'}</button></div>
    <div className="data-retention-setting"><div><strong>Tự động xóa dữ liệu sau</strong><span>Xóa dữ liệu thường xuyên giúp ứng dụng hoạt động mượt mà hơn.</span></div><select value={retention} disabled={savingRetention} onChange={(event) => void changeRetention(event.target.value as DataRetention)}>{DATA_RETENTION_OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select></div>
    <div className="data-table-wrap"><table className="data-table"><thead><tr><th>{tr('Table')}</th><th>{tr('Rows')}</th></tr></thead><tbody>{value.tables.map((table) => <tr key={table.name}><td><code>{table.name}</code></td><td>{formatNumber(table.rowCount)}</td></tr>)}</tbody></table></div>
    <div className="data-storage-note">{tr('Page size')}: {formatBytes(value.pageSizeBytes)} · {tr('Free pages')}: {formatNumber(value.freePageCount)}</div>
  </div>;
}

function DiagnosticLogPanel() {
  const [value, setValue] = useState<DiagnosticLogs>(); const [loading, setLoading] = useState(true); const [error, setError] = useState('');
  const refresh = useCallback(async () => { setLoading(true); setError(''); try { setValue(await api.diagnosticLogs()); } catch (reason) { setError(reason instanceof Error ? reason.message : tr('Could not read diagnostic logs.')); } finally { setLoading(false); } }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  if (loading && !value) return <PanelState loading label={tr('Loading logs')} />;
  if (error && !value) return <PanelState error label={error} />;
  if (!value) return null;
  return <div className="data-settings-panel"><PanelHeader title={tr('Diagnostic logs')} subtitle={`${value.path} · ${formatNumber(value.lineCount)} ${tr('lines')}`} loading={loading} refresh={refresh} />{!value.lines.length ? <div className="data-empty">{tr('No diagnostic logs yet.')}</div> : <div className="diagnostic-log-list">{value.lines.map((line, index) => <div className="diagnostic-log-row" key={`${index}-${line}`}><code>{line}</code></div>)}</div>}</div>;
}

function ExtensionLogPanel() {
  const [logs, setLogs] = useState<ChatGptExtensionLog[]>([]); const [loading, setLoading] = useState(true); const [error, setError] = useState('');
  const refresh = useCallback(async () => { setLoading(true); setError(''); try { setLogs((await getChatGptExtensionLogs()).slice(-300).reverse()); } catch (reason) { setError(reason instanceof Error ? reason.message : tr('Could not read extension logs.')); } finally { setLoading(false); } }, []);
  useEffect(() => { void refresh(); }, [refresh]);
  const content = useMemo(() => logs.map((log, index) => <div className={`extension-log-window-row ${log.level}`} key={`${log.at}-${index}`}><time>{new Date(log.at).toLocaleTimeString()}</time><strong>{log.source}</strong><span>{log.message}</span></div>), [logs]);
  return <div className="data-settings-panel"><PanelHeader title={tr('Extension logs')} subtitle="ChatCMD ChatGPT Bridge" loading={loading} refresh={refresh} />{error ? <div className="data-panel-error"><AlertTriangle />{error}</div> : loading && !logs.length ? <PanelState loading label={tr('Loading')} /> : !logs.length ? <div className="data-empty">{tr('No extension logs yet.')}</div> : <div className="extension-log-settings-body">{content}</div>}</div>;
}

function PanelHeader({ title, subtitle, loading, refresh }: { title: string; subtitle: string; loading: boolean; refresh: () => void | Promise<void> }) { return <header className="data-panel-header"><div><strong>{title}</strong><small title={subtitle}>{subtitle}</small></div><button type="button" onClick={() => void refresh()} disabled={loading}><RefreshCw className={loading ? 'spin' : ''} /></button></header>; }
function PanelState({ loading, error, label }: { loading?: boolean; error?: boolean; label: string }) { return <div className={`data-panel-state ${error ? 'error' : ''}`}>{loading ? <LoaderCircle className="spin" /> : <AlertTriangle />}{label}</div>; }
function Metric({ label, value }: { label: string; value: string }) { return <div className="data-metric"><span>{label}</span><strong>{value}</strong></div>; }
function formatNumber(value: number) { return new Intl.NumberFormat().format(value); }
function formatBytes(value: number) { if (!Number.isFinite(value) || value <= 0) return '0 B'; const units = ['B', 'KB', 'MB', 'GB', 'TB']; const unit = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1); return `${(value / 1024 ** unit).toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`; }
