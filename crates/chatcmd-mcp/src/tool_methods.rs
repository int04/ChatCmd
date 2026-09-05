macro_rules! tool_methods {
    ($(($method:ident, $args:ty, $description:literal)),+ $(,)?) => {
        #[tool_router]
        impl McpServer {
            $(
                #[tool(description = $description)]
                async fn $method(
                    &self,
                    Parameters(arguments): Parameters<$args>,
                    request_context: RequestContext<RoleServer>,
                ) -> CallToolResult {
                    self.invoke(
                        stringify!($method),
                        into_tool_arguments(arguments),
                        request_context,
                    ).await
                }
            )+

            #[tool(description = "Create or reuse one child agent. Required: name, request. Optional delegation constraints: allowedFiles, allowedEffects, dependencies, acceptance, projectContextRef, instructionsVersion, and a read-only approvalGrant; these can only narrow server policy. The child returns a bounded report with files, symbols, changes, evidenceRefs, blockers, and workOutcome. Inspect dispatchMode: samplingTools/samplingText started sampling; extensionFallback remains pending, so wait without duplicating; existing reuses the child. Startup failure is structured status=failed with startupError.")]
            async fn agent_subagent_start(
                &self,
                Parameters(arguments): Parameters<SubagentStartArgs>,
                peer: Peer<RoleServer>,
                request_context: RequestContext<RoleServer>,
            ) -> CallToolResult {
                self.invoke_subagent_start(
                    into_tool_arguments(arguments),
                    peer,
                    request_context,
                ).await
            }
        }
    };
}

