export function formatToolOutput(tool: string, output: unknown): string {
  if (output === null || output === undefined) return '';
  if (typeof output === 'string') return output;
  const special = formatKnownTool(tool, output);
  return special || formatStructured(output);
}

function formatKnownTool(tool: string, output: unknown): string {
  const value = asObject(output);
  switch (tool) {
    case 'fs_list': return formatFsList(output);
    case 'fs_list_v2': return formatFsListEnvelope(value);
    case 'fs_find': return formatPathList(output);
    case 'fs_stat': return formatFsEntry(value);
    case 'fs_create_directory': return actionPath('Đã tạo thư mục', value);
    case 'fs_copy': return transferSummary('Đã sao chép', value);
    case 'fs_move': return transferSummary('Đã di chuyển', value);
    case 'fs_delete': return booleanAction('Đã xóa', value.deleted, pathFrom(value));
    case 'fs_write_raw': return actionPath('Đã ghi dữ liệu vào', value);
    case 'workspace_roots': return formatPathList(output, 'Workspace');
    case 'device_list': return formatDeviceList(output);
    case 'device_get': return formatDevice(value);
    case 'process_list': return formatProcessList(output);
    case 'process_inspect': return formatProcess(value);
    case 'process_kill': return booleanAction('Đã dừng process', value.killed ?? value.terminated, stringish(value.processId ?? value.pid));
    case 'command_run': return formatCommandExecution(value);
    case 'shell_create': return formatShell(value, 'Đã tạo terminal session');
    case 'shell_inspect': return formatShell(value, 'Terminal session');
    case 'shell_list': return formatShellList(output);
    case 'shell_resize': return simpleStatus(value, 'Đã thay đổi kích thước terminal');
    case 'shell_signal': return simpleStatus(value, 'Đã gửi signal tới terminal');
    case 'shell_close': return simpleStatus(value, 'Đã đóng terminal session');
    case 'shell_write': return value.writtenBytes !== undefined ? `Đã gửi ${value.writtenBytes} byte vào terminal.` : '';
    case 'git_branch': return formatGitBranches(output);
    case 'git_log': return formatGitLog(output);
    case 'git_commit': return formatGitCommit(value);
    case 'task_get': return formatTask(value);
    case 'task_artifact_read': return formatArtifact(value);
    case 'skills_list': return formatSkills(output);
    case 'skill_read': return formatSkill(value);
    case 'agent_subagent_start': return formatSubagent(value);
    case 'agent_subagent_wait': return formatSubagents(value);
    default: return '';
  }
}

function formatCommandExecution(value: Record<string, unknown>) {
  const command = asObject(value.command);
  return compact([
    label('Execution ID', value.executionId),
    label('Lệnh', command.executable),
    label('Số đối số', command.argumentCount),
    label('Thư mục', value.cwd),
    label('Trạng thái', value.terminalState),
    value.exitCode !== undefined ? `Exit code: ${value.exitCode ?? '—'}` : '',
    value.timedOut === true ? 'Đã hết thời gian.' : '',
    value.cancelled === true ? 'Đã hủy.' : '',
    label('Thời gian', value.elapsedMs !== undefined ? `${value.elapsedMs} ms` : undefined),
    label('Artifact', value.artifactRef),
    stringish(value.stdout) ? `stdout:\n${stringish(value.stdout)}` : '',
    stringish(value.stderr) ? `stderr:\n${stringish(value.stderr)}` : '',
  ]);
}

function formatFsList(output: unknown) {
  if (!Array.isArray(output)) return '';
  if (!output.length) return 'Thư mục trống.';
  return output.map((item) => {
    const value = asObject(item);
    const type = stringish(value.entryType ?? value.entry_type ?? value.type) || 'item';
    const path = pathFrom(value) || stringish(value.name);
    const size = value.size !== undefined ? ` · ${value.size} byte` : '';
    return `${type === 'directory' ? '📁' : '📄'} ${path}${size}`;
  }).join('\n');
}

function formatFsListEnvelope(value: Record<string, unknown>) {
  const data = asObject(value.data);
  const body = formatFsList(data.items ?? value.data);
  const page = asObject(value.page);
  const truncation = asObject(value.truncation);
  const contentRef = asObject(value.contentRef);
  return compact([
    body,
    stringish(data.sort) ? `Thứ tự: ${humanKey(stringish(data.sort))}` : '',
    stringish(data.directoryVersion) ? `Phiên bản thư mục: ${stringish(data.directoryVersion)}` : '',
    page.hasMore === true ? 'Còn dữ liệu ở trang tiếp theo.' : '',
    truncation.truncated === true ? `Kết quả bị cắt${stringish(truncation.reason) ? `: ${humanKey(stringish(truncation.reason))}` : '.'}` : '',
    stringish(contentRef.id) ? `Nội dung đầy đủ: ${stringish(contentRef.id)}` : '',
  ]);
}

