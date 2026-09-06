# Plan 18 — Thêm lease, heartbeat và watchdog cho sub-agent trạng thái `running`

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy sửa lifecycle sub-agent để một child đã chuyển sang `running` nhưng mất worker/finalizer không thể khóa parent vô hạn. Thêm lease/heartbeat/max runtime, watchdog expire, terminal state `timedOut`/`interrupted`, cancellation cleanup và xử lý race idempotent. Không commit.

## Ưu tiên

**P0 — orchestration reliability.** Audit thực tế đã gặp child đứng `running`, làm parent không thể `agent_turn_complete` cho đến khi dừng thủ công.

## Bằng chứng hiện tại cần kiểm tra lại

- `src/runtime_host/subagents.rs:270-326` `wait_for_subagents` poll trạng thái và mỗi lần chỉ chờ tối đa 40 giây; nếu còn running chỉ yêu cầu gọi lại.
- `subagents.rs:328-367` `ensure_subagents_finished` từ chối finalization khi child pending/running.
- `subagents.rs:417-451` `expire_unclaimed_subagents` chỉ query `status='pending'`; child `running` không được expire ở đây.
- Constants đầu file trước audit có timeout 60 giây cho native pending và 180 giây cho extension fallback pending; cần đọc lại line hiện tại.
- `src/runtime_host/finalization_watchdog.rs:52-96` bỏ qua auto-finalize khi `has_active_subagent_work` thấy `pending` hoặc `running`.
- Không thấy `lease_expires_at`, heartbeat timestamp, owner worker ID hoặc `max_runtime` cho running child.

## Mục tiêu

1. Mỗi sub-agent run có lease hữu hạn và owner/attempt rõ.
2. Worker đang sống gia hạn lease bằng heartbeat từ hoạt động thực tế; không cần spam timeline.
3. Watchdog chuyển child stale sang terminal state và giải phóng parent/concurrency slot.
4. Parent finalization không bị khóa vô hạn bởi stale `running` row.
5. Completion và timeout chạy đồng thời phải idempotent; không biến completed thành failed/timedOut.
6. Extension fallback/native sampling/host-native worker đều dùng semantics lease phù hợp.
7. App restart reconcile được run pending/running cũ.
8. UI hiển thị rõ `pending`, `running`, `timedOut`, `failed`, `stopped`, `completed`, retry attempt và lý do.

## Schema/state đề xuất

Mở rộng `subagent_runs` với fields tương đương:

```text
worker_id TEXT NULL
attempt INTEGER NOT NULL DEFAULT 0
lease_acquired_at_ms INTEGER NULL
lease_expires_at_ms INTEGER NULL
last_heartbeat_at_ms INTEGER NULL
max_runtime_ms INTEGER NOT NULL
started_at_ms INTEGER NULL
terminal_reason TEXT NULL
```

Nếu dùng migration khác, vẫn phải bảo đảm query/index:

- status + lease_expires_at_ms;
- parent_task_id + parent_turn_id + status;
- child_task_id unique/lookup;
- worker_id/attempt cho compare-and-set.

State machine:

```text
pending -> running -> completed|failed|stopped|timedOut|interrupted
pending -> failed|timedOut|stopped
running -> pending (chỉ khi retry policy explicit, attempt tăng)
```

Không cho terminal → running trở lại cùng attempt.

## Lease semantics

- Worker claim dùng SQL compare-and-set: chỉ claim pending hoặc stale retryable state theo policy.
- Khi claim, set `worker_id`, `attempt`, `started_at`, `lease_expires_at`.
- Heartbeat chỉ gia hạn nếu `id + worker_id + attempt + status='running'` khớp.
- Lease duration đủ lớn hơn heartbeat interval và chịu jitter; ví dụ heartbeat 10–15 giây, lease 45–60 giây. Giá trị phải config/validate.
- `max_runtime_ms` là hard deadline tính từ started, không được heartbeat gia hạn vượt vô hạn trừ explicit override policy.
- Hoạt động child MCP hợp lệ (`agent_progress`, tool started/finished, final response) có thể piggyback heartbeat; thêm timer heartbeat nếu child đang suy nghĩ lâu không gọi tool.
- Heartbeat không tạo timeline event mỗi lần; chỉ update row/metric, log sample khi lỗi.

