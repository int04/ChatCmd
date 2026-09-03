# Plan 21 — Tối ưu approval cho project lớn bằng scoped grants và batch approval an toàn

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy thiết kế lại approval policy để AI làm việc trên repository lớn không phải xin phép lặp lại cho từng read/stat/list/search vô hại, nhưng tuyệt đối không biến thành blanket unrestricted access. Thêm phân loại tool, task/turn/path-scoped grant, budget-scoped approval, batch approval summary, expiry/revocation và audit log. Không commit.

## Ưu tiên

**P1 — UX và throughput, nhưng security-sensitive.** Project lớn cần hàng chục/hàng trăm read/search/stat calls; approval từng call gây overhead rất lớn. Mutation/destructive operations vẫn phải giữ kiểm soát mạnh.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/approval.rs` chứa `requires_execution_approval` và test trước audit cho thấy `fs_read_text` thuộc nhóm cần approval; đọc lại toàn file và schema hiện tại.
- `src/runtime_host/persistence.rs:14-45` gọi `authorize_execution` trước register/started event.
- `src/runtime_host/dispatch.rs:55-105` tạo task path scopes từ project folder và explicit user path grants; đây là cơ sở scope approval.
- `src/api/task_controls.rs` và route approval APIs quản lý pending/resolution; cần audit transaction/idempotency/lifecycle.
- Settings execution mode có `approval`, `allow`, `deny`; hiện granularity chủ yếu theo task/tool allowlist.
- User-message path grant tests tại `src/runtime_host/user_message_path_tests.rs` phải được giữ và mở rộng; child/subagent inheritance cần kiểm tra.

## Mục tiêu

1. Read-only, metadata-only và mutation/destructive được phân loại bằng manifest/capability typed, không dựa vào tên string rải rác.
2. Một approval có thể cấp quyền có giới hạn cho nhiều safe read calls trong đúng task/project/path và thời hạn.
3. Approval luôn kèm resource budgets; không cho approved search quét vô hạn.
4. Mutation approval hiển thị summary cụ thể: paths, operation, bytes/files estimate, overwrite/delete, expectedVersion, rollback/atomicity—not full content.
5. Batch approval không widen path hoặc tool ngoài nội dung user nhìn thấy.
6. Grants có expiry, usage count/byte cap, revocation và persistence/restart semantics rõ.
7. Child agent không tự động nhận quyền rộng hơn parent; inheritance explicit và least privilege.
8. Mọi use/deny/expire/revoke được audit, không log secret/content.

## Phân loại capability đề xuất

Đưa vào tool manifest plan 01:

```rust
enum ToolRiskClass {
    MetadataRead,
    ContentRead,
    ComputeRead,
    Create,
    Modify,
    MoveCopy,
    Destructive,
    ProcessExecution,
    Privileged,
}

