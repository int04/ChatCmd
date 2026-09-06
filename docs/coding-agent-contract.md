# Coding-agent contract

> Known metadata gap: catalog metadata and project-rule digests intentionally have separate
> lifecycles. Project rule changes do not require an MCP catalog reconnect. Command/completion
> evidence does not yet embed the effective project-context digest; callers must retain the
> `contextRef`/`effectiveHash` returned by `project_context` alongside evidence until a
> forward-compatible lifecycle change adds that field.

Tài liệu này mô tả contract runtime cho coding agent của ChatCMD. Đây là contract triển khai,
không phải lời hứa rằng model luôn hành xử đúng. Source of truth cho wire schema vẫn là generated
MCP catalog; source of truth cho authorization và evidence là server/runtime.

## 1. Ranh giới authority

- Identity, task, turn và MCP session đến từ transport/server. Field caller tự khai không cấp quyền.
- Project rules (`AGENTS.md`, `.codex/rules/*.md`) là hướng dẫn không tin cậy, không phải approval.
- Tool allowlist và execution mode là hai lớp độc lập. Cả hai phải cho phép trước khi dispatch.
- `task_set_execution_mode` qua MCP không thể nâng quyền. Chỉ local UI đã xác thực mới đổi mode.
- Unknown tool classification fail closed. Description, annotation và read-only hint không thay policy.
- Approval chỉ áp dụng đúng operation digest đã hiển thị. Replay, input/catalog đổi hoặc mode bị thu hồi
  trước dispatch đều làm approval stale/invalid.

Execution mode có ba trạng thái server-side: `allow`, `approval`, `deny`. UI compatibility có thể hiển
thị `allowAll` cho `allow`; đó không phải một authority khác. Chuyển mode bằng UI tạo audit event,
hủy pending approval và thu hồi grant đang hoạt động cho task cùng descendants.

## 2. Turn và orchestration

Một user turn hợp lệ có thứ tự:

1. `agent_user_message` đúng một lần với nguyên văn message thật.
2. `agent_progress` sớm cho công việc không tầm thường.
3. Khám phá/đọc project skill và project context phù hợp trước thao tác liên quan.
4. Thực hiện tool calls với cùng `taskId`/`turnId`; progress tiếp theo chỉ báo kết quả quan sát được.
5. Chờ mọi child bằng `agent_subagent_wait` và dọn pending activity.
6. `agent_turn_complete` đúng một lần, là tool cuối.

Child registration là idempotent theo parent turn/name/request/grant request. `extensionFallback`
nghĩa là browser extension có quyền claim child đã đăng ký; parent không được làm trùng phần việc.
Child không tự kế thừa authority. Grant cho child phải là intersection có budget của một grant cha
đang active và bị ràng buộc với child attempt.

## 3. Clarification và execution consent

`agent_plan_question` mặc định là clarification. `questionKind=executionConsent` dùng state machine
server-defined, không suy quyền từ vị trí option hoặc text do model tự đặt.

- Chỉ lựa chọn đúng consent choice do server phát hành mới có thể cho kết quả approved.
- Reject, cancel, timeout, custom text, task/turn mismatch và cancellation đều fail closed.
- Consent là một lần dùng và chỉ áp dụng workflow đã chờ nó; nó không tạo reusable approval grant,
  không đổi execution mode và không bỏ qua C01 tool authorization.
- Clarification cũ không được diễn giải lại thành execution consent.
- Lifecycle được audit trong bảng `plan_questions`: question/task/turn/issuer/kind/scope digest,
  created/deadline và terminal resolution/resolved time. Terminal transition là compare-and-set nên
  answer, timeout, cancel và replay chỉ có một winner.
- Pending consent chỉ có receiver trong runtime hiện tại. Restart đánh dấu mọi pending record là
  expired; approved record cũ chỉ là audit history và không được nạp lại thành receiver hay authority.
- Scope digest hiện bind task, turn, issuer, kind, prompt và hai option. Plan revision cùng versioned
  project-scope snapshot chưa có trong lifecycle này; record lưu limitation đó và vì vậy không được
  dùng digest như một execution grant.

## 4. Filesystem, Git và command boundary

Mọi path được canonicalize và kiểm tra trong task workspace/path scopes. Symlink/reparse traversal,
workspace root deletion và path scope sibling đều fail closed. Mutation có precondition/version và
staging/atomic publish khi contract tool hỗ trợ; trạng thái partial hoặc source remaining phải được
báo thay vì giả rollback hoàn chỉnh.

Git dùng executable + argv, không shell interpolation. `git_commit` yêu cầu `paths` không rỗng hoặc
`all=true`, từ chối staged file ngoài scope và mixed staged/unstaged ambiguity.

`command_run` là process non-interactive có command boundary riêng:

- input bắt buộc `executable`, `cwd`; `arguments` là argv array;
- environment override bị giới hạn, không cho thay các biến authority/loader nhạy cảm;
- process chỉ spawn sau allowlist và C01 execution authorization;
- stdout/stderr preview, artifact, runtime và concurrency đều bounded;
- timeout/cancel dọn process tree theo primitive của platform;
- retry cùng owner và idempotency key quan sát/reuse execution cũ, không tự chạy lại;
- `executionId` và lookup bị ràng buộc task + agent.

