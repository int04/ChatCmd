# ChatCMD MCP Methods

Tài liệu này liệt kê các MCP tool/method mà `chatcmd-mcp` hiện expose để agent gọi về ChatCMD server/runtime.

Nguồn đối chiếu chính:

- `crates/chatcmd-mcp/src/lib.rs` — đăng ký rmcp router + schema/description của từng method; đây là source of truth runtime.
- `crates/chatcmd-mcp/src/tool_catalog.rs` — sinh canonical manifest, capability flags, metadata và `catalogHash` trực tiếp từ rmcp router; không duy trì danh sách tool thủ công thứ hai.

> Tổng số method phải lấy từ generated catalog/runtime thay vì hard-code trong tài liệu hoặc connector.

## Quy ước chung

Phần lớn method đều có các trường correlation chung do ChatCMD bổ sung:

- `taskId`: ID của ChatCMD task/conversation hiện tại.
- `turnId`: ID của user turn hiện tại. Mọi tool trong cùng một turn phải dùng cùng `turnId`.
- `agentId`: định danh agent gọi tool; server có thể ghi đè bằng authenticated identity.
- `requestId`: khóa idempotency do caller sinh; thường có thể bỏ qua để ChatCMD tự sinh.

Luồng agent bắt buộc:

1. `agent_user_message` phải là tool đầu tiên và chỉ gọi đúng một lần cho user turn thật.
2. Với mọi yêu cầu không-trivial, gọi `agent_progress` ngay sau đó để tóm tắt user yêu cầu gì và agent sẽ làm gì tiếp theo, trước `skills_list` hoặc tool substantive khác.
3. Với công việc project không tầm thường, gọi `skills_list`; nếu có skill phù hợp thì đọc bằng `skill_read` trước khi thao tác liên quan.
4. Trong lúc thực hiện, duy trì `agent_progress` theo checkpoint có ý nghĩa: thường sau khoảng 2–4 substantive operation hoặc sau một batch thao tác low-level liên quan chặt. Không cần callback theo từng tool; shell polling nhanh có thể gom cho đến khi trạng thái/output thay đổi đáng kể, còn lỗi/retry nên báo hướng xử lý trước khi đổi cách làm.
5. Nếu có sub-agent thì phải chờ chúng hoàn tất bằng `agent_subagent_wait`.
6. `agent_turn_complete` phải là tool cuối cùng, gọi đúng một lần ngay trước khi agent trả lời user.

---

## 1. Device methods

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `device_list` | Không có tham số riêng | Liệt kê các execution device/máy hiện đang có thể dùng để thực thi. |
| `device_get` | `deviceId` | Lấy thông tin chi tiết của một execution device cụ thể. |

---

## 2. Shell / PTY methods

Các method này quản lý terminal session dạng PTY chạy lâu dài, dùng được đa nền tảng.

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `shell_create` | `workingDirectory?`, `executable?`, `arguments?`, `environment?`, `columns?`, `rows?` | Tạo một persistent PTY/terminal session. `workingDirectory` là field chuẩn; `cwd` và `initialWorkingDirectory` chỉ là alias tương thích. |
| `shell_write` | `sessionId`, `text`, `appendNewLine?` | Gửi input literal vào terminal session. `input` là alias tương thích của `text`. |
| `shell_wait` | `sessionId`, `timeoutMs?` | Chờ terminal/process trong PTY thay đổi hoặc kết thúc. Hết timeout không tự kill session. |
| `shell_read` | `sessionId`, `afterSequence?`, `maxEvents?` | Đọc output replayable của PTY theo sequence. `afterSequence` là cursor chuẩn; `startSequence`/`fromSequence` là alias tương thích. |
| `shell_signal` | `sessionId`, `signal` | Gửi signal portable tới terminal, ví dụ interrupt/terminate tùy signal runtime hỗ trợ. |
| `shell_resize` | `sessionId`, `columns`, `rows` | Resize kích thước terminal PTY. |
| `shell_close` | `sessionId`, `force?` | Đóng PTY session; có thể force-close khi cần. |
| `shell_list` | Không có tham số riêng | Liệt kê các PTY session hiện có. |
| `shell_inspect` | `sessionId` | Xem trạng thái/thông tin của một PTY session. |

---

## 3. Workspace và filesystem methods

