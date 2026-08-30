# ChatCMD MCP Methods

Tài liệu này liệt kê các MCP tool/method mà `chatcmd-mcp` hiện expose để agent gọi về ChatCMD server/runtime.

Nguồn đối chiếu chính:

- `crates/chatcmd-mcp/src/tool_catalog.rs` — danh sách method ổn định được expose.
- `crates/chatcmd-mcp/src/lib.rs` — schema/description của từng method.

> Tổng số hiện tại: **45 methods**.

## Quy ước chung

Phần lớn method đều có các trường correlation chung do ChatCMD bổ sung:

- `taskId`: ID của ChatCMD task/conversation hiện tại.
- `turnId`: ID của user turn hiện tại. Mọi tool trong cùng một turn phải dùng cùng `turnId`.
- `agentId`: định danh agent gọi tool; server có thể ghi đè bằng authenticated identity.
- `requestId`: khóa idempotency do caller sinh; thường có thể bỏ qua để ChatCMD tự sinh.

Luồng agent bắt buộc:

1. `agent_user_message` phải là tool đầu tiên của mỗi user turn.
2. Với công việc project không tầm thường, gọi `skills_list`; nếu có skill phù hợp thì đọc bằng `skill_read` trước khi thao tác liên quan.
3. Thực hiện các MCP method cần thiết.
4. Nếu có sub-agent thì phải chờ chúng hoàn tất bằng `agent_subagent_wait`.
5. `agent_turn_complete` phải là tool cuối cùng, gọi đúng một lần ngay trước khi agent trả lời user.

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
| `fs_list` | `path`, `offset?`, `limit?` | Liệt kê file/thư mục con bên trong một path. |
| `fs_search` | `path`, `query`, `caseSensitive?`, `maxResults?`, `maxFileBytes?`, `includeIgnored?`, `exclude?` | Tìm kiếm **nội dung text** trong workspace. Khi tìm từ root nên dùng `path: "."`. |
| `fs_find` | `path`, `pattern`, `maxResults?`, `maxDepth?` | Tìm **đường dẫn/tên file hoặc thư mục** theo pattern. Nên dùng khi chưa chắc relative path thay vì đoán path. |
| `fs_read_text` | `path`, `startLine?`, `lineCount?`, `maxCharacters?` | Đọc file text UTF-8; hỗ trợ đọc theo range để tránh tải file lớn toàn bộ. |
| `fs_write_text` | `path`, `content`, `overwrite?` | Ghi nguyên tử nội dung UTF-8 vào file; dùng cho tạo mới hoặc thay toàn bộ file. |
| `fs_replace_text` | `path`, `oldText`, `newText`, `expectedOccurrences?` | Chỉnh sửa an toàn bằng exact text replacement. `oldText` phải khớp nội dung hiện tại. |
| `fs_write_raw` | `path`, `base64`, `overwrite?` | Decode Base64 và ghi atomically dữ liệu binary/raw vào workspace. |
| `fs_stat` | `path` | Xem metadata của một file/thư mục: loại entry, size, readonly, v.v. |
| `fs_create_directory` | `path` | Tạo thư mục trong workspace. |
| `fs_copy` | `source`, `destination`, `overwrite?` | Copy file/thư mục trong canonical workspace scope. |
| `fs_move` | `source`, `destination`, `overwrite?` | Move/rename file hoặc thư mục trong canonical workspace scope. |
| `fs_delete` | `path`, `recursive?` | Xóa file/thư mục theo policy; thư mục có thể cần `recursive`. |

### Phân biệt nhanh `fs_search` và `fs_find`

- `fs_search`: tìm **chuỗi trong nội dung file**.
- `fs_find`: tìm **file/folder/path** theo tên/pattern.

---

## 4. Git methods

Các Git method được thiết kế để tránh shell interpolation và truyền argument có kiểm soát.