## Watchdog behavior

Tạo worker định kỳ:

1. Query bounded batch `status='running'` với lease expired hoặc hard deadline exceeded.
2. CAS transition sang `timedOut`/`interrupted`, set reason/completed time.
3. Cancel active activity/process/terminal thuộc child nếu còn local handle.
4. Reconcile orphaned tool calls của child.
5. Update child task terminal state và clear active session.
6. Publish một terminal event cho child và subagent status cho parent.
7. Parent `wait_for_subagents` thấy terminal state và được finalize.
8. Release admission/concurrency accounting.

Nếu cancellation/cleanup chưa hoàn tất, có thể dùng intermediate `timingOut` nhưng phải có watchdog retry và terminal deadline; không để intermediate khóa vô hạn.

## Retry policy

- Không tự retry arbitrary coding agent vô hạn.
- Retry chỉ khi dispatch/worker acquisition failure hoặc policy explicit, tối đa attempts hữu hạn.
- Mỗi attempt có worker ID/lease riêng.
- Retry không tạo child task/subagent duplicate; giữ same logical subagent ID, attempt tăng và event rõ.
- Extension fallback attempts hiện có phải được hợp nhất với lease state, tránh hai watchdog cạnh tranh.

## Parent finalization

- `ensure_subagents_finished` phải gọi expire/reconcile stale running trước khi quyết định block.
- Nếu child timed out, parent có thể finalize và nhận summary `allFinished=true`, `allCompleted=false` cùng terminal reason.
- `agent_subagent_wait` nên trả `nextPollAfterMs`, counts, earliest lease expiry và bounded child summary; không cần busy poll 250 ms liên tục nếu DB/event notification có thể dùng.
- Có thể dùng `Notify`/watch channel cho state change để giảm polling; vẫn giữ timeout fallback.

## App restart

Startup reconcile:

- Running rows từ process cũ không được coi đang sống chỉ vì lease chưa hết quá lâu; worker instance ID/boot ID giúp quyết định.
- Có thể expire ngay run owned bởi old boot ID hoặc đợi short grace.
- Close/reconcile terminal sessions và orphan tool calls.
- Pending fallback requests theo existing retry logic nhưng không duplicate browser child.

## Các bước triển khai

1. Đọc toàn bộ schema/migrations/repository cho `subagent_runs`, task status và concurrency counting.
2. Viết reproduction integration test child claim rồi mất worker; hiện test phải chứng minh parent bị block.
3. Thêm migration/index và typed lease config.
4. Implement claim/heartbeat/CAS terminal transitions.
5. Hook heartbeat vào child worker/sampling/extension fallback activity.
6. Implement running watchdog + cleanup/reconcile.
7. Sửa `wait_for_subagents`, `ensure_subagents_finished`, finalization watchdog và concurrency counter.
8. Thêm startup reconcile/boot ID.
9. Cập nhật events/API/UI/i18n/docs.
10. Thêm metrics: active leases, expired, heartbeat lag, runtime, retry attempts.

## Race/edge cases bắt buộc

- Completion đúng lúc watchdog expire.
- Heartbeat chậm đến sau terminal transition.
- Hai watchdog instances/process restart cùng scan.
- Worker A lease hết, worker B retry claim; A quay lại gửi result.
- Parent stopped/deleted trong khi child running.
- Child creates terminal/process rồi mất worker.
- Database busy/error trong heartbeat.
- System sleep/resume; dùng wall clock cho persisted lease nhưng xử lý jump hợp lý.
- Max runtime reached dù heartbeat vẫn đều.
- Extension fallback claimed sau timeout.
- Child task completed nhưng `subagent_runs` chưa update và ngược lại.

