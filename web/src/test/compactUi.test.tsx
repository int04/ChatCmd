import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { api } from '../api';
import { chatGptExtensionStatus, dispatchChatGptRequest, resumeChatGptCompact } from '../chatgptBridge';
import { CompactProvider } from '../chatgpt/compact/CompactProvider';
import { CompactStatusCard } from '../chatgpt/compact/CompactControls';
import { compactConfirmation, compactSteps, type CompactPhase } from '../chatgpt/compact/types';
import { compactText } from '../chatgpt/compact/copy';
import { TasksPage } from '../pages/TasksPage';
import { setAppLanguage, tr } from '../i18n';
import type { TimelineEvent } from '../types';
import { compactBridge, compactJob, compactTask, compactTaskId, extensionReady, newUrl, nextRequest, oldUrl } from './compactFixtures';

const { listeners } = vi.hoisted(() => ({ listeners: new Set<(event: TimelineEvent) => void>() }));
vi.mock('../realtime', async () => {
  const { useEffect } = await import('react');
  return { useRealtime: (callback: (event: TimelineEvent) => void) => {
    useEffect(() => { listeners.add(callback); return () => { listeners.delete(callback); }; }, [callback]);
    return 'online';
  } };
});
vi.mock('../chatgptBridge', async (original) => ({
  ...await original<typeof import('../chatgptBridge')>(),
  chatGptExtensionStatus: vi.fn(), resumeChatGptCompact: vi.fn(), dispatchChatGptRequest: vi.fn(),
}));
vi.mock('../tasks/TaskAccessCard', () => ({ TaskAccessCard: () => <section data-testid="permission-card">Terminal permissions</section> }));
vi.mock('../tasks/TaskTerminalSection', () => ({ TaskTerminalSection: () => null }));
vi.mock('../tasks/SubagentApprovalQueue', () => ({ SubagentApprovalQueue: () => null }));

beforeEach(() => {
  vi.useFakeTimers(); setAppLanguage('vi', false);
  vi.mocked(chatGptExtensionStatus).mockReset().mockResolvedValue(extensionReady);
  vi.mocked(resumeChatGptCompact).mockReset().mockResolvedValue(undefined);
  vi.mocked(dispatchChatGptRequest).mockReset().mockResolvedValue(undefined);
  vi.spyOn(api, 'task').mockResolvedValue(compactTask);
  vi.spyOn(api, 'chatGptBridge').mockResolvedValue(compactBridge);
  vi.spyOn(api, 'chatGptCompact').mockResolvedValue({ active: null, history: [] });
  vi.spyOn(api, 'startChatGptCompact').mockResolvedValue(compactJob());
  vi.spyOn(api, 'chatGptCompactJob').mockResolvedValue(compactJob());
  vi.spyOn(api, 'cancelChatGptCompact').mockResolvedValue(compactJob({ phase: 'cancelled', revision: 2 }));
  vi.spyOn(api, 'chatGptQueue').mockResolvedValue([]);
  vi.spyOn(api, 'deleteChatGptQueuedMessage').mockResolvedValue(undefined);
  vi.spyOn(api, 'createChatGptQueuedMessage');
  vi.spyOn(api, 'sendChatGptMessage').mockResolvedValue(nextRequest);
  vi.spyOn(api, 'chatGptRequest').mockResolvedValue(nextRequest);
});
afterEach(() => { vi.restoreAllMocks(); setAppLanguage('en', false); });

function LocationProbe() { return <output data-testid="route">{useLocation().pathname}</output>; }
function mountTask() {
  return render(<MemoryRouter initialEntries={[`/tasks/${compactTaskId}`]}>
    <LocationProbe /><Routes><Route path="/tasks/:taskId" element={<TasksPage />} /></Routes>
  </MemoryRouter>);
}
async function flush() { await act(async () => { await Promise.resolve(); }); }
async function tick(ms = 2_000) { await act(async () => { await vi.advanceTimersByTimeAsync(ms); }); }
async function confirmCompact() {
  fireEvent.click(screen.getByRole('button', { name: 'Compact & resume now' }));
  fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: compactText('confirm') }));
  await flush();
}
function currentSteps() { return within(screen.getByRole('list', { name: 'Compact & resume steps' })).getAllByRole('listitem'); }