ToolCapabilities {
    risk_class: ToolRiskClass,
    path_fields: Vec<PathFieldRole>,
    supports_budget: bool,
    supports_dry_run: bool,
    supports_expected_version: bool,
}
```

Ví dụ:

- `workspace_roots`, `fs_stat`: MetadataRead.
- `fs_list`, `fs_find`: Metadata/ComputeRead.
- `fs_read_text`, `fs_search`: ContentRead/ComputeRead.
- `fs_write_text`, `fs_apply_edits`: Modify/Create.
- `fs_copy`, `fs_move`: MoveCopy.
- `fs_delete`: Destructive.
- `shell_create`, `process_kill`, `git_commit`: ProcessExecution/Destructive tùy semantics.

Không tự coi Git read command hoàn toàn vô hại nếu hooks/config/external diff có thể chạy; plan 15 phải disable unsafe extensions hoặc classification phản ánh thực tế.

## Grant model đề xuất

```rust
ApprovalGrant {
    id: GrantId,
    owner_agent_id: AgentId,
    task_id: TaskId,
    turn_id: Option<TurnId>,
    allowed_tools: ToolSet,
    path_scopes: Vec<PathScope>,
    max_calls: u64,
    max_files_scanned: Option<u64>,
    max_bytes_read: Option<u64>,
    max_bytes_written: Option<u64>,
    expires_at_ms: i64,
    inherited_from: Option<GrantId>,
    state: Active|Revoked|Expired|Exhausted,
}
```

`PathScope` phân biệt exact file và directory subtree, bind canonical root/file identity khi phù hợp. Không dùng plain string prefix chưa canonicalize. Grant không được cấp filesystem root hoặc parent/sibling ngoài user-selected project/path.

## Safe-read flow đề xuất

1. User phê duyệt conversation/project hoặc một request đầu tiên với preview rõ:
   - tool classes được phép;
   - exact project root/path scopes;
   - thời hạn;
   - call/byte/file budgets;
   - có/không include ignored/hidden.
2. Runtime tạo grant task-scoped.
3. Các call read-only tiếp theo match capability + scope + remaining budget thì không popup lại.
4. Mỗi call atomically consume usage/budget.
5. Path/tool/options vượt grant → popup approval mới, không tự widen.
6. Mutation/destructive không được dùng safe-read grant.

Có thể cung cấp preset:

- `Read project metadata`;
- `Read/search project files`;
- `Modify selected files`;
- `Run approved build/test command`.

Preset chỉ là UI template, server vẫn lưu explicit capabilities/scopes/budgets.

## Batch mutation approval

Với `fs_apply_edits` hoặc multi-file operation:

- AI gửi dry-run/preflight summary.
- Approval hiển thị exact paths, count, additions/deletions/estimated bytes, expected versions và conflict behavior.
- User có thể approve toàn batch; runtime bind approval với operation digest canonical.
- Nếu inputs/path/version/options thay đổi sau approval, digest mismatch và phải xin lại.
- Không lưu/full-render replacement content lớn; preview bounded + artifact lazy view.

Copy/move/delete recursive approval phải hiển thị estimate + permanent/quarantine + rollback guarantee. Nếu preflight incomplete do budget, UI phải nói rõ, không dùng wording chắc chắn.

## Grant inheritance cho sub-agent

- Mặc định child chỉ nhận subset tối thiểu cần cho delegated request.
- Parent registration có thể chỉ định requested capabilities/scopes; server intersect với parent active grant.
- Child không nhận destructive/process permission nếu parent grant chỉ read.
- Grant bind child task/attempt/lease; hết child thì expire.
- Extension fallback không được biến grant thành global Agent permission.
- UI/audit hiển thị inherited grant và parent source.

## Persistence, expiry và revocation

- Approval/grant state update transactionally.
- Expiry dùng wall time persisted; call authorization consume phải CAS để không vượt maxCalls trong concurrency.
- App restart giữ grant chỉ nếu policy cho và chưa expiry; ephemeral turn grant có thể expire ngay.
- Stop conversation/revoke agent/delete task hủy grants liên quan.
- Rotate token/disable agent không để grant cũ dùng tiếp.
- UI có nút revoke hiện thời; revoke ảnh hưởng call chưa bắt đầu, active mutation dùng cancellation/rollback semantics plan 16.

## Các bước triển khai

1. Audit approval schema/queries/UI và lập risk matrix toàn catalog.
2. Chuyển risk metadata vào single tool manifest; bỏ string match duplicate.
3. Tạo grant schema/types/repository với indexes và atomic budget consumption.
4. Implement matcher capability + path scope + option constraints.
5. Implement safe-read project grant và migration từ execution mode hiện tại.
6. Implement operation digest/batch mutation approval.
7. Implement child grant intersection/inheritance.
8. Implement expiry/revocation/startup/task-stop cleanup.
9. Cập nhật pending approval API/UI/i18n/sound/UX mà không spam.
10. Thêm audit events/metrics redacted và docs security model.

## Security/edge cases bắt buộc

- Relative `../`, absolute path, symlink/junction swap sau grant.
- Exact file grant dùng cho sibling hoặc renamed path.
- Directory moved/replaced sau grant.
- `includeIgnored=true`/hidden option không nằm trong approved scope.
- Concurrent calls tiêu thụ call/byte cap.
- Grant hết hạn trong lúc call queued/approval open.
- Mutation request thay content/path sau user approve.
- Parent/child privilege escalation.
- Agent disabled/token rotated/task stopped.
- Approval replay bằng requestId cũ hoặc task khác.
- Tool catalog/risk metadata đổi sau grant; catalog version mismatch phải invalidate/re-evaluate.

## Test bắt buộc

- Safe read sequence chỉ prompt một lần trong đúng scope/budget.
- Path/tool/risk/options vượt scope prompt/deny.
- Metadata grant không đọc content.
- Read grant không sửa/xóa/run shell.
- Atomic budget consumption dưới concurrent calls.
- Expiry/revoke/restart/task stop behavior.
- Operation digest mismatch khi bất kỳ mutating field thay đổi.
- Child inheritance chỉ là intersection; child stale attempt không dùng grant.
- Symlink/path scope tests tích hợp plan 07.
- UI snapshot/e2e cho approval summary, incomplete preflight và revoke.
- Audit/log không chứa full content/secrets.
- Backward compatibility/migration execution mode hiện tại.

## Tiêu chí nghiệm thu

- Read workflow trên project lớn không cần popup từng call trong approved bounds.
- Không tồn tại blanket grant ngầm cho toàn filesystem hoặc mọi tool.
- Tool risk/capabilities có một nguồn chân lý.
- Grant bind task/agent/path/tool/options/budgets/expiry và consume atomically.
- Mutation batch bind canonical digest và version/preflight summary.
- Child không nâng quyền.
- User có thể xem/revoke grant, audit rõ.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Chạy frontend typecheck/test/build cho approval UI.

## Kết quả AI phải trả về

- Risk matrix toàn catalog.
- Grant schema/matcher/budget precedence.
- UI approval flow trước/sau.
- Child inheritance và invalidation rules.
- File/migration đã đổi.
- Security/race/e2e test và kết quả.