| Method | Tham số chính | Ý nghĩa |
|---|---|---|
| `git_status` | `cwd?` | Xem trạng thái working tree của repository. `path` cũ được chấp nhận như alias của `cwd`. |
| `git_diff` | `cwd?`, `staged?`, `stat?`, `path?` | Lấy Git diff; có thể lọc staged, chỉ stat hoặc theo một file cụ thể. |
| `git_log` | `cwd?`, `count?`, `path?` | Xem lịch sử commit có giới hạn số lượng; có thể lọc theo path. |
| `git_branch` | `cwd?` | Liệt kê các Git branch. |
| `git_show` | `revision`, `cwd?`, `path?` | Xem nội dung của một revision/commit đã được validate; có thể lọc theo path. |
| `git_commit` | `message`, `cwd?`, `all?`, `paths?` | Tạo Git commit mà không dùng shell interpolation. `all` mặc định là `true`; không được gọi với object rỗng. |

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
| `agent_user_message` | `content` | **Bắt buộc là MCP call đầu tiên của mỗi user turn.** Đồng bộ nguyên văn user message lên ChatCMD và thiết lập/correlate `taskId` + `turnId`. `content` phải đúng nguyên văn message hiện tại. |
| `agent_progress` | `message`, `suggestedTitle?` | Gửi một progress milestone ngắn để UI/server biết agent đang làm tới đâu. Không gọi sau `agent_turn_complete`. |
| `agent_subagent_start` | `name`, `request` | Tạo và dispatch một child agent khi ChatGPT chủ động chia việc hoặc người dùng yêu cầu chia agent. Chỉ sử dụng model sampling do ChatGPT/MCP host cung cấp; nếu host không hỗ trợ sampling thì trả `samplingUnavailable`/`failed` và tuyệt đối không khởi chạy Codex hay executor local. |
| `agent_subagent_wait` | `timeoutMs?` | Chờ các child agent của parent turn. Nếu `allFinished=false` thì tiếp tục gọi lại trước khi finalize. |
| `agent_turn_complete` | `content`, `suggestedTitle?` | **Bắt buộc là MCP call cuối cùng.** Xác nhận turn đã hoàn tất và gửi đúng nội dung cuối cùng agent sẽ trả cho user. Chỉ được gọi đúng một lần sau khi mọi tool/sub-agent đã xong. |

---

## 9. Danh sách đầy đủ theo thứ tự server expose

Thứ tự ổn định hiện tại trong `TOOL_NAMES`:

```text
01. device_list
02. device_get
03. shell_create
04. shell_write
05. shell_wait
06. shell_read
07. shell_signal
08. shell_resize
09. shell_close
10. shell_list
11. shell_inspect
12. workspace_roots
13. fs_list
14. fs_search
15. fs_find
16. fs_read_text
17. fs_write_text
18. fs_replace_text
19. fs_write_raw
20. fs_stat
21. fs_create_directory
22. fs_copy
23. fs_move
24. fs_delete
25. git_status
26. git_diff
27. git_log
28. git_branch
29. git_show
30. git_commit
31. process_list
32. process_inspect
33. process_kill
34. skills_list
35. skill_read
36. task_get
37. task_list
38. task_set_execution_mode
39. task_artifact_list
40. task_artifact_read
41. agent_user_message
42. agent_progress
43. agent_subagent_start
44. agent_subagent_wait
45. agent_turn_complete
```

---

## 10. Luồng gọi mẫu

Một turn sửa code thông thường có thể có flow:

```text
agent_user_message
  -> skills_list
  -> skill_read                  (nếu có skill phù hợp)
  -> workspace_roots / fs_find
  -> fs_read_text / fs_search
  -> fs_replace_text / fs_write_text
  -> git_diff / git_status       (nếu cần kiểm tra thay đổi)
  -> agent_progress              (tùy công việc dài)
  -> agent_turn_complete
```

Một turn có sub-agent:

```text
agent_user_message
  -> skills_list
  -> agent_subagent_start
  -> ... parent tiếp tục công việc ...
  -> agent_subagent_wait
  -> agent_subagent_wait         (lặp nếu allFinished=false)
  -> agent_turn_complete
```

Một turn chạy terminal dài:

```text
agent_user_message
  -> shell_create
  -> shell_write
  -> shell_read / shell_wait
  -> shell_read                  (đọc tiếp bằng afterSequence)
  -> shell_close                 (nếu cần đóng session)
  -> agent_turn_complete
```

---

## 11. Lưu ý khi bổ sung MCP method mới

Khi thêm/xóa/đổi tên MCP tool, cần đồng bộ ít nhất:

1. `crates/chatcmd-mcp/src/tool_catalog.rs` — cập nhật `TOOL_NAMES`.
2. `crates/chatcmd-mcp/src/lib.rs` — schema argument + tool description + handler/router.
3. Runtime dispatch/handler tương ứng ở phía ChatCMD nếu method cần xử lý mới.
4. Test liên quan tới tool catalog/schema/dispatch.
5. Cập nhật lại file `docs/mcp_method.md` này để tài liệu không lệch code.