describe('Compact & Resume task UI', () => {
  it('requires the exact accessible confirmation and restores focus on cancel/Escape without any mutation', async () => {
    mountTask(); await flush();
    expect(screen.getByText(compactText('empty'))).toBeVisible();
    const trigger = screen.getByRole('button', { name: 'Compact & resume now' });
    trigger.focus(); fireEvent.click(trigger);
    const dialog = screen.getByRole('dialog', { name: 'Compact & resume now' });
    expect(dialog).toHaveAccessibleDescription(compactConfirmation);
    expect(within(dialog).getByText(compactConfirmation)).toBeVisible();
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(api.startChatGptCompact).not.toHaveBeenCalled();
    expect(resumeChatGptCompact).not.toHaveBeenCalled();
    fireEvent.click(within(dialog).getByRole('button', { name: tr('Cancel') }));
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(trigger).toHaveFocus();
    fireEvent.click(trigger); fireEvent.keyDown(document, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(api.startChatGptCompact).not.toHaveBeenCalled();
    expect(resumeChatGptCompact).not.toHaveBeenCalled();
  });

  it('shows all four steps above input and locks conflicting actions while retaining the draft', async () => {
    mountTask(); await flush();
    const input = screen.getByRole('textbox', { name: tr('Next message to ChatGPT') });
    fireEvent.change(input, { target: { value: 'My preserved draft' } });
    await confirmCompact();
    expect(api.startChatGptCompact).toHaveBeenCalledExactlyOnceWith(compactTaskId, false);
    expect(resumeChatGptCompact).toHaveBeenCalledExactlyOnceWith('compact-test-job', compactTaskId);
    const heading = screen.getByRole('heading', { name: 'ChatGPT is writing the handoff' });
    expect(heading.compareDocumentPosition(input) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(currentSteps()).toHaveLength(4);
    compactSteps.forEach((step, index) => expect(currentSteps()[index]).toHaveTextContent(step.label));
    expect(currentSteps()[0]).toHaveAttribute('aria-current', 'step');
    expect(screen.getByRole('region', { name: 'Compact & resume progress' }).querySelector('[aria-live="polite"]')).not.toBeNull();
    for (const name of ['Compact & resume now', tr('Send'), tr('Close this tab'), tr('Change model'), tr('Queue another message'), tr('Send immediate message')]) {
      expect(screen.getByRole('button', { name })).toBeDisabled();
    }
    expect(input).toHaveValue('My preserved draft'); expect(input).toBeDisabled();
    fireEvent.submit(input.closest('form')!);
    expect(api.sendChatGptMessage).not.toHaveBeenCalled();
    expect(api.deleteChatGptQueuedMessage).not.toHaveBeenCalled();
  });

  it('polls completion, refreshes both bridge URLs, and sends the unchanged draft in the same task', async () => {
    mountTask(); await flush();
    const input = screen.getByRole('textbox', { name: tr('Next message to ChatGPT') });
    fireEvent.change(input, { target: { value: 'My preserved draft' } });
    await confirmCompact();
    const completed = compactJob({ phase: 'completed', revision: 5, newConversationId: 'new-chat', newConversationUrl: newUrl });
    vi.mocked(api.chatGptCompactJob).mockResolvedValue(completed);
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: null, history: [completed, completed] });
    // Backend completion arrives before the bridge GET catches up: sending must remain locked.
    await tick();
    expect(input).toBeDisabled(); expect(input).toHaveValue('My preserved draft');
    vi.mocked(api.chatGptBridge).mockResolvedValue({ ...compactBridge, conversationId: 'new-chat', conversationUrl: newUrl });
    await tick();
    expect(input).toBeEnabled(); expect(input).toHaveValue('My preserved draft');
    expect(screen.getByTestId('route')).toHaveTextContent(`/tasks/${compactTaskId}`);
    expect(screen.getByRole('link', { name: tr('Open original conversation') })).toHaveAttribute('href', newUrl);
    expect(screen.queryByRole('heading', { name: 'ChatGPT is writing the handoff' })).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'Compact & resume progress' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: tr('Send') })); await flush();
    expect(api.sendChatGptMessage).toHaveBeenCalledExactlyOnceWith(compactTaskId, { model: 'Auto', content: 'My preserved draft' });
    expect(dispatchChatGptRequest).toHaveBeenCalledWith(expect.objectContaining({ requestId: 'next-request', conversationUrl: newUrl }));
    expect(api.task).toHaveBeenCalledWith(compactTaskId);
    expect(screen.getByTestId('route')).toHaveTextContent(`/tasks/${compactTaskId}`);
  });

  it('renders deduplicated old-reference history directly below terminal permissions without task navigation', async () => {
    const completed = compactJob({ phase: 'completed', revision: 5, newConversationId: 'new-chat', newConversationUrl: newUrl });
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: null, history: [completed, completed] });
    mountTask(); await flush();
    const history = screen.getByRole('region', { name: 'Lịch sử thu gọn ngữ cảnh' });
    expect(screen.getByTestId('permission-card').nextElementSibling).toBe(history);
    expect(within(history).getAllByRole('listitem')).toHaveLength(1);
    const link = within(history).getByRole('link', { name: new RegExp(compactText('reference')) });
    expect(link).toHaveAttribute('href', oldUrl); expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(history.querySelector('time')).toHaveAttribute('datetime', new Date(completed.createdAtMs).toISOString());
    fireEvent.click(link);
    expect(screen.getByTestId('route')).toHaveTextContent(`/tasks/${compactTaskId}`);
    fireEvent.click(within(history).getByText(compactText('details')));
    expect(within(history).getByText('old-chat')).toBeInTheDocument();
    expect(within(history).getByText('new-chat')).toBeInTheDocument();
    expect(within(history).getByText('Thinking')).toBeInTheDocument();
    expect(api.startChatGptCompact).not.toHaveBeenCalled();
  });

  it('recovers a durable job after remount and exposes missing-extension retry and CAS cancellation', async () => {
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob({ phase: 'writing_handoff', revision: 3 }), history: [] });
    vi.mocked(chatGptExtensionStatus).mockResolvedValue({ ...extensionReady, ready: false, conversationTabOpen: false });
    vi.mocked(resumeChatGptCompact).mockRejectedValue(new Error('extension unavailable'));
    const view = mountTask(); await flush();
    expect(screen.getByText(/Tiến trình đã lưu nhưng extension chưa xác nhận/)).toBeVisible();
    expect(screen.getByRole('button', { name: compactText('resume') })).toBeEnabled();
    expect(api.startChatGptCompact).not.toHaveBeenCalled();
    view.unmount(); mountTask(); await flush();
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(2);
    expect(currentSteps()[1]).toHaveAttribute('aria-current', 'step');
    vi.mocked(resumeChatGptCompact).mockResolvedValue(undefined);
    fireEvent.click(screen.getByRole('button', { name: compactText('resume') })); await flush();
    await tick(10_000);
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(3);
    vi.mocked(api.cancelChatGptCompact).mockResolvedValue(compactJob({ phase: 'cancelled', revision: 4 }));
    fireEvent.click(screen.getByRole('button', { name: compactText('cancelJob') })); await flush();
    expect(api.cancelChatGptCompact).toHaveBeenCalledExactlyOnceWith('compact-test-job', 3);
    expect(screen.queryByRole('button', { name: compactText('cancelJob') })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Compact & resume now' })).toBeEnabled();
  });

  it('refreshes active state on realtime/reconnect and leaves unrelated task events alone', async () => {
    mountTask(); await flush();
    const baseline = vi.mocked(api.chatGptCompact).mock.calls.length;
    await act(async () => { listeners.forEach((listener) => listener({ id: 'unrelated', type: 'chatgpt_compact_updated', taskId: 'another-task', occurredAt: '' })); });
    expect(api.chatGptCompact).toHaveBeenCalledTimes(baseline);
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    await act(async () => { listeners.forEach((listener) => listener({ id: 'own-update', type: 'chatgpt_compact_updated', taskId: compactTaskId, occurredAt: '' })); });
    expect(screen.getByRole('heading', { name: 'ChatGPT is writing the handoff' })).toBeVisible();
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(1);
    await tick(10_000);
    await act(async () => { listeners.forEach((listener) => listener({ id: 'connected', type: 'system.connected', occurredAt: '' })); });
    expect(resumeChatGptCompact).toHaveBeenCalledTimes(2);
  });

  it('preserves queued messages and an open queue draft through an externally started compact and extension disconnect', async () => {
    vi.mocked(chatGptExtensionStatus).mockResolvedValue({ ...extensionReady, conversationReady: false });
    vi.mocked(api.chatGptQueue).mockResolvedValue([{
      id: 'queued-one', taskId: compactTaskId, content: 'Already queued', mode: 'queued',
      sortOrder: 0, createdAtMs: 1, updatedAtMs: 1,
    }]);
    mountTask(); await flush();
    fireEvent.click(screen.getByRole('button', { name: tr('Queue another message') }));
    const queueDialog = screen.getByRole('dialog');
    const draft = within(queueDialog).getByRole('textbox');
    fireEvent.change(draft, { target: { value: 'Unsubmitted queue draft' } });
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: compactJob(), history: [] });
    await act(async () => { listeners.forEach((listener) => listener({ id: 'external-start', type: 'chatgpt_compact_updated', taskId: compactTaskId, occurredAt: '' })); });
    vi.mocked(chatGptExtensionStatus).mockResolvedValue({ ...extensionReady, ready: false, conversationTabOpen: false });
    await tick();
    expect(screen.getByRole('dialog')).toBe(queueDialog);
    expect(draft).toHaveValue('Unsubmitted queue draft');
    expect(within(queueDialog).getByRole('button', { name: tr('Add to queue') })).toBeDisabled();
    expect(screen.getByText('Already queued')).toBeInTheDocument();
    expect(api.sendChatGptMessage).not.toHaveBeenCalled();
    expect(api.deleteChatGptQueuedMessage).not.toHaveBeenCalled();
    expect(api.createChatGptQueuedMessage).not.toHaveBeenCalled();
  });

  it('does not unlock sends when compact state is initially unavailable', async () => {
    vi.mocked(api.chatGptCompact).mockRejectedValue(new Error('offline'));
    mountTask(); await flush();
    expect(screen.getByRole('button', { name: 'Compact & resume now' })).toBeDisabled();
    expect(screen.getByRole('button', { name: tr('Send') })).toBeDisabled();
    expect(screen.getAllByText(/offline/).length).toBeGreaterThan(0);
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: null, history: [] });
    fireEvent.click(within(screen.getByRole('region', { name: 'Compact & resume error' })).getByRole('button', { name: tr('Retry') }));
    await flush();
    expect(screen.getByRole('button', { name: 'Compact & resume now' })).toBeEnabled();
  });

  it.each<CompactPhase>(['preparing', 'writing_handoff', 'saving_handoff', 'opening_new_chat', 'completed', 'cancelled'])('renders %s with correct step/status and waiting detail', async (phase) => {
    const job = compactJob({ phase, detail: 'A durable server detail' });
    vi.mocked(api.chatGptCompact).mockResolvedValue({ active: ['completed', 'cancelled'].includes(phase) ? null : job, history: [job] });
    render(<CompactProvider taskId={compactTaskId}><CompactStatusCard /></CompactProvider>); await flush();
    if (phase === 'completed' || phase === 'cancelled') {
      expect(screen.queryByRole('region', { name: 'Compact & resume progress' })).not.toBeInTheDocument();
      expect(screen.queryByText('ChatGPT is writing the handoff')).not.toBeInTheDocument();
      return;
    }
    const index = compactSteps.findIndex((step) => step.phase === phase);
    currentSteps().forEach((step, i) => {
      if (i === index) expect(step).toHaveAttribute('aria-current', 'step');
      else expect(step).not.toHaveAttribute('aria-current');
    });
    expect(screen.getByRole('status')).toHaveTextContent('A durable server detail');
    expect(currentSteps()).toHaveLength(4);
  });
});

it('continuation checkbox is opt-in for each confirmation and is never submitted by cancelling', async () => {
  mountTask(); await flush();
  expect(screen.queryByRole('heading', { name: 'ChatGPT is writing the handoff' })).not.toBeInTheDocument();
  const trigger = screen.getByRole('button', { name: 'Compact & resume now' });
  fireEvent.click(trigger);
  let checkbox = screen.getByRole('checkbox', { name: compactText('continueAfterCompact') });
  expect(checkbox).not.toBeChecked();
  fireEvent.click(checkbox);
  fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: tr('Cancel') }));
  expect(api.startChatGptCompact).not.toHaveBeenCalled();
  fireEvent.click(trigger);
  checkbox = screen.getByRole('checkbox', { name: compactText('continueAfterCompact') });
  expect(checkbox).not.toBeChecked();
  fireEvent.click(checkbox);
  fireEvent.click(within(screen.getByRole('dialog')).getByRole('button', { name: compactText('confirm') }));
  await flush();
  expect(api.startChatGptCompact).toHaveBeenCalledExactlyOnceWith(compactTaskId, true);
});