Các method `fs_*` thao tác trực tiếp trong canonical workspace scope và tuân theo policy/path grant của ChatCMD.

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `workspace_roots` | Không có tham số riêng | Liệt kê các canonical workspace root mà agent được phép thao tác. |
| `fs_list` | `path`, `offset?`, `limit?` | Legacy compatibility: trả trực tiếp mảng `FsEntry`, global sort theo tên rồi mới offset/limit; runtime cap `limit` ở 2.000. Với thư mục lớn nên dùng `fs_list_v2`. |
| `fs_list_v2` | `path`, `cursor?`, `limit?`, `sort?`, `metadata?`, `includeHidden?`, `budget?` | Cursor pagination bounded-work theo `sort=filesystem` (không hứa global alphabetical). `metadata` hỗ trợ `type`, `size`, `readonly`; mặc định `[]` để tránh stat. Result envelope v1 có `data.items`, `data.directoryVersion`, `data.sort`, `page.nextCursor/hasMore`, usage `entriesScanned/metadataCalls`, truncation/warnings khi cần. Cursor chỉ dùng lại cho cùng path/options; directory đổi thì continuation fail và phải restart. |
| `fs_search` | `path`, `query`, `caseSensitive?`, `maxResults?`, `maxFileBytes?`, `includeIgnored?`, `exclude?` | Tìm kiếm **nội dung text** trong workspace. Khi tìm từ root nên dùng `path: "."`. |
| `fs_find` | `path`, `pattern`, `patternMode?`, `caseSensitive?`, `entryTypes?`, `maxDepth?`, `includeIgnored?`, `includeHidden?`, `exclude?`, `extensions?`, `cursor?`, `limit?`, `budget?` (`maxResults?` legacy) | Tìm **đường dẫn/tên file hoặc thư mục** bằng traversal có early-stop và cursor. `patternMode=literal` tìm chuỗi trong filename; `glob` match path tương đối như `**/*.rs`; `regex` match regex trên path tương đối. Kết quả dùng `ToolResultEnvelope`, tiếp tục bằng `page.nextCursor` với cùng path/options. Bỏ `patternMode` giữ tương thích legacy `*foo*` literal-contains và trả warning. |
| `fs_read_text` | `path`, `startLine?`, `lineCount?`, `maxCharacters?` | Adapter tương thích cho contract cũ; nội bộ dùng reader streaming/range, không còn tải toàn file vào RAM. Với file lớn/resumable nên dùng `fs_read_text_v2`. |
| `fs_read_text_v2` | `path`, `range { unit: line\|byte, start, limit }`, `maxBytes?`, `includeLineEndings?`, `expectedVersion?`, `budget { timeoutMs?, maxBytesRead? }?` | Reader streaming/range bounded-memory. Trả `range`, `nextStartLine`/`nextByteOffset`, `truncated` + `truncationReason`, `bytesRead`, `sizeBytes`, `versionToken`, UTF-8/BOM và newline metadata; `expectedVersion` chặn continuation stale khi file đã đổi. |
| `fs_write_text` | `path`, `content`, `overwrite?` | Ghi nguyên tử nội dung UTF-8 vào file; dùng cho tạo mới hoặc thay toàn bộ file. |
| `fs_replace_text` | `path`, `oldText`, `newText`, `expectedOccurrences?` | Chỉnh sửa an toàn bằng exact text replacement. `oldText` phải khớp nội dung hiện tại. |
| `fs_apply_edits` | `path`, `expectedVersion`, `coordinateSystem`, `edits`, `columnEncoding?`, `dryRun?`, `preserveLineEndings?`, `preserveBom?`, `budget?` | Sửa nhiều range UTF-8 không chồng lấn bằng streaming temp-file transaction; kiểm tra version trước xử lý và ngay trước atomic commit. |
| `fs_write_raw` | `path`, `base64`, `overwrite?` | Decode Base64 và ghi atomically dữ liệu binary/raw vào workspace. |
| `fs_stat` | `path` | Xem metadata của một file/thư mục: loại entry, size, readonly, v.v. |
| `fs_create_directory` | `path` | Tạo thư mục trong workspace. |
| `fs_copy` | `source`, `destination`, `conflictPolicy?`, `atomicPublish?`, `verify?`, `preserveMetadata?`, `followSymlinks?`, `dryRun?`, `expectedSourceVersion?`, `expectedDestinationVersion?`, `budget?`, `overwrite?` | Preflight bounded, copy vào sibling staging, verify rồi atomic publish; `overwrite` là adapter cũ cho `replace`. Symlink/reparse không được follow. |
| `fs_move` | giống `fs_copy` | Stage-copy → verify → publish trước khi xóa source; báo `completedWithSourceRemaining` nếu cleanup source lỗi. |
| `fs_delete` | `path`, `recursive?`, `mode?`, `expectedVersion?`, `dryRun?`, `budget?` | `mode=quarantine` mặc định; permanent phải explicit. Root/grant root bị từ chối và traversal dùng no-follow. |

