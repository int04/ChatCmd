import { Copy, KeyRound, Pencil, Plus, RotateCw, Trash2 } from 'lucide-react';
import { FormEvent, useMemo, useState } from 'react';
import { api } from '../api';
import { Empty, ErrorState, Loading, Modal, PageHeading, ProblemBanner, StatusBadge } from '../components';
import type { Agent, AgentInput, SecretResult, Tool } from '../types';
import { useLoad } from '../useLoad';

const blank: AgentInput = { name: '', enabled: true, projectFolder: '', toolIds: [] };
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
  const tools = useLoad(api.tools, []);
  const presets = useLoad(api.presets, []);
  const [editor, setEditor] = useState<Agent | 'new'>();
  const [secret, setSecret] = useState<SecretResult>();
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
    } catch (error) {
      setProblem(error instanceof Error ? error.message : 'Save failed');
    }
  };
  const rotate = async (agent: Agent) => {
    setProblem('');
    try { setSecret(await api.rotateAgentSecret(agent.id)); }
    catch (error) { setProblem(error instanceof Error ? error.message : 'Rotation failed'); }
  };
  const remove = async (agent: Agent) => {
    if (!confirm(`Delete local agent “${agent.name}”?`)) return;
    try {
      await api.deleteAgent(agent.id);
      agents.setData((current) => current?.filter((item) => item.id !== agent.id));
    } catch (error) {
      setProblem(error instanceof Error ? error.message : 'Delete failed');
    }
  };
  return <div>
    <PageHeading eyebrow="MCP ACCESS" title="Agents" body="Local clients, project scope, and tool permissions." actions={<button className="button primary" onClick={() => setEditor('new')}><Plus />New agent</button>} />
    <ProblemBanner message={problem} clear={() => setProblem('')} />
    {agents.loading ? <Loading label="Loading agents" /> : agents.error ? <ErrorState message={agents.error} retry={() => void agents.reload()} /> : !agents.data?.length ? <Empty title="No MCP agents" body="Create an agent to receive a one-time MCP connection URL." /> : <div className="agent-grid">{agents.data.map((agent) => <article className="agent-card" key={agent.id}>
      <header><span className="agent-avatar"><KeyRound /></span><div><strong>{agent.name}</strong><code>{agent.id}</code></div><StatusBadge state={agent.enabled ? 'ready' : 'stopped'} label={agent.enabled ? 'Enabled' : 'Disabled'} /></header>
      <dl><div><dt>Project folder</dt><dd>{agent.projectFolder || 'All configured roots'}</dd></div><div><dt>Tool access</dt><dd>{agent.toolIds.length} selected{agent.presetId ? ` · preset ${agent.presetId}` : ''}</dd></div><div><dt>Token</dt><dd>{agent.secretLast4 ? `•••• ${agent.secretLast4}` : 'Hidden'}</dd></div></dl>
      <div className="button-row"><button className="button secondary compact" onClick={() => setEditor(agent)}><Pencil />Edit</button><button className="button secondary compact" onClick={() => void rotate(agent)}><RotateCw />Rotate token</button><button className="icon-button danger-icon" aria-label={`Delete ${agent.name}`} onClick={() => void remove(agent)}><Trash2 /></button></div>
    </article>)}</div>}
    {editor && <AgentEditor agent={editor === 'new' ? undefined : editor} tools={tools.data ?? []} presets={presets.data ?? []} close={() => setEditor(undefined)} save={save} />}
    {secret && <SecretModal result={secret} close={() => setSecret(undefined)} />}
  </div>;
}