Tool call trả thành công chỉ có nghĩa record đã được trả. Agent phải đọc `terminalState`, `exitCode`,
`signal`, `timedOut` và `cancelled`. Text `PASS`, fake prompt hoặc ANSI output không chứng minh command
thành công. Shell explicit chỉ cho aggregate exit của shell wrapper, không chứng minh từng lệnh con.
PTY vẫn dành cho interactive workflow và không tạo verified command evidence granular.

## 5. Completion quality và evidence

Completion có ba trục tách biệt:

| Trục | Giá trị |
| --- | --- |
| lifecycle | `running`, `completed`, `failed`, `cancelled` |
| work outcome | `completed`, `partial`, `blocked` |
| verification | `passed`, `failed`, `notRun`, `notApplicable`, `stale`, `unknown` |

`workOutcome` là khai báo của agent và được ghi provenance. `verification` do server tổng hợp từ
`command_run` execution IDs, ownership, persisted tool-result event, turn, exit/timeout/cancel,
declared scope và criterion mapping. Boolean hoặc terminal text do agent gửi không có authority.

Quy tắc fail-safe hiện tại:

- không có evidence: `notRun`, trừ docs/review khai `notApplicable` kèm reason;
- ref thiếu, sai owner hoặc record mất sau restart: diagnostic + `unknown`, không khóa finalization;
- non-zero exit, timeout hoặc cancel: `failed`;
- evidence từ turn cũ hoặc source state thay đổi: `stale`;
- criterion không có ref, scope trống hoặc evidence chưa chứng minh freshness: `unknown`;
- chỉ mọi criterion được cover bởi evidence fresh, server-owned mới có thể `passed`.

`command_run` lưu durable execution ownership/idempotency record và source fingerprint before/after.
Completed result có thể được reuse sau restart; record đang running khi host chết được đóng thành
`unknown/hostRestarted` và không tự chạy lại. Badge `passed` vẫn yêu cầu evidence fresh đúng task/turn,
scope và source hiện tại; orphan, thiếu fingerprint hoặc source đổi tiếp tục là `unknown`/`stale`.

## 6. Compatibility và migration

Thay đổi completion là additive, không có SQL migration mới: normalized quality report schema v1 được
lưu dưới dạng bounded timeline status event. Client cũ chỉ gửi `content` vẫn hoàn tất; server gán
`workOutcome=completed` với provenance `legacyDefault` và `verification=notRun`. Dữ liệu completion cũ
không được backfill thành verified.

Approval grants dùng append-only migration `0018_approval_grants.sql`; child grant request dùng
`0020_subagent_approval_inheritance.sql`. Plan-question audit dùng additive migration
`0021_plan_questions.sql`. Migration không backfill clarification/consent legacy và startup không
revive approved hay pending record cũ. Không sửa migration đã phát hành và không tạo grant từ
approval/consent cũ. Upgrade giữ task/event cũ nhưng authority cũ chỉ hợp lệ nếu record mới đáp ứng
owner, scope, catalog hash, expiry, counters và active state.

Catalog thêm field/tool theo hướng additive nhưng schema/capability change làm đổi `catalogHash`.
Client phải reconnect, initialize và list tools lại; chỉ retry operation một lần sau refresh.
Behavior wording có `instructionsVersion`/`instructionsHash` riêng.

## 7. Rollout và rollback

Rollout đề xuất:

1. Chạy migration/storage và catalog smoke trên database mới lẫn bản nâng cấp được hỗ trợ.
2. Deploy server/runtime trước; client legacy tiếp tục dùng completion tối thiểu.
3. Bật UI quality card ở chế độ conservative; không hiển thị passed khi status unknown/stale/notRun.
4. Theo dõi catalog mismatch, pending approval, evidence diagnostics, timeout/cancel và artifact quota.
5. Chỉ bật verified experience rộng sau khi restart recovery, source fingerprint và UI smoke có test
   trên platform phát hành.

Rollback binary không được downgrade database schema. Dùng backup và binary hiểu schema hiện tại;
ChatCMD chủ động từ chối database mới hơn binary. Khi rollback UI/client, server vẫn giữ additive
fields/timeline events và legacy client có thể bỏ qua chúng. Không tái kích hoạt pending consent hay
approval cũ trong quá trình rollback.

## 8. Release acceptance

Các gate tự động tối thiểu:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p chatcmd-mcp --test release_catalog_smoke
cargo test -p chatcmd-mcp --test coding_behavior_harness
cd web && npm ci && npm run lint && npm test -- --run && npm run build
cd chatgpt-extension && node --test content-chatgpt.test.cjs
```

Desktop DEV release workflow phải chỉ chạy bằng `workflow_dispatch`. Adversarial workflow có thể chạy
PR/schedule/manual để tạo test evidence nhưng không được publish release. Trước phát hành cần manual
smoke trên clean Windows/macOS: MCP initialize/list/call, approval allow/reject/revoke, deny-before-spawn,
command exit 0/1/timeout/cancel, task completion quality, restart/migration và extension flow.

Tier C live-model smoke đang **BLOCKED / NOT RUN**: repository không có isolated live host/model/quota
configuration hay redacted transcript artifact cho lượt này. Không gắn nhãn “live validated” cho đến
khi có ít nhất ba lần chạy cùng seed/cấu hình và lưu model/host/version/date, instruction hash, transcript
đã redact, source diff cùng execution evidence.