### Phân biệt nhanh `fs_search` và `fs_find`

- `fs_search`: tìm **chuỗi trong nội dung file**.
- `fs_find`: tìm **file/folder/path** theo tên/pattern.

---

## 4. Git methods

Các Git method được thiết kế để tránh shell interpolation và truyền argument có kiểm soát.

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `git_status` | `cwd?`, Git limits | Xem porcelain v2 và branch metadata của working tree. `path` cũ được chấp nhận như alias của `cwd`. |
| `git_diff` | `cwd?`, `staged?`, `stat?`, `path?`, Git limits | Lấy Git diff; nên gọi `stat=true` trước khi yêu cầu patch lớn. Ext-diff và màu luôn bị tắt. |
| `git_log` | `cwd?`, `count?`, `path?` | Xem lịch sử commit có giới hạn số lượng; có thể lọc theo path. |
| `git_branch` | `cwd?`, Git limits | Liệt kê branch bằng format machine-readable. |
| `git_show` | `revision`, `cwd?`, `path?` | Xem nội dung của một revision/commit đã được validate; có thể lọc theo path. |
| `git_commit` | `message`, `cwd?`, `all?`, `paths?` | Tạo Git commit mà không dùng shell interpolation. `all` mặc định là `true`; không được gọi với object rỗng. |

Mọi Git method nhận thêm `outputMode` (`inline` hoặc `inlineOrArtifact`),
`maxOutputBytes`, `maxStderrBytes`, `timeoutMs`, `maxRuntimeMs`,
`artifactMaxBytes` và `killOnLimit`. Mặc định timeout là 30 giây, stdout preview
512 KiB, stderr preview 128 KiB và artifact tối đa 256 MiB. Result giữ các field
cũ (`exitCode`, `stdout`, `stderr`, `truncated`) và bổ sung byte counters,
`truncationReason`, `artifactRef`, SHA-256, elapsed time, timeout/cancellation.
Git chạy với stdin/pager/credential prompt bị vô hiệu hóa; path luôn được truyền sau
`--`, không qua shell interpolation.

---

## 5. Process methods

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `process_list` | Không có tham số riêng | Liệt kê process cục bộ đang chạy trên execution host. |
| `process_inspect` | `processId` | Xem chi tiết một local process. |
| `process_kill` | `processId`, `entireTree?` | Kết thúc process theo policy; có thể kill toàn bộ process tree. |

---

## 6. Skill methods

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `skills_list` | Không có tham số riêng | Khám phá các skill trong `.agents` và `.codex`. Với project work không tầm thường, method này phải được gọi sau `agent_user_message` và trước khi inspect/code nếu chưa biết skill phù hợp. |
| `skill_read` | `skillId` | Đọc đầy đủ instruction của một skill phù hợp. `id` là compatibility alias của `skillId`. |

---

## 7. Task methods

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `task_get` | Dùng `taskId` chung | Đọc state hiện tại của ChatCMD task. |
| `task_list` | Không có tham số riêng | Liệt kê các task. |
| `task_set_execution_mode` | `mode` | Đổi execution mode của task hiện tại. |
| `task_artifact_list` | Dùng `taskId` chung | Liệt kê các artifact được gắn với task. |
| `task_artifact_read` | `artifactId` | Đọc một task artifact cụ thể. |

---

## 8. Agent lifecycle / orchestration methods