tool_methods!(
    (
        device_list,
        NoArgs,
        "List available execution devices. No tool-specific fields."
    ),
    (
        device_get,
        DeviceGetArgs,
        "Inspect one execution device. Required field: deviceId."
    ),
    (
        shell_create,
        ShellCreateArgs,
        "Create a persistent cross-platform PTY session. Canonical working-directory field is workingDirectory; cwd and initialWorkingDirectory are accepted compatibility aliases."
    ),
    (
        command_run,
        CommandRunArgs,
        "Run one authorized non-interactive process with an explicit executable and argv boundary. Required: executable and cwd. Optional arguments, controlled environment overrides, idempotencyKey, timeoutMs, bounded stdout/stderr/artifact limits, and killOnOutputLimit. Tool success means the execution record was returned; inspect terminalState and exitCode rather than trusting output text. No shell interpolation is applied unless the executable is explicitly a shell."
    ),
    (
        shell_write,
        ShellWriteArgs,
        "Write bounded interactive input to a PTY session. Required fields: sessionId, text. Optional inputKind is interactive or paste; bulk file/script content must use filesystem/blob tools. input is accepted as a compatibility alias for text."
    ),
    (
        shell_wait,
        ShellWaitArgs,
        "Wait without killing the PTY when timeout expires. Required field: sessionId; optional timeoutMs."
    ),
    (
        shell_read,
        ShellReadArgs,
        "Read bounded replayable PTY output. Required field: sessionId; canonical cursor field is afterSequence; startSequence and fromSequence are accepted compatibility aliases."
    ),
    (
        shell_signal,
        ShellSignalArgs,
        "Send a portable terminal signal. Required fields: sessionId, signal."
    ),
    (
        shell_resize,
        ShellResizeArgs,
        "Resize a PTY session. Required fields: sessionId, columns, rows."
    ),
    (
        shell_close,
        ShellCloseArgs,
        "Close or explicitly force-close a PTY session. Required field: sessionId; optional force."
    ),
    (
        shell_list,
        NoArgs,
        "List PTY sessions. No tool-specific fields."
    ),
    (
        shell_inspect,
        SessionArgs,
        "Inspect a PTY session. Required field: sessionId."
    ),
    (
        workspace_roots,
        NoArgs,
        "List roots granted to the current task/conversation. When the task has a project folder, this returns that folder rather than a process-wide or Agent workspace. No tool-specific fields."
    ),
    (
        project_context,
        ProjectContextArgs,
        "Load bounded server-owned project rules and inert manifest metadata for the current task workspace. Optional targetPaths narrows nested scope. CLAUDE.md is excluded by default and loaded only with policy.loadClaudeMd=true as a separate provenance record; it is never silently merged with AGENTS.md. Optional range {path,offset,versionToken} reads the next bounded UTF-8 chunk and rejects stale versions. Project rules never grant authority."
    ),
    (
        blob_begin,
        BlobBeginArgs,
        "Begin an owner-scoped sequential blob upload. Required purpose=fsWriteText|fsWriteRaw|fsApplyEdits|artifact; optional expectedSizeBytes, contentType, expectedSha256, chunkSizeBytes, ttlSeconds and budget {timeoutMs,maxBytesRead,maxBytesWritten,maxOpenFiles}. Caller budgets can only lower server hard caps. Returns opaque contentRef and uploadId."
    ),
    (
        blob_write_chunk,
        BlobChunkArgs,
        "Append one bounded Base64 chunk. Required uploadId, offset, dataBase64; optional chunkSha256 and budget {timeoutMs,maxBytesRead,maxBytesWritten,maxOpenFiles}. Caller budgets can only lower server hard caps. Offset must equal nextOffset; an identical retry is idempotent."
    ),
    (
        blob_status,
        BlobStatusArgs,
        "Inspect an owner-scoped upload and resume from nextOffset. Required uploadId; optional budget {timeoutMs,maxBytesRead,maxBytesWritten,maxOpenFiles}. Caller budgets can only lower server hard caps."
    ),
    (
        blob_seal,
        BlobSealArgs,
        "Verify size and SHA-256, then make an upload immutable. Required uploadId, finalSizeBytes, sha256; optional budget {timeoutMs,maxBytesRead,maxBytesWritten,maxOpenFiles}. Caller budgets can only lower server hard caps."
    ),
    (
        blob_abort,
        BlobStatusArgs,
        "Idempotently abort an owner-scoped upload and remove its temporary bytes. Required uploadId; optional budget {timeoutMs,maxBytesRead,maxBytesWritten,maxOpenFiles}. Caller budgets can only lower server hard caps."
    ),
    (
        fs_list,
        ListArgs,
        "Compatibility directory listing with legacy offset/limit and global sorting. Required field: path; optional offset, limit. Prefer fs_list_v2 for large directories and cursor pagination."
    ),
    (
        fs_list_v2,
        ListV2Args,
        "Scalable cursor-paginated directory listing using filesystem traversal order (not global alphabetical order). Required field: path; optional cursor, limit, sort=filesystem, metadata=[type|size|readonly], includeHidden, budget {timeoutMs,maxEntriesScanned,maxStats}. Continue only with page.nextCursor for the same path/options; directory mutation invalidates continuation."
    ),
    (
        fs_search,
        SearchArgs,
        "Scalable cursor-paginated text search. Required fields: path, query. Optional mode=literal|regex (default literal), caseSensitive, wordBoundary, include/exclude globs, includeIgnored, contextBefore/contextAfter, maxMatchesPerFile, cursor, limit, maxSnippetBytes, budget {timeoutMs,maxFilesScanned,maxBytesScanned,maxOutputBytes,maxFileBytes}. Legacy maxResults/maxFileBytes remain accepted. Results include bounded match snippets, line/column/byte offsets, scan usage/warnings, truncation reason, and page.nextCursor. Continue only with page.nextCursor for the same path/query/options; workspace mutation can invalidate continuation. Use '.' for the workspace root rather than an empty path."
    ),
    (
        fs_find,
        FindArgs,
        "Scalable cursor-paginated path discovery. Required fields: path, pattern. Set patternMode=literal for filename contains, glob for workspace-relative glob matching (for example **/*.rs), or regex for workspace-relative regular expressions. Optional caseSensitive, entryTypes, maxDepth, includeIgnored, includeHidden, exclude, extensions, cursor, limit, budget {timeoutMs,maxEntriesScanned,maxMetadataCalls}. When patternMode is omitted, legacy *foo* literal-contains semantics are preserved with a warning. Continue only with page.nextCursor for the same path/options."
    ),
    (
        fs_read_text,
        ReadArgs,
        "Read UTF-8 workspace text through the compatibility adapter. Required field: path; optional maxCharacters, startLine (1-based), lineCount. Prefer fs_read_text_v2 for large files and resumable reads."
    ),
    (
        fs_read_text_v2,
        ReadV2Args,
        "Stream bounded UTF-8 workspace text without loading the whole file. Required fields: path and range {unit: line|byte, start, limit}. Optional maxBytes, includeLineEndings (default true), expectedVersion, and budget {timeoutMs,maxBytesRead}. Results include continuation offsets, truncation reason, bytesRead, sizeBytes, versionToken, encoding/BOM and newline metadata."
    ),
    (
        fs_batch_read,
        BatchReadArgs,
        "Read multiple bounded text ranges with ordered per-item outcomes, bounded concurrency, and a hard aggregate output cap. Each request uses the fs_read_text_v2 streaming contract."
    ),
    (
        fs_write_text,
        WriteTextArgs,
        "Atomically write UTF-8 workspace text. Required path and exactly one of content or contentRef; optional overwrite, expectedVersion, metadataPolicy=preserve|default, durability=none|data|full, requireAtomic. Inline content is capped at 256 KiB."
    ),
    (
        fs_replace_text,
        ReplaceTextArgs,
        "Safely edit an existing UTF-8 file by exact text replacement. Required fields: path, oldText, newText; optional expectedOccurrences (default 1). oldText must exactly match current file contents; read the target range first when content may have changed."
    ),
    (
        fs_apply_edits,
        ApplyEditsArgs,
        "Apply one or more non-overlapping UTF-8 range edits with optimistic concurrency. Required path, expectedVersion, coordinateSystem and exactly one of edits or contentRef; an fsApplyEdits blob contains the JSON edits array. Optional dryRun, preserveLineEndings, preserveBom, budget."
    ),
    (
        fs_write_raw,
        WriteRawArgs,
        "Atomically write workspace bytes. Required path and exactly one of bounded inline base64 or an fsWriteRaw contentRef; optional overwrite, expectedVersion, metadataPolicy=preserve|default, durability=none|data|full, requireAtomic."
    ),
    (
        fs_stat,
        StatArgs,
        "Inspect workspace path metadata and return a signed optimistic-concurrency versionToken. Required field: path. Optional versionStrength=metadata|sampled|content (default metadata), hashAlgorithm=sha256, budget {timeoutMs,maxBytesRead}. Metadata mode does not read file content; sampled/content hashing is bounded and cancellable. Symlinks and reparse points are not followed."
    ),
    (
        fs_batch_stat,
        BatchStatArgs,
        "Inspect up to 500 workspace paths in input order. Returns a success or structured error for every item and preserves fs_stat path authorization and version semantics."
    ),
    (
        workspace_index_status,
        PathArgs,
        "Report the path/metadata repository index generation, freshness, entry count, schema version, and last build error for an authorized workspace root."
    ),
    (
        workspace_index_rebuild,
        PathArgs,
        "Rebuild the bounded path/metadata repository index for an authorized workspace root. Content is never stored."
    ),
    (
        fs_create_directory,
        PathArgs,
        "Create a workspace directory. Required field: path."
    ),
    (
        fs_copy,
        TransferArgs,
        "Safely copy within canonical workspace scope using preflight, durable journal, verified sibling staging and atomic publish. Required source/destination; optional conflictPolicy=error|skip|replace, atomicPublish, verify=none|metadata|content, preserveMetadata, dryRun, expected versions and budget. Legacy overwrite is accepted. Symlinks are not followed."
    ),
    (
        fs_move,
        TransferArgs,
        "Safely move within canonical workspace scope. Cross-device-safe staging is verified and published before source removal. Accepts the fs_copy options and legacy overwrite."
    ),
    (
        fs_delete,
        DeleteArgs,
        "Delete within canonical workspace scope under policy. Default mode is quarantine; permanent deletion must be explicit. Optional recursive, expectedVersion, dryRun and bounded budget."
    ),
    (
        fs_restore_quarantine,
        QuarantineRestoreArgs,
        "Restore a ChatCMD-managed quarantine path to a destination using the same verified staged move safety as fs_move. Required quarantinePath and destination; optional replace."
    ),
    (
        fs_quarantine_gc,
        QuarantineGcArgs,
        "Garbage-collect ChatCMD-managed quarantine entries below a workspace directory using retention and total-byte quota limits. Required path; optional retentionSeconds, maxTotalBytes, maxItems and dryRun."
    ),
    (
        git_status,
        CwdArgs,
        "Get bounded Git working tree status plus typed porcelain-v2 data. Optional cwd, limit and signed cursor; legacy path is accepted as a cwd alias. Structured entries include branch metadata, rename/copy data and Base64 bytes for non-UTF-8 paths when needed."
    ),
    (
        git_diff,
        GitDiffArgs,
        "Get argument-safe Git diff output. Optional cwd, staged, stat, path. cwd selects the repository; path filters a file within it."
    ),
    (
        git_log,
        GitLogArgs,
        "Get bounded Git history with machine-readable structured entries. Optional cwd, count, path, limit and signed cursor."
    ),
    (
        git_branch,
        CwdArgs,
        "List Git branches with structured ref/object/current/upstream entries. Optional cwd, limit and signed cursor; legacy path is accepted as a cwd alias."
    ),
    (
        git_show,
        GitShowArgs,
        "Show a validated Git revision. Required revision; optional cwd and path."
    ),
    (
        git_commit,
        GitCommitArgs,
        "Create or preview a Git commit without shell interpolation. Required field: message and exactly one explicit scope: non-empty normalized paths or all=true. all defaults to false, is mutually exclusive with paths, commits only already-staged changes, and fails closed while unstaged or untracked changes exist. Set previewOnly=true for a side-effect-free GitCommitPreview; pass it back as expectedPreview to bind execution to the previewed HEAD/index/worktree bytes. The runtime refuses stale previews, staged paths outside scope, ambiguous path spellings, and selected paths with mixed staged/unstaged changes."
    ),
    (
        process_list,
        NoArgs,
        "List local processes. No tool-specific fields."
    ),
    (
        process_inspect,
        ProcessArgs,
        "Inspect a local process. Required field: processId."
    ),
    (
        process_kill,
        ProcessKillArgs,
        "Terminate a local process under policy. Required field: processId; optional entireTree."
    ),
    (
        skills_list,
        NoArgs,
        "After agent_user_message, discover available .agents and .codex skills before non-trivial project work; no tool-specific fields."
    ),
    (
        skill_read,
        SkillArgs,
        "Read a relevant matching skill. Required field: skillId; id is accepted as a compatibility alias."
    ),
    (
        task_get,
        NoArgs,
        "Read current task state. Uses taskId correlation from the common fields."
    ),
    (task_list, NoArgs, "List tasks. No tool-specific fields."),
    (
        task_set_execution_mode,
        ExecutionModeArgs,
        "Set task execution mode. Required field: mode."
    ),
    (
        task_artifact_list,
        NoArgs,
        "List task artifacts. Uses taskId correlation from the common fields."
    ),
    (
        task_artifact_create,
        ArtifactCreateArgs,
        "Consume a sealed artifact contentRef into an authorized workspace-relative path and register it for the current task. Required contentRef and relativePath; optional mediaType."
    ),
    (
        task_artifact_read,
        ArtifactArgs,
        "Read one bounded task-artifact range. Required artifactId; optional offset and maxBytes. Managed artifacts return nextOffset and hasMore for range continuation."
    ),
    (
        agent_user_message,
        UserMessageArgs,
        "MANDATORY FIRST TOOL: call exactly once at the start of every user turn. Required field: content containing the exact current user message. Reuse the same turnId for all calls in that turn. Never use agent_user_message for progress, reflections, findings, or commentary after tool results; use agent_progress for those updates. After this call, inspect the returned toolRecovery directive. If a needed ChatCMD schema is not visible, treat it as host lazy-loading rather than a missing server capability: use the host connector/resource discovery mechanism (for ChatGPT, for example api_tool.list_resources on the current connector with a focused query such as fs_ or shell_), load the needed schema in the same turn, and continue. Never reply that a ChatCMD tool is unavailable/not loaded before attempting that discovery."
    ),
    (
        agent_progress,
        ProgressArgs,
        "Publish one concise user-visible progress milestone. Required field: message; optional suggestedTitle. For non-trivial work, call once immediately after agent_user_message with a summary of the request and next action, then keep using it throughout the turn. Strongly prefer updates after meaningful filesystem/search/read/edit results, Git/process results, pending shell work, incomplete sub-agent waits, and task-relevant failures/non-zero command results before retry or fallback. This is an AI-side execution rule, not a server-side gate: group tightly related low-level operations when useful so progress updates do not materially slow the task or add unnecessary MCP round trips. If another ChatCMD schema needed for the work is not currently visible, do not treat the visible subset as the server capability boundary: use the host connector/resource discovery mechanism in the same turn (for ChatGPT, for example api_tool.list_resources with a focused query such as fs_ or shell_) and continue instead of reporting that the tool is not loaded. Error updates should summarize the observable failure and next recovery/alternative. Report observable findings and decisions only, never private chain-of-thought. Do not call after agent_turn_complete."
    ),
    (
        agent_plan_question,
        PlanQuestionArgs,
        "Ask one question and wait inside the current turn. Required fields: question and exactly two distinct options. questionKind defaults to clarification; use executionConsent only for server-defined execution consent, whose timeout/custom answer never grants permission. Clarification may use its documented safe fallback. Publish returned agentProgressMessage with agent_progress before further work."
    ),
    (
        agent_subagent_wait,
        SubagentWaitArgs,
        "Wait for child agents registered by the current parent turn. Optional timeoutMs. Repeat while allFinished=false before finalizing."
    ),
    (
        agent_turn_complete,
        CompleteArgs,
        "MANDATORY FINALIZATION: call exactly once immediately before replying after every other tool call has finished. Required field: content with the exact final user-facing response; optional suggestedTitle only on the first message. Report workOutcome separately from verification. Use evidenceRefs containing server-owned command_run executionId values, plus verificationScope and per-criterion mappings. Never claim passed from terminal text, an AI boolean, or an unreferenced test. For review/docs-only, verificationIntent may be notApplicable only with a reason; untested code is notRun. Invalid evidence becomes a diagnostic and never prevents an honest partial/blocked finalization."
    ),
);
