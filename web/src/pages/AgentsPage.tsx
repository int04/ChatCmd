import { CheckCircle2, Cloud, Copy, Globe2, KeyRound, Link2, LoaderCircle, Network, Pencil, Plus, RotateCw, Trash2, Wifi } from 'lucide-react';
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
  const tools = useLoad(api.tools, []);
  const presets = useLoad(api.presets, []);
  const settings = useLoad(api.settings, []);
  const [editor, setEditor] = useState<Agent | 'new'>();
  const [secret, setSecret] = useState<SecretResult>();
  const [pluginAgent, setPluginAgent] = useState<Agent>();
  const [rotateTarget, setRotateTarget] = useState<Agent>();
  const [rotatingAgentId, setRotatingAgentId] = useState<string>();
  const [addingTunnel, setAddingTunnel] = useState(false);
  const [testingTunnelId, setTestingTunnelId] = useState<number>();
  const [testedTunnelId, setTestedTunnelId] = useState<number>();
  const [deleteTunnelTarget, setDeleteTunnelTarget] = useState<Tunnel>();
  const [deletingTunnelId, setDeletingTunnelId] = useState<number>();
  const [problem, setProblem] = useState('');

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
  const rotate = async () => {
    if (!rotateTarget) return;
    const agent = rotateTarget;
    setProblem('');
    setRotatingAgentId(agent.id);
    try {
      setSecret(await api.rotateAgentSecret(agent.id));
      setRotateTarget(undefined);
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Rotation failed')); }
    finally { setRotatingAgentId(undefined); }
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
  const deleteTunnel = async () => {
    if (!deleteTunnelTarget) return;
    const id = deleteTunnelTarget.id;
    setProblem('');
    setDeletingTunnelId(id);
    try {
      await api.deleteTunnel(id);
      tunnels.setData((current) => current?.filter((item) => item.id !== id));
      if (testedTunnelId === id) setTestedTunnelId(undefined);
      setDeleteTunnelTarget(undefined);
    } catch (error) { setProblem(error instanceof Error ? error.message : tr('Tunnel could not be deleted')); }
    finally { setDeletingTunnelId(undefined); }
  };
  const presetName = (presetId?: string) => presetId ? presets.data?.find((preset) => preset.id === presetId)?.name ?? tr('Saved permission profile') : tr('Custom permissions');

  return <div className="agents-page agents-simple-page agents-clean-page">
    <PageHeading eyebrow={tr('AI ACCESS')} title={tr('Plugin list')} body={tr('Connect this device once, then choose which AI clients are allowed to use it.')} actions={<button className="button primary" onClick={() => setEditor('new')}><Plus />{tr('Create new Plugin connection')}</button>} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />

    <section className="agents-clean-access">
      <header className="agents-clean-section-header">
        <div><h2>{tr('Created Plugin list')}</h2><p>{tr('A list of created Plugins that can be added to AI models with MCP support to communicate with your computer.')}</p></div>
        <span>{agents.data?.length ?? 0}</span>
      </header>
      {agents.loading ? <Loading label={tr('Loading agents')} /> : agents.error ? <ErrorState message={agents.error} retry={() => void agents.reload()} /> : !agents.data?.length ? <div className="agents-clean-empty">
        <span><KeyRound /></span><div><strong>{tr('No AI has access yet')}</strong><p>{tr('Add an AI client, choose what it is allowed to do, then copy its connection link.')}</p></div><button className="button primary" onClick={() => setEditor('new')}><Plus />{tr('Create new Plugin connection')}</button>
      </div> : <div className="agents-clean-list">
        {agents.data.map((agent) => <article className="agents-clean-row" key={agent.id}>
          <span className="agent-avatar"><KeyRound /></span>
          <div className="agents-clean-row-copy"><div><strong>{agent.name}</strong><StatusBadge state={agent.enabled ? 'ready' : 'stopped'} label={agent.enabled ? tr('Allowed') : tr('Blocked')} /></div><p>{tr('{count} permissions', { count: agent.toolIds.length })} · {presetName(agent.presetId)}</p></div>
          <div className="agents-clean-row-actions">
            <button className="button primary" onClick={() => setPluginAgent(agent)}><Link2 />{tr('Copy connection link')}</button>
            <button className="button secondary" onClick={() => setEditor(agent)}><Pencil />{tr('Permissions')}</button>
            <details className="agents-clean-more"><summary className="icon-button" aria-label={tr('More options')}>•••</summary><div><button type="button" onClick={() => setRotateTarget(agent)}><RotateCw />{tr('Create new access code')}</button><button type="button" className="danger" onClick={() => void remove(agent)}><Trash2 />{tr('Delete')}</button></div></details>
          </div>
        </article>)}
      </div>}
    </section>

    <details className="agents-clean-advanced" open>
      <summary><span><Network />{tr('Custom Tunnel / private domain')}</span><small>{tr('For custom domains, Cloudflare Tunnel, or public IP addresses')}</small></summary>
      <div className="agents-clean-advanced-body">
        <div className="agents-clean-advanced-heading"><div><strong>{tr('Custom public addresses')}</strong><p>{tr('You can configure your own tunnel server or add tunnels from Cloudflare and similar services so AI models such as ChatGPT or Grok can access your personal computer.')}</p></div><button className="button secondary" onClick={() => setAddingTunnel(true)}><Plus />{tr('Add new domain / Tunnel')}</button></div>
        {tunnels.loading ? <div className="tunnel-panel-state"><LoaderCircle className="spin" />{tr('Loading public addresses')}</div> : tunnels.error ? <div className="tunnel-panel-state error"><span>{tunnels.error}</span><button className="button secondary compact" onClick={() => void tunnels.reload()}>{tr('Retry')}</button></div> : !tunnels.data?.length ? <div className="agents-clean-no-custom">{tr('No custom addresses')}</div> : <div className="tunnel-list">{tunnels.data.map((tunnel) => <article className="tunnel-row" key={tunnel.id}>
          <span className="tunnel-icon"><Globe2 /></span><div className="tunnel-copy"><strong>{tunnel.baseUrl}</strong><small>{tr('Custom connection address')}</small></div><div className="tunnel-row-actions">{testedTunnelId === tunnel.id && <span className="tunnel-ok"><CheckCircle2 /></span>}<button className="button secondary compact" disabled={testingTunnelId === tunnel.id || deletingTunnelId === tunnel.id} onClick={() => void testTunnel(tunnel)}>{testingTunnelId === tunnel.id ? <LoaderCircle className="spin" /> : <Wifi />}{tr('Test')}</button><button className="icon-button danger-icon" disabled={deletingTunnelId === tunnel.id} onClick={() => setDeleteTunnelTarget(tunnel)} aria-label={tr('Delete public address {address}', { address: tunnel.baseUrl })}>{deletingTunnelId === tunnel.id ? <LoaderCircle className="spin" /> : <Trash2 />}</button></div>
        </article>)}</div>}
      </div>
    </details>

    {editor && <AgentEditor agent={editor === 'new' ? undefined : editor} tools={tools.data ?? []} presets={presets.data ?? []} close={() => setEditor(undefined)} save={save} />}
    {secret && <SecretModal result={secret} close={() => setSecret(undefined)} />}
    {rotateTarget && <Modal title={tr('Create new access code?')} description={rotateTarget.name} close={() => !rotatingAgentId && setRotateTarget(undefined)} dangerous>
      <div className="warning-copy">{tr('After confirmation, you must update this Plugin link on your AI models before it can be used again. Confirm?')}</div>
      <div className="modal-actions"><button className="button secondary" type="button" disabled={Boolean(rotatingAgentId)} onClick={() => setRotateTarget(undefined)}>{tr('Cancel')}</button><button className="button danger" type="button" disabled={Boolean(rotatingAgentId)} onClick={() => void rotate()}>{rotatingAgentId ? <LoaderCircle className="spin" /> : <RotateCw />}{rotatingAgentId ? tr('Creating…') : tr('Confirm')}</button></div>
    </Modal>}
    {addingTunnel && <AddTunnelModal port={settings.data?.port} close={() => setAddingTunnel(false)} added={tunnelAdded} />}
    {deleteTunnelTarget && <Modal title={tr('Delete public address?')} description={deleteTunnelTarget.baseUrl} close={() => !deletingTunnelId && setDeleteTunnelTarget(undefined)} dangerous>
      <div className="warning-copy">{tr('This address will be removed from the saved Tunnel list and will no longer be available when generating connection links.')}</div>
      <div className="modal-actions"><button className="button secondary" type="button" disabled={Boolean(deletingTunnelId)} onClick={() => setDeleteTunnelTarget(undefined)}>{tr('Cancel')}</button><button className="button danger" type="button" disabled={Boolean(deletingTunnelId)} onClick={() => void deleteTunnel()}>{deletingTunnelId ? <LoaderCircle className="spin" /> : <Trash2 />}{deletingTunnelId ? tr('Deleting…') : tr('Delete public address')}</button></div>
    </Modal>}
    {pluginAgent && <PluginLinksModal agent={pluginAgent} close={() => setPluginAgent(undefined)} onTestTunnel={testTunnel} testingTunnelId={testingTunnelId} />}
  </div>;
}

function AddTunnelModal({ port, close, added }: { port?: number; close: () => void; added: (tunnel: Tunnel) => void }) {
  const [baseUrl, setBaseUrl] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError('');
    const value = baseUrl.trim();
    try {
      const parsed = new URL(value.includes('://') ? value : `https://${value}`);
      if (parsed.hostname.toLowerCase() === 'localhost' || parsed.hostname === '127.0.0.1') {
        setError(tr('localhost and 127.0.0.1 cannot be added as public addresses'));
        return;
      }
    } catch {
      // Let the backend return the existing validation message for malformed addresses.
    }
    setBusy(true);
    try { added(await api.createTunnel(value)); }
    catch (reason) { setError(reason instanceof Error ? reason.message : tr('Tunnel could not be added')); }
    finally { setBusy(false); }
  };
  return <Modal title={tr('Add public address')} description={tr('ChatCMD will verify that this address reaches the current device before saving it.')} close={close}>
    <form className="form-stack" onSubmit={submit}>
      <div className="tunnel-guide"><span><Cloud /></span><div><strong>{tr('Use an address you control')}</strong><div className="tunnel-port-highlight"><small>{tr('Rust server port')}</small><strong>{port ?? '—'}</strong></div><p>{tr('Point your domain, reverse proxy, Cloudflare Tunnel, or router port forwarding to the ChatCMD Rust server running on this device. The current local server port is {port}.', { port: port ?? '—' })}</p><p>{tr('For Cloudflare Tunnel, use a service target such as http://127.0.0.1:{port}. For router/NAT forwarding, forward an external TCP port to this computer on port {port}, then enter the public domain or IP here.', { port: port ?? '—' })}</p><div className="tunnel-guide-examples"><code>https://mcp.example.com</code><code>{`http://203.0.113.10:${port ?? 'PORT'}`}</code></div></div></div>
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
    {!links ? <div className="plugin-links-state"><LoaderCircle className="spin" />{tr('Preparing connection links…')}</div> : !links.length ? <div className="plugin-links-empty"><Network /><strong>{tr('No connection address available')}</strong><p>{tr('Add your own public address before creating a connection link.')}</p></div> : <div className="plugin-link-list">{links.map((link) => <article className="plugin-link-row" key={link.tunnelId}>
      <span className="plugin-link-icon"><Link2 /></span><div className="plugin-link-copy"><strong>{link.baseUrl}</strong><code>{link.maskedEndpoint}</code></div><div className="plugin-link-actions"><button className="button secondary compact" disabled={testingTunnelId === link.tunnelId} onClick={() => void test(link)}>{testingTunnelId === link.tunnelId ? <LoaderCircle className="spin" /> : <Wifi />}{tr('Test connection')}</button><button className="button primary compact" disabled={copyingTunnelId === link.tunnelId} onClick={() => void copy(link)}>{copyingTunnelId === link.tunnelId ? <LoaderCircle className="spin" /> : copiedTunnelId === link.tunnelId ? <CheckCircle2 /> : <Copy />}{copiedTunnelId === link.tunnelId ? tr('Copied') : tr('Copy')}</button></div>
    </article>)}</div>}
    <p className="plugin-link-security"><KeyRound />{tr('This connection link contains a secret access key. ChatCMD keeps it hidden on screen and copies the complete value directly to your clipboard.')}</p>
    <div className="modal-actions"><button className="button secondary" onClick={close}>{tr('Close')}</button></div>
  </Modal>;
}

function AgentEditor({ agent, tools, presets, close, save }: { agent?: Agent; tools: Awaited<ReturnType<typeof api.tools>>; presets: Awaited<ReturnType<typeof api.presets>>; close: () => void; save: (input: AgentInput) => Promise<void> }) {
  const [value, setValue] = useState<AgentInput>(() => agent ? { name: agent.name, enabled: agent.enabled, presetId: agent.presetId, toolIds: agent.toolIds } : { ...blank, toolIds: tools.map((tool) => tool.id) });
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
  return <Modal title={agent ? tr('Edit {name}', { name: agent.name }) : tr('Create new Plugin connection')} description={tr('These permissions apply only to the AI client that uses this profile.')} close={close}>
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