Đây là nhóm method điều phối vòng đời một turn giữa ChatGPT/agent và ChatCMD server.

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `agent_user_message` | `content` | **Bắt buộc là MCP call đầu tiên và chỉ gọi đúng một lần trong mỗi user turn.** Đồng bộ nguyên văn user message lên ChatCMD và thiết lập/correlate `taskId` + `turnId`. `content` phải đúng nguyên văn message hiện tại. Không dùng method này cho progress/reflection/finding sau tool result; các cập nhật đó phải dùng `agent_progress`. |
| `agent_progress` | `message`, `suggestedTitle?` | **Rule phía AI cho mọi turn project không-trivial.** Ngay sau `agent_user_message` nên gửi progress tóm tắt yêu cầu + hành động kế tiếp. Sau các kết quả `fs_*` có ý nghĩa (đặc biệt `fs_find`, `fs_search`, `fs_read_text`, edit/write/delete), Git/process, `shell_read`/`shell_wait` còn pending, sub-agent wait chưa xong, hoặc failure/non-zero, AI nên gửi progress mô tả kết quả quan sát được và bước tiếp theo trước khi tiếp tục. Đây không phải runtime gate: server không reject tool chỉ vì thiếu progress; các thao tác low-level liên quan chặt có thể gom thành một checkpoint để tránh làm chậm tiến độ và tránh callback MCP không cần thiết. Không gửi private chain-of-thought. |
| `agent_plan_question` | `question`, `options` | Tạm dừng plan để hỏi người dùng một câu hỏi có hai lựa chọn rõ ràng; câu trả lời được tiếp tục qua hàng đợi phê duyệt của ChatCMD. |
| `agent_subagent_start` | `name`, `request` | Tạo và dispatch một child agent khi ChatGPT chủ động chia việc hoặc người dùng yêu cầu chia agent. Chỉ sử dụng model sampling do ChatGPT/MCP host cung cấp; nếu host không hỗ trợ sampling thì trả `samplingUnavailable`/`failed` và tuyệt đối không khởi chạy Codex hay executor local. |
| `agent_subagent_wait` | `timeoutMs?` | Chờ các child agent của parent turn. Nếu `allFinished=false` thì tiếp tục gọi lại trước khi finalize. |
| `agent_turn_complete` | `content`, `suggestedTitle?` | **Bắt buộc là MCP call cuối cùng.** Xác nhận turn đã hoàn tất và gửi đúng nội dung cuối cùng agent sẽ trả cho user. Chỉ được gọi đúng một lần sau khi mọi tool/sub-agent đã xong. |

Lưu ý: `fs_find`, `fs_search`, `fs_read_text`, các tool sửa file, shell, Git... **không tạo thêm `agent_user_message`**. `agent_user_message` chỉ đại diện cho message thật của user ở đầu turn. Sau kết quả của các tool này, message cập nhật gửi cho user phải đi qua `agent_progress`.

---

## 9. Generated catalog, version và cache invalidation

`TOOL_NAMES` được sinh từ chính `McpServer::tool_router().list_all()` và sort deterministic. Không copy danh sách tool sang connector, UI, release script hoặc tài liệu.

Canonical manifest chứa `protocolVersion`, `catalogVersion` và với mỗi tool có `name`, normalized input schema, `resultSchema` cùng capability flags. Tool chưa migrate result contract có `resultSchema: null` và `resultSchemaVersion: null`; `fs_list_v2` và `fs_find` quảng bá `resultSchemaVersion: 1` cùng generated JSON schema của `ToolResultEnvelope<...>` tương ứng. Trước khi hash SHA-256, object keys được sort và metadata chỉ để mô tả như `description`/`title` được bỏ khỏi contract; vì vậy đổi wording không làm invalid cache, còn đổi input/result schema hoặc capability sẽ làm đổi `catalogHash`.

Chi tiết semantics, cursor/error code, migration inventory và các ví dụ complete/paged/truncated/content-backed nằm tại `docs/tool_result_envelope.md`.

Metadata runtime gồm `appVersion`, `protocolVersion`, `catalogVersion`, `catalogHash`, `buildId`. MCP initialize trả metadata dưới prefix `CHATCMD_CATALOG_METADATA=...` trong server instructions. HTTP host cũng expose endpoint authenticated `GET /mcp/{token}/catalog` để diagnostics lấy metadata + canonical manifest; token vẫn chỉ ở auth boundary và không được ghi vào structured catalog log.

