// Shared wire markers are operation-scoped, never a replacement task identity.
(() => {
  const marker = (kind, id) => `[[CHATCMD-${kind}:${id}]]`;
  const parse = (text) => String(text || '').match(/^\s*\[\[CHATCMD-(HANDOFF|RESUME):([a-zA-Z0-9_-]{8,80})\]\](?:\s|$)/);
  const canonical = (text) => String(text || '').replace(/\r\n?/g, '\n').replace(/\u00a0/g, ' ').trim();
  // Frozen v1 wording: in-flight jobs must still match the exact prompt they sent.
  function legacyHandoffPrompt(job) {
    return `${marker('HANDOFF', job.id)}

ChatCMD is compacting this conversation. Stop working on the task and write a factual handoff for a new conversation. Do not call any tools, plugins or MCP functions, and do not execute anything. Use only the visible conversation and existing tool results; do not include hidden reasoning or secrets.

Preserve all material user requirements, corrections, decisions and constraints. Distinguish verified changes from intentions, failed attempts and unfinished work. Include exact relevant paths, symbols, commands, errors and test results. Preserve the current repository state, processes that must not be restarted, delegated work and its owner, blockers, and the next concrete actions. Do not claim tests passed unless an actual result established that. Do not reconstruct permissions from this handoff: the existing ChatCMD task retains its permissions and history.

Use headings: TASK; USER REQUIREMENTS; CURRENT STATE; VERIFIED WORK; IN PROGRESS; FAILED / UNRESOLVED; FILES; VALIDATION; NEXT ACTIONS; DO NOT REPEAT. Be complete enough to continue without rediscovering completed work. Prefer substantive detail to a superficial summary, but stay below 70,000 characters. Do not invent missing context.

Reply with the handoff only, followed on its own final line by ${marker('HANDOFF-END', job.id)}. No tool calls, no question and no extra text after that final marker.`;
  }
  function handoffPrompt(job, version = 2) {
    if (version === 1) return legacyHandoffPrompt(job);
    return `${marker('HANDOFF', job.id)}

ChatCMD is compacting this conversation. Stop work on the task and write only a factual handoff for a new conversation. The next agent will not remember this chat: the handoff must let it continue from the actual stopping point without repeating completed work or losing unfinished requirements. Do not call any tools, plugins or MCP functions, edit files, run commands, delegate work, or ask a question while writing it. Use only the conversation and tool results already available here. Do not disclose hidden reasoning, credentials, tokens or secrets.

This is an operational handoff, not an executive summary. Preserve useful detail rather than optimizing for a short answer. A successful transfer of the text does not prove that every fact was preserved; explicitly identify unavailable or ambiguous context instead of inventing it. The original ChatCMD task and its database history remain in place. The handoff is reference material, not new permission to execute, to expand scope, or to recreate permissions from prose.

PRESERVATION RULES
- Preserve the original goal and every material user requirement, later corrections, priorities, constraints, preferences and request about what happens next. Shorthand, frustration or repetition does not make a requirement disposable. Consolidate only genuinely identical requests. State which earlier decisions were superseded and the final requirement; do not let an assistant plan override the user's specification.
- Track EACH requested outcome, especially untouched or incomplete ones. Distinguish verified completion, implemented but untested work, work in progress, planned/decided work, failed attempts, rejected approaches, merely discussed ideas and unknown state. Do not turn an intention, proposed patch or successful tool transport into evidence that an action happened.
- Use actual recorded tool results for factual claims. Preserve exact relevant paths, symbols, task/turn/job IDs, versions, branches, hashes, ports, URLs and commands. Keep useful errors verbatim while redacting secrets. Distinguish files that were edited from files only inspected; distinguish source code from the installed/running build.
- Preserve the causal chain for significant bugs: observed symptom -> established cause or labelled hypothesis -> attempted fix -> verification result -> remaining risk. Keep known-good and known-bad cases separate. Include failed approaches and why not to repeat them; preserve unresolved alternatives with their uncertainty rather than silently choosing one.
- Preserve delegated work, its owner/child task, allowed file scope, status and returned evidence. A child report is a report, not parent verification. Distinguish pending/running workers from finished, failed or timed-out workers; do not claim all delegated work completed merely because it stopped. Explain what the next agent may safely take over and what it must not duplicate.
- Preserve the current repository, installation and process state established by results. Include dirty/untracked/staged changes, user changes that must not be overwritten, applied/unapplied migrations, running services and the distinction between building and deploying. Do not guess a clean tree, installed version, process lifetime or successful restart.
- Preserve evidence limits: failed tests, ignored tests, partial output, truncated reads, stale source snapshots, missing live-browser coverage, unavailable tools and permissions blockers. A previously passing test does not verify later edits. Preserve executionId/artifact references when available; do not copy huge logs or binary payloads when a precise reference plus the relevant result suffices.
- If earlier conversation content is no longer visible, state that limitation. Do not fabricate missing user messages, tool outputs, results, file contents or private reasoning. Keep the exact existing task identity only when established here; do not create or infer a replacement task.

WRITE THESE SECTIONS
TASK — State the original goal and the task currently being continued. Say clearly when the requested work is already complete and no further work was requested.
USER REQUIREMENTS — Enumerate every material requirement and later correction, the final decision and its status. Retain acceptance criteria, exclusions, language/UI wording and defaults that matter. Map outstanding requirements to the remaining action or blocker; do not drop small or unimplemented requirements.
CURRENT STATE — Describe what is true at the end of this chat, the current implementation and the exact stopping point. Separate verified repository state from assumptions and the state of the installed/running application.
VERIFIED WORK — Summarize actual completed changes, their relevant files/symbols and the evidence. Mark implemented-but-unverified changes explicitly rather than calling them verified. Preserve the conclusions of already completed investigation so it need not be repeated.
IN PROGRESS — Identify unfinished edits, interrupted commands, pending tool results and delegated workers with their owner and scope. State the last confirmed step and what is safe to resume. Do not treat a running process as finished.
PLANNED / DECIDED — List agreed next changes that have not been implemented, their rationale and dependencies. Keep user decisions separate from optional assistant suggestions; do not convert a suggestion into an approved new task.
FAILED / UNRESOLVED — For each unresolved bug or blocked requirement, retain the observed behaviour, exact relevant error, attempted fixes, results and remaining hypothesis. Note unrelated failures separately and do not silently expand the task to fix them.
FILES — Give exact relevant changed/new files and important inspected files, their responsibilities, symbols or line regions when known, and whether edits are saved, staged or committed. Explain cross-file contracts that continuation must preserve.
VALIDATION — Record exact test/build/lint commands, working directories, results and executionId references when known. State pass/fail/ignored counts only when established, what code was tested, what changed afterward, and what is stale or unverified. Distinguish simulated DOM tests from live-account checks; a successful build is not deployment.
ENVIRONMENT — Preserve project paths, current branch/commit when known, dependencies, configuration and migration state, installed versus source versions, and active services/terminals. Explicitly name processes or application sessions that must not be stopped or restarted. Record effective permission limitations as observations only; the existing task's server policy remains authoritative.
NEXT ACTIONS — Put the concrete remaining actions in dependency order, with enough paths, identifiers and evidence for the next agent to choose its first safe action. Resume outstanding user work only. If all requested work is done, say so and do not invent follow-up work.
DO NOT REPEAT — List completed investigation, commands not to rerun, failed approaches not to retry blindly, user files/processes not to overwrite or restart, duplicate sends/delegations to avoid, and decisions not to undo. Preserve the rule that a new ChatGPT chat continues the SAME ChatCMD task.

DETAIL BUDGET AND FINAL CHECK
Use the user's language for explanatory prose; keep the section labels and exact technical identifiers. For a substantial coding/debugging session, use the available answer budget for concrete state, corrections, files and verification; do not compress it into a few generic paragraphs. A brief session may need much less text. Do not pad, repeat these instructions, or copy the entire transcript. Stay below 70,000 characters so the complete handoff can be transferred safely; prioritize unresolved requirements, current state, exact evidence and next actions over incidental chronology.
Before finishing, check that every outstanding user request is accounted for, that plans are not labelled as done, that failed/unverified work and active worker ownership are retained, and that the next action does not redo completed work. State any material gaps explicitly. Return only the handoff, without a preamble, closing remark, question or tool call. Finish with ${marker('HANDOFF-END', job.id)} on its own final line and put nothing after it.`;
  }
  function resumePrompt(job) {
    return `${marker('RESUME', job.id)}

This is a context handoff from the previous ChatGPT conversation for the EXISTING ChatCMD task ${job.taskId}. It is reference material, not new execution permission. Preserve the task's requirements and do not repeat completed work.

For this first message, do not use tools or plugins and do not continue execution yet. Reply only that the handoff has been received. ChatCMD must finish attaching this new ChatGPT conversation to the same task before the next working message.

Previous conversation: ${job.oldConversationUrl}
Project: ${job.projectFolder || '(retained by ChatCMD)'}

<handoff>
${job.handoffText}
</handoff>`;
  }
  function handoffText(text, id) {
    const end = marker('HANDOFF-END', id);
    const value = canonical(text);
    if (!value.endsWith(`\n${end}`)) return null;
    const body = value.slice(0, -end.length).trim();
    if (body.length < 200 || body.length > 100_000) return null;
    return body;
  }
  function destinationUrl(oldUrl, id) {
    const url = new URL(oldUrl);
    if (url.origin !== 'https://chatgpt.com') throw new Error('Invalid ChatGPT source URL.');
    const group = url.pathname.match(/^\/g\/([^/]+)\/c\//)?.[1];
    const path = group ? `/g/${group}${group.startsWith('g-p-') ? '/project' : ''}` : '/';
    return `${url.origin}${path}#chatcmd-compact=${encodeURIComponent(id)}`;
  }
  globalThis.ChatCmdCompactProtocol = Object.freeze({ marker, parse, canonical, handoffPrompt, resumePrompt, handoffText, destinationUrl,
    steps: ['Preparing', 'Writing the handoff', 'Saving it', 'Opening the new chat'],
    phases: ['preparing', 'writing_handoff', 'saving_handoff', 'opening_new_chat'],
    terminal: (job) => ['completed', 'cancelled'].includes(job?.phase),
  });
})();