function formatPathList(output: unknown, prefix = '') {
  if (!Array.isArray(output)) return '';
  if (!output.length) return 'Không có kết quả.';
  return output.map((item, index) => {
    const path = typeof item === 'string' ? item : pathFrom(asObject(item));
    return `${prefix ? `${prefix} ${index + 1}: ` : ''}${path || formatStructured(item)}`;
  }).join('\n');
}

function formatFsEntry(value: Record<string, unknown>) {
  const path = pathFrom(value);
  if (!path) return '';
  const lines = [`Đường dẫn: ${path}`];
  const type = stringish(value.entryType ?? value.entry_type ?? value.type);
  if (type) lines.push(`Loại: ${humanKey(type)}`);
  if (value.size !== undefined) lines.push(`Kích thước: ${value.size} byte`);
  if (value.readonly !== undefined) lines.push(`Chỉ đọc: ${yesNo(value.readonly)}`);
  return lines.join('\n');
}

function formatDeviceList(output: unknown) {
  if (!Array.isArray(output)) return '';
  return output.map((item, index) => `Thiết bị ${index + 1}\n${indent(formatDevice(asObject(item)))}`).join('\n\n');
}
function formatDevice(value: Record<string, unknown>) {
  return compact([
    label('ID', value.deviceId ?? value.device_id ?? value.id),
    label('Tên', value.name ?? value.label),
    label('Hệ điều hành', value.os ?? value.platform),
    label('Trạng thái', value.status ?? value.state),
  ]);
}
function formatProcessList(output: unknown) {
  if (!Array.isArray(output)) return '';
  if (!output.length) return 'Không có process.';
  return output.map((item) => formatProcess(asObject(item))).join('\n\n');
}
function formatProcess(value: Record<string, unknown>) {
  return compact([
    label('PID', value.pid ?? value.processId),
    label('Tên process', value.name ?? value.processName),
    label('Lệnh', value.command ?? value.executable),
    label('CPU', value.cpu),
    label('Bộ nhớ', value.memory ?? value.memoryBytes),
    label('Trạng thái', value.status ?? value.state),
  ]);
}
function formatShellList(output: unknown) {
  if (!Array.isArray(output)) return '';
  return output.map((item) => formatShell(asObject(item), 'Terminal session')).join('\n\n');
}
function formatShell(value: Record<string, unknown>, title: string) {
  const body = compact([
    label('Session', value.sessionId ?? value.session_id ?? value.id),
    label('Process ID', value.processId ?? value.pid),
    label('Trạng thái', value.status ?? value.state),
    label('Thư mục', value.initialWorkingDirectory ?? value.workingDirectory ?? value.cwd),
    label('Exit code', value.exitCode ?? value.exit_code),
  ]);
  return body ? `${title}\n${indent(body)}` : '';
}
function formatGitBranches(output: unknown) {
  if (Array.isArray(output)) return output.map((item) => typeof item === 'string' ? `• ${item}` : `• ${formatStructured(item)}`).join('\n');
  return '';
}
function formatGitLog(output: unknown) {
  if (!Array.isArray(output)) return '';
  return output.map((item) => {
    const value = asObject(item);
    const hash = stringish(value.hash ?? value.commit ?? value.id);
    const subject = stringish(value.subject ?? value.message ?? value.title);
    const author = stringish(value.author ?? value.authorName);
    return compact([`${hash}${hash && subject ? ' · ' : ''}${subject}`, author ? `Tác giả: ${author}` : '']);
  }).join('\n\n');
}
function formatGitCommit(value: Record<string, unknown>) {
  const stdout = stringish(value.stdout);
  if (stdout) return stdout;
  return compact([label('Commit', value.commit ?? value.hash), label('Message', value.message), label('Files', value.filesChanged)]);
}
function formatTask(value: Record<string, unknown>) {
  const task = asObject(value.task ?? value);
  return compact([
    label('Task', task.id ?? task.taskId),
    label('Tiêu đề', task.title),
    label('Trạng thái', task.status),
    label('Lượt', task.turnCount),
    label('Cập nhật', task.updatedAtUtc ?? task.updatedAt),
  ]);
}
function formatArtifact(value: Record<string, unknown>) {
  const artifact = asObject(value.artifact);
  const content = stringish(value.content);
  const meta = compact([label('Artifact', artifact.relativePath ?? artifact.id), label('Loại', artifact.mediaType), label('Kích thước', artifact.sizeBytes)]);
  return compact([meta, content ? `Nội dung:\n${content}` : '']);
}
function formatSkills(output: unknown) {
  if (!Array.isArray(output)) return '';
  return output.map((item) => {
    const value = asObject(item);
    const name = stringish(value.title ?? value.name ?? value.id);
    const description = stringish(value.description);
    return `• ${name}${description ? `\n  ${description}` : ''}`;
  }).join('\n');
}
function formatSkill(value: Record<string, unknown>) {
  return compact([label('Skill', value.title ?? value.name ?? value.id), stringish(value.instructions)]);
}
function formatSubagent(value: Record<string, unknown>) {
  return compact([label('Agent con', value.name ?? value.agentName), label('Task', value.taskId ?? value.childTaskId), label('Trạng thái', value.status)]);
}
function formatSubagents(value: Record<string, unknown>) {
  const finished = value.allFinished ?? value.completed;
  const children = value.children ?? value.subagents ?? value.result;
  return compact([finished !== undefined ? `Đã hoàn tất toàn bộ: ${yesNo(finished)}` : '', Array.isArray(children) ? formatStructured(children) : '']);
}