## Test bắt buộc

- Pending unclaimed timeout hiện có không regress.
- Running lease expires → child terminal → parent finalizable.
- Heartbeat giữ child alive trước hard deadline.
- Hard max runtime thắng heartbeat.
- CAS completion-vs-timeout chỉ có một terminal winner.
- Stale worker result bị từ chối bằng worker ID/attempt.
- App restart reconcile old running rows.
- Parent stop cascades cancel/terminal state.
- Concurrency slot được giải phóng sau timeout.
- `agent_subagent_wait` không busy spin và trả summary đúng.
- UI tests cho timed out/retry/reason.

## Tiêu chí nghiệm thu

- Không có `running` row có thể sống vô hạn mà không heartbeat/max runtime.
- Parent không bị block sau lease expiry và cleanup.
- Heartbeat/terminal transitions là idempotent/CAS-safe.
- Worker cũ không thể hoàn tất attempt mới.
- Startup recovery không duplicate child.
- UI và events phân biệt timeout với user stop/failure.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test --workspace
```

Chạy frontend test/build nếu sửa UI.

## Kết quả AI phải trả về

- Migration/schema/index mới.
- Lease/heartbeat/default timing và lý do.
- State machine/CAS queries.
- Watchdog/cleanup/startup recovery flow.
- File/UI đã đổi.
- Race/integration test và kết quả.

## Kiểm tra còn lại sau triển khai

Lượt rà soát ngày 2026-09-04 đã chạy lại toàn bộ validation và xử lý các blocker có thể khắc phục trong môi trường hiện tại:

- `cargo fmt --check`, `cargo check --workspace`, `cargo test --workspace`: đạt; các test lease expiry, heartbeat, hard runtime, CAS completion-vs-timeout và restart reconcile đều đạt.
- `npm test -- --run`: đạt 15/15 test files, 54/54 tests, gồm `src/test/App.test.tsx` và `src/tasks/subagentStatus.test.ts`.
- `npm run build`: đạt.
- `npm run lint`: đạt sau khi sửa cấu hình Node globals cho script build, cập nhật realtime listener ref ngoài render và ổn định dependency của các React hooks liên quan.

## Hoàn tất kiểm tra bổ sung 2026-09-04

Hai gap cuối đã được xử lý bằng integration harness an toàn, không cần suspend hoặc đổi đồng hồ hệ thống thật của máy phát triển:

- **Watchdog timeout đóng process/PTY thật:** `cleanup_timed_out_subagent` giờ tìm toàn bộ terminal đang `starting/running` của child và force-close qua `ShellRuntime::close(..., force=true)` trước khi đánh dấu DB `interrupted`. Test `watchdog_timeout_force_closes_real_child_pty_process` tạo PTY/process thật (`/bin/sh -c "sleep 60"`), làm lease hết hạn, chạy watchdog và xác nhận PID không còn tồn tại đồng thời `terminal_sessions.status='interrupted'`.
- **Suspend/resume / system-clock jump semantics:** watchdog được tách thêm `expire_stale_subagents_at(..., now)` để test deterministic persisted wall-clock deadline mà không đổi clock OS. Test `persisted_deadlines_handle_backward_and_forward_clock_jumps` xác nhận clock lùi không timeout lease sớm và clock tiến/resume vượt persisted lease/hard deadline sẽ timeout đúng.

Validation cuối:

- `cargo fmt --check`: đạt.
- `cargo check --workspace`: đạt.
- `cargo test --workspace`: đạt, bao gồm hai integration test mới và toàn bộ test Plan 18 hiện có.
- Frontend không thay đổi thêm trong lượt này; validation frontend đầy đủ (`npm test -- --run`, `npm run build`, `npm run lint`) đã đạt ở lượt rà soát ngay trước đó.

Plan 18 hiện không còn mục chưa hoàn thiện đã biết và đủ điều kiện `_Checkdone`.