Caller có thể gửi `clientCatalogHash` trong common tool arguments. Nếu hash khác server, request fail-fast với `error.code = "catalog_mismatch"`, kèm cả `clientCatalogHash`, `serverCatalogHash` và recovery instruction. Connector phải bỏ schema cache cũ, reconnect/initialize/list_tools lại và chỉ retry operation tối đa một lần sau refresh để tránh retry loop.

Release gate cho catalog là `cargo test -p chatcmd-mcp --test release_catalog_smoke`: test spawn binary `catalog_smoke_server` qua stdio transport thật hai lần, gọi MCP initialize/list_tools, bắt buộc có `fs_replace_text`, và so names + normalized schema với canonical manifest. Gate này cần chạy trên cả Windows và macOS ở CI/release pipeline; repository hiện không có `.github/workflows`, nên command này được giữ trong Cargo test suite để pipeline hiện có/tương lai gọi trực tiếp.

---

## 10. Luồng gọi mẫu

Một turn sửa code thông thường có thể có flow:

```text
agent_user_message
  -> agent_progress              (ngay lập tức: tóm tắt user yêu cầu gì + bước tiếp theo)
  -> skills_list
  -> skill_read                  (nếu có skill phù hợp)
  -> workspace_roots / fs_find
  -> fs_read_text / fs_search    (có thể gom các read/search liên quan thành một batch)
  -> agent_progress              (báo finding chính sau batch inspect/search)
  -> fs_replace_text / fs_write_text
  -> agent_progress              (báo file vừa đổi gì và tác động chính nếu đáng báo)
  -> git_diff / git_status       (nếu cần kiểm tra thay đổi)
  -> agent_progress              (báo kết quả verify/Git đáng chú ý)
  -> agent_turn_complete
```

Cadence mặc định là khoảng **2–4 substantive operation hoặc hết một coherent batch**, không phải một progress cho mỗi tool call. Nếu có finding quan trọng, lỗi, hoặc chuyển phase thì có thể báo sớm hơn.

Một turn có sub-agent:

```text
agent_user_message
  -> agent_progress              (tóm tắt yêu cầu + kế hoạch chia việc)
  -> skills_list
  -> agent_subagent_start
  -> agent_progress              (báo child đã được dispatch hoặc lỗi dispatch + fallback)
  -> ... parent tiếp tục công việc, vẫn giữ cadence progress ...
  -> agent_subagent_wait
  -> agent_progress              (nếu child vẫn chưa xong, báo đang chờ gì)
  -> agent_subagent_wait         (lặp nếu allFinished=false)
  -> agent_turn_complete
```

Một turn chạy terminal dài:

```text
agent_user_message
  -> agent_progress              (tóm tắt command/workflow sắp chạy)
  -> shell_create
  -> shell_write
  -> shell_wait / shell_read     (có thể poll ngắn liên tiếp)
  -> agent_progress              (nếu vẫn chạy lâu: báo stage/output hiện tại + đang chờ gì)
  -> shell_wait / shell_read     (tiếp tục poll; không lặp message nếu trạng thái chưa đổi)
  -> agent_progress              (khi có thay đổi đáng kể, kết quả cuối, hoặc lỗi + hướng recovery)
  -> shell_close                 (nếu cần đóng session)
  -> agent_turn_complete
```

Nếu command/tool trả lỗi hoặc exit code khác 0, `agent_progress` phải xuất hiện **trước** lần retry, đổi command hoặc fallback tiếp theo; không được retry âm thầm.

---

## 11. Lưu ý khi bổ sung MCP method mới

Khi thêm/xóa/đổi tên MCP tool, cần đồng bộ ít nhất:

1. `crates/chatcmd-mcp/src/lib.rs` — thêm/sửa schema argument + tool description + handler trong rmcp router. `TOOL_NAMES` và canonical manifest sẽ tự sinh từ router này.
2. Runtime dispatch/handler tương ứng ở phía ChatCMD nếu method cần xử lý mới.
3. Xác định capability flags trong `tool_catalog.rs` nếu semantics mới không được rule hiện tại bao phủ.
4. Chạy invariant tests catalog/schema/dispatcher và `release_catalog_smoke`; mọi thay đổi contract hợp lệ phải làm `catalogHash` thay đổi.
5. Cập nhật tài liệu về semantics nếu cần, nhưng không copy lại full tool list để tránh drift.