function formatStructured(value: unknown, depth = 0): string {
  if (value === null) return 'Không có';
  if (value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'bigint') return String(value);
  if (typeof value === 'boolean') return yesNo(value);
  if (Array.isArray(value)) {
    if (!value.length) return 'Không có mục nào.';
    return value.map((item, index) => {
      const rendered = formatStructured(item, depth + 1);
      return isPrimitive(item) ? `• ${rendered}` : `${index + 1}.\n${indent(rendered)}`;
    }).join('\n');
  }
  const entries = Object.entries(asObject(value)).filter(([key, item]) => key !== '__chatcmdDiff' && item !== undefined && item !== null);
  if (!entries.length) return 'Hoàn tất.';
  return entries.map(([key, item]) => {
    const rendered = formatStructured(item, depth + 1);
    if (isPrimitive(item)) return `${humanKey(key)}: ${rendered}`;
    return `${humanKey(key)}:\n${indent(rendered)}`;
  }).join('\n');
}

function actionPath(action: string, value: Record<string, unknown>) { const path = pathFrom(value); return path ? `${action}: ${path}` : ''; }
function transferSummary(action: string, value: Record<string, unknown>) {
  const source = stringish(value.source ?? value.from);
  const destination = stringish(value.destination ?? value.to ?? value.path);
  return source || destination ? `${action}${source ? `\nTừ: ${source}` : ''}${destination ? `\nĐến: ${destination}` : ''}` : '';
}
function booleanAction(action: string, result: unknown, detail = '') { return result === false ? `Không thể thực hiện thao tác.${detail ? `\n${detail}` : ''}` : `${action}${detail ? `: ${detail}` : '.'}`; }
function simpleStatus(value: Record<string, unknown>, action: string) { return compact([action, label('Session', value.sessionId ?? value.session_id), label('Trạng thái', value.status ?? value.state)]); }
function pathFrom(value: Record<string, unknown>) { return stringish(value.path ?? value.fullPath ?? value.relativePath ?? value.destination); }
function label(name: string, value: unknown) { const text = stringish(value); return text ? `${name}: ${text}` : ''; }
function stringish(value: unknown) { if (value === null || value === undefined) return ''; if (typeof value === 'string') return value; if (typeof value === 'number' || typeof value === 'bigint' || typeof value === 'boolean') return String(value); return ''; }
function yesNo(value: unknown) { return value === true ? 'Có' : value === false ? 'Không' : stringish(value); }
function compact(values: string[]) { return values.filter(Boolean).join('\n'); }
function indent(value: string) { return value.split('\n').map((line) => `  ${line}`).join('\n'); }
function isPrimitive(value: unknown) { return value === null || ['string', 'number', 'bigint', 'boolean', 'undefined'].includes(typeof value); }
function asObject(value: unknown): Record<string, unknown> { return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : {}; }
function humanKey(key: string) { return key.replace(/([a-z0-9])([A-Z])/g, '$1 $2').replaceAll('_', ' ').replace(/^./, (char) => char.toUpperCase()); }
