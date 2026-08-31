import { CheckCircle2, Cloud, Copy, Globe2, KeyRound, Link2, LoaderCircle, Network, Pencil, PlugZap, Plus, Power, RotateCw, Trash2, Wifi } from 'lucide-react';
import { FormEvent, useEffect, useMemo, useState } from 'react';
import { api } from '../api';
import { Empty, ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import { tr } from '../i18n';
import type { Agent, AgentInput, PluginLink, SecretResult, Tool, Tunnel } from '../types';
import { useLoad } from '../useLoad';

const blank: AgentInput = { name: '', enabled: true, toolIds: [] };
const groupOrder = ['group-device', 'group-terminal', 'group-files', 'group-git', 'group-process', 'group-skills', 'group-tasks'];
const groupNames: Record<string, string> = {
  'group-device': 'Device',
  'group-terminal': 'Terminal',
  'group-files': 'Files & workspace',
  'group-git': 'Git',
  'group-process': 'Processes',
  'group-skills': 'Skills',
  'group-tasks': 'Tasks & agent lifecycle',
};

export function AgentsPage() {
  const agents = useLoad(api.agents, []);
  const tunnels = useLoad(api.tunnels, []);
  const managedTunnel = useLoad(api.managedTunnelStatus, []);
  const tools = useLoad(api.tools, []);
  const presets = useLoad(api.presets, []);
  const [editor, setEditor] = useState<Agent | 'new'>();
  const [secret, setSecret] = useState<SecretResult>();
  const [pluginAgent, setPluginAgent] = useState<Agent>();
  const [addingTunnel, setAddingTunnel] = useState(false);
  const [testingTunnelId, setTestingTunnelId] = useState<number>();
  const [testedTunnelId, setTestedTunnelId] = useState<number>();
  const [tunnelConnectionBusy, setTunnelConnectionBusy] = useState(false);
  const [problem, setProblem] = useState('');
  const refreshManagedTunnel = managedTunnel.refresh;
  const managedTunnelState = managedTunnel.data?.state;
  const managedTunnelActive = managedTunnelState === 'connecting' || managedTunnelState === 'connected' || managedTunnelState === 'reconnecting';
  const managedTunnelStateLabel = managedTunnel.loading && !managedTunnel.data
    ? tr('Checking…')
    : managedTunnelState === 'connecting'
      ? tr('Connecting…')
      : managedTunnelState === 'connected'
        ? tr('Connected')
        : managedTunnelState === 'reconnecting'
          ? tr('Reconnecting…')
          : managedTunnelState === 'disconnected'
            ? tr('Disconnected')
            : tr('Unavailable');

  useEffect(() => {
    if (!managedTunnelState || managedTunnelState === 'disconnected') return;
    const timer = window.setInterval(() => { void refreshManagedTunnel(); }, 3000);
    return () => window.clearInterval(timer);
  }, [managedTunnelState, refreshManagedTunnel]);

  const updateList = (next: Agent) => agents.setData((current) => current ? [next, ...current.filter((item) => item.id !== next.id)] : [next]);
  const save = async (input: AgentInput) => {
    setProblem('');
    try {
      if (editor === 'new') {
        const created = await api.createAgent(input);
        if (created.agent) updateList(created.agent);
        setSecret(created);
      } else if (editor) {
        updateList(await api.updateAgent(editor.id, input));
      }
      setEditor(undefined);
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Save failed')); }
  };
  const rotate = async (agent: Agent) => {
    setProblem('');
    try { setSecret(await api.rotateAgentSecret(agent.id)); }
    catch (error) { setProblem(error instanceof Error ? error.message : tr('Rotation failed')); }
  };
  const remove = async (agent: Agent) => {
    if (!confirm(tr('Delete access profile “{name}”? Existing connection links for it will stop working.', { name: agent.name }))) return;
    try {
      await api.deleteAgent(agent.id);
      agents.setData((current) => current?.filter((item) => item.id !== agent.id));
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Delete failed')); }
  };
  const testTunnel = async (tunnel: Tunnel) => {
    setProblem('');
    setTestedTunnelId(undefined);
    setTestingTunnelId(tunnel.id);
    try {
      await api.testTunnel(tunnel.id);
      setTestedTunnelId(tunnel.id);
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Tunnel test failed')); }
    finally { setTestingTunnelId(undefined); }
  };
  const tunnelAdded = (tunnel: Tunnel) => {
    tunnels.setData((current) => current ? [tunnel, ...current.filter((item) => item.id !== tunnel.id)] : [tunnel]);
    setAddingTunnel(false);
    setTestedTunnelId(tunnel.id);
  };
  const connectTunnel = async () => {
    setProblem('');
    setTunnelConnectionBusy(true);
    managedTunnel.setData((current) => current ? { ...current, state: 'connecting', connected: false, lastError: undefined } : current);
    try {
      managedTunnel.setData(await api.connectManagedTunnel());
      await tunnels.reload();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : tr('Tunnel connection failed'));
      await managedTunnel.reload();
    }
    finally { setTunnelConnectionBusy(false); }
  };
  const disconnectTunnel = async () => {
    setProblem('');
    setTunnelConnectionBusy(true);
    try { managedTunnel.setData(await api.disconnectManagedTunnel()); }
    catch (error) { setProblem(error instanceof Error ? error.message : tr('Tunnel disconnect failed')); }
    finally { setTunnelConnectionBusy(false); }
  };

  const presetName = (presetId?: string) => presetId ? presets.data?.find((preset) => preset.id === presetId)?.name ?? tr('Saved permission profile') : tr('Custom permissions');

  return <div className="agents-page">
    <PageHeading eyebrow={tr('MCP ACCESS')} title={tr('Agents & Tunnels')} body={tr('Create access profiles for AI clients and choose how this device can be reached from the Internet.')} actions={<button className="button primary" onClick={() => setEditor('new')}><Plus />{tr('New access profile')}</button>} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />
    <div className="agents-tunnel-grid">
      <section className="agents-workspace-panel">
        <header className="agents-panel-header"><div><span className="eyebrow">{tr('AGENTS')}</span><h2>{tr('AI access profiles')}</h2><p>{tr('Each profile represents one AI client and defines exactly which tools that client is allowed to use.')}</p></div><span className="agents-panel-count">{agents.data?.length ?? 0}</span></header>
        {agents.loading ? <Loading label={tr('Loading agents')} /> : agents.error ? <ErrorState message={agents.error} retry={() => void agents.reload()} /> : !agents.data?.length ? <Empty title={tr('No access profiles yet')} body={tr('Create a profile to give an AI client controlled access to this device.')} /> : <div className="agent-list-grid">
          {agents.data.map((agent) => <article className="agent-list-card" key={agent.id}>
            <div className="agent-list-main">
              <div className="agent-list-identity">
                <span className="agent-avatar"><KeyRound /></span>
                <div><div className="agent-list-title"><strong>{agent.name}</strong><StatusBadge state={agent.enabled ? 'ready' : 'stopped'} label={agent.enabled ? tr('Enabled') : tr('Disabled')} /></div><code title={agent.id}>{agent.id}</code></div>
              </div>
              <div className="agent-list-meta">
                <div><span>{tr('Allowed tools')}</span><strong>{tr('{count} tools allowed', { count: agent.toolIds.length })}</strong></div>
                <div><span>{tr('Permission mode')}</span><strong>{presetName(agent.presetId)}</strong></div>
              </div>
            </div>
            <div className="agent-list-actions">
              <button className="button primary compact agent-plugin-button" onClick={() => setPluginAgent(agent)}><Link2 />{tr('Get connection link')}</button>
              <div className="agent-list-secondary-actions">
                <button className="button secondary compact" onClick={() => setEditor(agent)}><Pencil />{tr('Edit')}</button>
                <button className="button secondary compact" title={tr('Create a new access key. Existing connection links for this profile will stop working.')} onClick={() => void rotate(agent)}><RotateCw />{tr('Rotate token')}</button>
                <button className="icon-button danger-icon" aria-label={tr('Delete {name}', { name: agent.name })} title={tr('Delete {name}', { name: agent.name })} onClick={() => void remove(agent)}><Trash2 /></button>
              </div>
            </div>
          </article>)}
        </div>}
      </section>

      <section className="tunnel-panel">
        <header className="tunnel-panel-header"><div><span className="eyebrow">{tr('TUNNEL')}</span><h2>{tr('Public routes')}</h2><p>{tr('Connect this device to ChatCMD Tunnel, or keep using your own Cloudflare/domain routes.')}</p></div><button className="button secondary tunnel-add-button" onClick={() => setAddingTunnel(true)}><Plus />{tr('Add public address')}</button></header>
        <div className={`managed-tunnel-bar ${managedTunnelState ?? 'loading'}`}>
          <span className="managed-tunnel-icon" aria-hidden="true"><PlugZap /></span>
          <div className="managed-tunnel-copy">
            <div><strong>{tr('ChatCMD Tunnel')}</strong><span role="status" aria-live="polite" className={`managed-tunnel-state ${managedTunnelState ?? 'loading'}`}>{managedTunnelStateLabel}</span></div>
            {managedTunnel.data?.publicUrl ? <code title={managedTunnel.data.publicUrl}>{managedTunnel.data.publicUrl}</code> : <p>{tr('When connected, ChatCMD creates a public Internet address for this device so approved AI clients can reach it remotely.')}</p>}
            {managedTunnel.data?.key && <small>{tr('Tunnel key')}: <code>{managedTunnel.data.key}</code> · {tr('Server')}: {managedTunnel.data.serverUrl}</small>}
            {(managedTunnel.error || managedTunnel.data?.lastError) && <small className="managed-tunnel-error">{managedTunnel.error || managedTunnel.data?.lastError}</small>}
          </div>
          <button type="button" className={`button compact ${managedTunnelActive ? 'secondary' : 'primary'}`} disabled={managedTunnel.loading || tunnelConnectionBusy || !managedTunnel.data} aria-label={managedTunnelActive ? tr('Stop ChatCMD Tunnel') : tr('Connect ChatCMD Tunnel')} onClick={() => void (managedTunnelActive ? disconnectTunnel() : connectTunnel())}>
            {tunnelConnectionBusy || managedTunnelState === 'connecting' ? <LoaderCircle className="spin" aria-hidden="true" /> : managedTunnelActive ? <Power aria-hidden="true" /> : <PlugZap aria-hidden="true" />}
            {managedTunnelState === 'connecting' ? tr('Connecting…') : tunnelConnectionBusy ? tr('Stopping…') : managedTunnelActive ? tr('Disconnect') : tr('Connect')}
          </button>
        </div>
        <div className="custom-tunnel-heading"><div><strong>{tr('Your public addresses')}</strong><small>{tr('Use a domain, Cloudflare Tunnel, or public IP/port that you manage yourself.')}</small></div><span aria-label={tr('{count} public addresses', { count: tunnels.data?.length ?? 0 })}>{tunnels.data?.length ?? 0}</span></div>
        {tunnels.loading ? <div className="tunnel-panel-state"><LoaderCircle className="spin" />{tr('Loading public addresses')}</div> : tunnels.error ? <div className="tunnel-panel-state error"><span>{tunnels.error}</span><button className="button secondary compact" onClick={() => void tunnels.reload()}>{tr('Retry')}</button></div> : !tunnels.data?.length ? <div className="tunnel-empty"><span><Network /></span><strong>{tr('No custom addresses yet')}</strong><p>{tr('Add a public address that routes to ChatCMD on this device. It can then be used to create connection links for AI clients.')}</p><button className="button secondary" onClick={() => setAddingTunnel(true)}><Plus />{tr('Add public address')}</button></div> : <div className="tunnel-list">{tunnels.data.map((tunnel) => <article className="tunnel-row" key={tunnel.id}>
          <span className="tunnel-icon"><Globe2 /></span><div className="tunnel-copy"><strong>{tunnel.baseUrl}</strong><small>{tr('Connection check')}: {tunnel.baseUrl}/api/ping</small></div><div className="tunnel-row-actions">{testedTunnelId === tunnel.id && <span className="tunnel-ok" title={tr('Tunnel is reachable')}><CheckCircle2 /></span>}<button className="button secondary compact" disabled={testingTunnelId === tunnel.id} onClick={() => void testTunnel(tunnel)}>{testingTunnelId === tunnel.id ? <LoaderCircle className="spin" /> : <Wifi />}{testingTunnelId === tunnel.id ? tr('Testing…') : tr('Test')}</button></div>
        </article>)}</div>}
      </section>
    </div>

    {editor && <AgentEditor agent={editor === 'new' ? undefined : editor} tools={tools.data ?? []} presets={presets.data ?? []} close={() => setEditor(undefined)} save={save} />}
    {secret && <SecretModal result={secret} close={() => setSecret(undefined)} />}
    {addingTunnel && <AddTunnelModal close={() => setAddingTunnel(false)} added={tunnelAdded} />}
    {pluginAgent && <PluginLinksModal agent={pluginAgent} close={() => setPluginAgent(undefined)} onTestTunnel={testTunnel} testingTunnelId={testingTunnelId} />}
  </div>;
}

function AddTunnelModal({ close, added }: { close: () => void; added: (tunnel: Tunnel) => void }) {
  const [baseUrl, setBaseUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError('');
    try { added(await api.createTunnel(baseUrl)); }
    catch (reason) { setError(reason instanceof Error ? reason.message : tr('Tunnel could not be added')); }
    finally { setBusy(false); }
  };
  return <Modal title={tr('Add public address')} description={tr('ChatCMD will verify that this address reaches the current device before saving it.')} close={close}>
    <form className="form-stack" onSubmit={submit}>
      <div className="tunnel-guide"><span><Cloud /></span><div><strong>{tr('Use an address you control')}</strong><p>{tr('The address must route to ChatCMD running on this device. You can use a Cloudflare Tunnel, your own domain, or a public IP/port exposed through your router.')}</p><div className="tunnel-guide-examples"><code>https://mcp.example.com</code><code>http://203.0.113.10:8080</code></div></div></div>
      <label>{tr('Public address')}<input required autoFocus maxLength={512} placeholder="https://mcp.example.com" value={baseUrl} onChange={(event) => setBaseUrl(event.target.value)} /><small>{tr('Enter a domain, IP:port, or full URL. If no protocol is provided, ChatCMD will try HTTPS and verify the connection before saving.')}</small></label>
      {error && <div className="tunnel-form-error">{error}</div>}
      <div className="modal-actions"><button className="button secondary" type="button" onClick={close}>{tr('Cancel')}</button><button className="button primary" disabled={busy || !baseUrl.trim()}>{busy ? <LoaderCircle className="spin" /> : <Wifi />}{busy ? tr('Testing & saving…') : tr('Verify & add')}</button></div>
    </form>
  </Modal>;
}

function PluginLinksModal({ agent, close, onTestTunnel, testingTunnelId }: { agent: Agent; close: () => void; onTestTunnel: (tunnel: Tunnel) => Promise<void>; testingTunnelId?: number }) {
  const [links, setLinks] = useState<PluginLink[]>();
  const [error, setError] = useState('');
  const [copiedTunnelId, setCopiedTunnelId] = useState<number>();
  const [copyingTunnelId, setCopyingTunnelId] = useState<number>();
  useEffect(() => {
    let active = true;
    void api.pluginLinks(agent.id).then((value) => { if (active) setLinks(value); }).catch((reason) => { if (active) setError(reason instanceof Error ? reason.message : tr('Could not load plugin links')); });
    return () => { active = false; };
  }, [agent.id]);
  const copy = async (link: PluginLink) => {
    setCopyingTunnelId(link.tunnelId);
    setCopiedTunnelId(undefined);
    setError('');
    try {
      const result = await api.copyPluginLink(agent.id, link.tunnelId);
      await navigator.clipboard.writeText(result.endpoint);
      setCopiedTunnelId(link.tunnelId);
    } catch (reason) { setError(reason instanceof Error ? reason.message : tr('Copy failed')); }
    finally { setCopyingTunnelId(undefined); }
  };
  const test = async (link: PluginLink) => onTestTunnel({ id: link.tunnelId, baseUrl: link.baseUrl });
  return <Modal title={tr('Connection links — {name}', { name: agent.name })} description={tr('Choose the Internet address this AI client will use. Because the link contains an access key, the full value stays hidden until you copy it.')} close={close}>
    {error && <div className="tunnel-form-error">{error}</div>}
    {!links ? <div className="plugin-links-state"><LoaderCircle className="spin" />{tr('Preparing connection links…')}</div> : !links.length ? <div className="plugin-links-empty"><Network /><strong>{tr('No connection address available')}</strong><p>{tr('Connect ChatCMD Tunnel or add your own public address before creating a connection link.')}</p></div> : <div className="plugin-link-list">{links.map((link) => <article className="plugin-link-row" key={link.tunnelId}>
      <span className="plugin-link-icon"><Link2 /></span><div className="plugin-link-copy"><strong>{link.baseUrl}</strong><code>{link.maskedEndpoint}</code></div><div className="plugin-link-actions"><button className="button secondary compact" disabled={testingTunnelId === link.tunnelId} onClick={() => void test(link)}>{testingTunnelId === link.tunnelId ? <LoaderCircle className="spin" /> : <Wifi />}{tr('Test connection')}</button><button className="button primary compact" disabled={copyingTunnelId === link.tunnelId} onClick={() => void copy(link)}>{copyingTunnelId === link.tunnelId ? <LoaderCircle className="spin" /> : copiedTunnelId === link.tunnelId ? <CheckCircle2 /> : <Copy />}{copiedTunnelId === link.tunnelId ? tr('Copied') : tr('Copy')}</button></div>
    </article>)}</div>}
    <p className="plugin-link-security"><KeyRound />{tr('This connection link contains a secret access key. ChatCMD keeps it hidden on screen and copies the complete value directly to your clipboard.')}</p>
    <div className="modal-actions"><button className="button secondary" onClick={close}>{tr('Close')}</button></div>
  </Modal>;
}

function AgentEditor({ agent, tools, presets, close, save }: { agent?: Agent; tools: Awaited<ReturnType<typeof api.tools>>; presets: Awaited<ReturnType<typeof api.presets>>; close: () => void; save: (input: AgentInput) => Promise<void> }) {
  const [value, setValue] = useState<AgentInput>(agent ? { name: agent.name, enabled: agent.enabled, presetId: agent.presetId, toolIds: agent.toolIds } : blank);
  const [busy, setBusy] = useState(false);
  const groups = useMemo(() => {
    const map = new Map<string, Tool[]>();
    for (const tool of tools) { const key = tool.group || 'group-other'; map.set(key, [...(map.get(key) ?? []), tool]); }
    return [...map.entries()].sort(([a], [b]) => { const ai = groupOrder.indexOf(a); const bi = groupOrder.indexOf(b); return (ai < 0 ? 999 : ai) - (bi < 0 ? 999 : bi) || a.localeCompare(b); });
  }, [tools]);
  const setTools = (toolIds: string[]) => setValue({ ...value, presetId: undefined, toolIds: [...new Set(toolIds)] });
  const toggleAll = (checked: boolean) => setTools(checked ? tools.map((tool) => tool.id) : []);
  const toggleGroup = (groupTools: Tool[], checked: boolean) => { const groupIds = new Set(groupTools.map((tool) => tool.id)); setTools(checked ? [...value.toolIds, ...groupIds] : value.toolIds.filter((id) => !groupIds.has(id))); };
  const submit = async (event: FormEvent) => { event.preventDefault(); setBusy(true); await save(value); setBusy(false); };
  const allSelected = tools.length > 0 && tools.every((tool) => value.toolIds.includes(tool.id));
  return <Modal title={agent ? tr('Edit {name}', { name: agent.name }) : tr('Create access profile')} description={tr('These permissions apply only to the AI client that uses this profile.')} close={close}>
    <form className="form-stack" onSubmit={submit}>
      <label>{tr('Name')}<input required maxLength={100} value={value.name} onChange={(event) => setValue({ ...value, name: event.target.value })} /></label>
      <label>{tr('Permission preset')}<select value={value.presetId ?? ''} onChange={(event) => { const presetId = event.target.value || undefined; const preset = presets.find((item) => item.id === presetId); setValue({ ...value, presetId, toolIds: preset ? preset.toolIds : value.toolIds }); }}><option value="">{tr('Custom permissions')}</option>{presets.map((preset) => <option value={preset.id} key={preset.id}>{preset.name}</option>)}</select></label>
      <fieldset className="permission-fieldset">
        <div className="permission-heading"><legend>{tr('Per-tool permissions')}</legend><div><span>{tr('{selected}/{total} selected', { selected: value.toolIds.length, total: tools.length })}</span><button type="button" className="button secondary compact" onClick={() => toggleAll(!allSelected)}>{allSelected ? tr('Clear all') : tr('Select all')}</button></div></div>
        <div className="permission-groups">{groups.map(([group, groupTools]) => { const selected = groupTools.filter((tool) => value.toolIds.includes(tool.id)).length; const groupSelected = selected === groupTools.length; return <section className="permission-group" key={group}>
          <header><div><strong>{tr(groupNames[group] ?? 'Other')}</strong><small>{tr('{selected}/{total} selected', { selected, total: groupTools.length })}</small></div><button type="button" className="button secondary compact" onClick={() => toggleGroup(groupTools, !groupSelected)}>{groupSelected ? tr('Clear group') : tr('Select group')}</button></header>
          <div className="permission-list">{groupTools.map((tool) => <label className="check-row" key={tool.id}><input type="checkbox" checked={value.toolIds.includes(tool.id)} onChange={(event) => setTools(event.target.checked ? [...value.toolIds, tool.id] : value.toolIds.filter((id) => id !== tool.id))} /><span><strong>{tool.name}{tool.dangerous && <em>{tr('Dangerous')}</em>}</strong><small>{tool.description}</small></span></label>)}</div>
        </section>; })}</div>
      </fieldset>
      <label className="check-row"><input type="checkbox" checked={value.enabled} onChange={(event) => setValue({ ...value, enabled: event.target.checked })} /><span><strong>{tr('Allow connections')}</strong><small>{tr('When enabled, AI clients can use connection links from this profile. Turn it off to block access without deleting the profile.')}</small></span></label>
      <div className="modal-actions"><button className="button secondary" type="button" onClick={close}>{tr('Cancel')}</button><button className="button primary" disabled={busy}>{busy ? tr('Saving…') : tr('Save access profile')}</button></div>
    </form>
  </Modal>;
}

function SecretModal({ result, close }: { result: SecretResult; close: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => { await navigator.clipboard.writeText(result.endpoint); setCopied(true); };
  return <Modal title={tr('Save this connection link')} description={tr('Paste this link into the MCP configuration of the AI client you want to connect.')} close={close} dangerous><div className="secret-box"><code>{result.endpoint}</code><button className="button primary" onClick={() => void copy()}><Copy />{copied ? tr('Copied') : tr('Copy connection link')}</button></div><p className="warning-copy">{tr('This link contains a secret access key and is shown in full only this time. Anyone who has it can use the permissions of this access profile, so do not share it publicly or store it in source control or shared logs.')}</p><div className="modal-actions"><button className="button secondary" onClick={close}>{tr('I saved the connection link')}</button></div></Modal>;
}