function AgentEditor({ agent, tools, presets, close, save }: { agent?: Agent; tools: Awaited<ReturnType<typeof api.tools>>; presets: Awaited<ReturnType<typeof api.presets>>; close: () => void; save: (input: AgentInput) => Promise<void> }) {
  const [value, setValue] = useState<AgentInput>(agent ? { name: agent.name, enabled: agent.enabled, projectFolder: agent.projectFolder, presetId: agent.presetId, toolIds: agent.toolIds } : blank);
  const [busy, setBusy] = useState(false);
  const groups = useMemo(() => {
    const map = new Map<string, Tool[]>();
    for (const tool of tools) {
      const key = tool.group || 'group-other';
      map.set(key, [...(map.get(key) ?? []), tool]);
    }
    return [...map.entries()].sort(([a], [b]) => {
      const ai = groupOrder.indexOf(a); const bi = groupOrder.indexOf(b);
      return (ai < 0 ? 999 : ai) - (bi < 0 ? 999 : bi) || a.localeCompare(b);
    });
  }, [tools]);
  const setTools = (toolIds: string[]) => setValue({ ...value, presetId: undefined, toolIds: [...new Set(toolIds)] });
  const toggleAll = (checked: boolean) => setTools(checked ? tools.map((tool) => tool.id) : []);
  const toggleGroup = (groupTools: Tool[], checked: boolean) => {
    const groupIds = new Set(groupTools.map((tool) => tool.id));
    setTools(checked ? [...value.toolIds, ...groupIds] : value.toolIds.filter((id) => !groupIds.has(id)));
  };
  const submit = async (event: FormEvent) => { event.preventDefault(); setBusy(true); await save(value); setBusy(false); };
  const allSelected = tools.length > 0 && tools.every((tool) => value.toolIds.includes(tool.id));
  return <Modal title={agent ? `Edit ${agent.name}` : 'Create MCP agent'} description="Permissions apply only to this local endpoint." close={close}>
    <form className="form-stack" onSubmit={submit}>
      <label>Name<input required maxLength={100} value={value.name} onChange={(event) => setValue({ ...value, name: event.target.value })} /></label>
      <label>Project folder<input value={value.projectFolder} onChange={(event) => setValue({ ...value, projectFolder: event.target.value })} placeholder="D:\Projects\example" /></label>
      <label>Permission preset<select value={value.presetId ?? ''} onChange={(event) => {
        const presetId = event.target.value || undefined;
        const preset = presets.find((item) => item.id === presetId);
        setValue({ ...value, presetId, toolIds: preset ? preset.toolIds : value.toolIds });
      }}><option value="">Custom permissions</option>{presets.map((preset) => <option value={preset.id} key={preset.id}>{preset.name}</option>)}</select></label>
      <fieldset className="permission-fieldset">
        <div className="permission-heading"><legend>Per-tool permissions</legend><div><span>{value.toolIds.length}/{tools.length} selected</span><button type="button" className="button secondary compact" onClick={() => toggleAll(!allSelected)}>{allSelected ? 'Clear all' : 'Select all'}</button></div></div>
        <div className="permission-groups">{groups.map(([group, groupTools]) => {
          const selected = groupTools.filter((tool) => value.toolIds.includes(tool.id)).length;
          const groupSelected = selected === groupTools.length;
          return <section className="permission-group" key={group}>
            <header><div><strong>{groupNames[group] ?? group.replace(/^group-/, '')}</strong><small>{selected}/{groupTools.length} selected</small></div><button type="button" className="button secondary compact" onClick={() => toggleGroup(groupTools, !groupSelected)}>{groupSelected ? 'Clear group' : 'Select group'}</button></header>
            <div className="permission-list">{groupTools.map((tool) => <label className="check-row" key={tool.id}><input type="checkbox" checked={value.toolIds.includes(tool.id)} onChange={(event) => setTools(event.target.checked ? [...value.toolIds, tool.id] : value.toolIds.filter((id) => id !== tool.id))} /><span><strong>{tool.name}{tool.dangerous && <em>Dangerous</em>}</strong><small>{tool.description}</small></span></label>)}</div>
          </section>;
        })}</div>
      </fieldset>
      <label className="check-row"><input type="checkbox" checked={value.enabled} onChange={(event) => setValue({ ...value, enabled: event.target.checked })} /><span><strong>Enabled</strong><small>Allow this client to use its tokenized MCP URL.</small></span></label>
      <div className="modal-actions"><button className="button secondary" type="button" onClick={close}>Cancel</button><button className="button primary" disabled={busy}>{busy ? 'Saving…' : 'Save agent'}</button></div>
    </form>
  </Modal>;
}

function SecretModal({ result, close }: { result: SecretResult; close: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => { await navigator.clipboard.writeText(result.endpoint); setCopied(true); };
  return <Modal title="Save this MCP URL now" description="Use this URL directly in the MCP client. No Authorization header is required." close={close} dangerous><div className="secret-box"><code>{result.endpoint}</code><button className="button primary" onClick={() => void copy()}><Copy />{copied ? 'Copied' : 'Copy MCP URL'}</button></div><p className="warning-copy">The final path segment is the agent token and is shown only once. Keep the complete URL out of source control, screenshots, browser history, and shared logs.</p><div className="modal-actions"><button className="button secondary" onClick={close}>I saved the MCP URL</button></div></Modal>;
}
