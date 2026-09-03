# Kế hoạch nâng cấp ChatCmdClient cho project lớn và file lớn

## Mục đích

Thư mục này tách toàn bộ hạng mục cần sửa thành các plan độc lập. Mỗi file được viết để có thể mở trong một cuộc trò chuyện AI mới mà không cần dựa vào lịch sử chat trước đó. AI thực thi phải đọc toàn bộ file plan tương ứng, tự kiểm tra lại source hiện tại rồi mới sửa.

Project áp dụng:

```text
/Users/ducnghia/Downloads/dev/ChatCmdClient
```

## Quy tắc chung cho mọi plan

- Tiếp tục trên working tree hiện tại; không làm mất thay đổi của người dùng.
- Không commit trừ khi người dùng yêu cầu riêng.
- Không sửa ngoài phạm vi plan đang thực hiện.
- Ưu tiên native Rust tool thay vì shell interpolation.
- File source mới hoặc đã tách nên giữ dưới 500 dòng; nếu logic lớn phải chia module theo chức năng.
- Giữ tương thích ngược khi hợp lý; nếu thay contract phải có version/migration rõ ràng.
- Không chỉ giới hạn dữ liệu trả về. Phải giới hạn cả I/O, RAM, CPU, thời gian, số file duyệt và dữ liệu persist/realtime.
- Mọi tác vụ dài phải hỗ trợ cancellation thực sự và giải phóng tài nguyên khi bị dừng.
- Chạy `cargo fmt --check`, `cargo check --workspace`, test liên quan và báo chính xác kết quả.
- Với thay đổi schema MCP, phải kiểm tra schema sinh ra, catalog runtime và binary/package thực tế, không chỉ unit test in-process.

## Thứ tự triển khai khuyến nghị

### P0 — nền tảng bắt buộc

1. [01-tool-catalog-release-consistency.md](01-tool-catalog-release-consistency.md)
2. [02-unified-tool-result-envelope.md](02-unified-tool-result-envelope.md)
3. [03-fs-read-text-streaming-range.md](03-fs-read-text-streaming-range.md)
4. [08-fs-stat-version-token.md](08-fs-stat-version-token.md)
5. [09-fs-apply-edits-versioned-range-edit.md](09-fs-apply-edits-versioned-range-edit.md)
6. [10-large-content-blob-transfer.md](10-large-content-blob-transfer.md)
7. [11-atomic-write-crash-safety-metadata.md](11-atomic-write-crash-safety-metadata.md)
8. [13-bounded-tool-persistence-and-artifacts.md](13-bounded-tool-persistence-and-artifacts.md)
9. [16-global-tool-budgets-cancellation-backpressure.md](16-global-tool-budgets-cancellation-backpressure.md)
10. [17-mcp-request-identity-without-body-rewrite.md](17-mcp-request-identity-without-body-rewrite.md)
11. [18-subagent-lease-heartbeat-watchdog.md](18-subagent-lease-heartbeat-watchdog.md)

### P1 — vận hành ổn định trên monorepo

12. [04-fs-list-cursor-pagination.md](04-fs-list-cursor-pagination.md)
13. [05-fs-find-scalable-traversal.md](05-fs-find-scalable-traversal.md)
14. [06-fs-search-v2-budget-cursor.md](06-fs-search-v2-budget-cursor.md)
15. [07-shared-ignore-rules-and-path-safety.md](07-shared-ignore-rules-and-path-safety.md)
16. [12-copy-move-delete-safety-rollback.md](12-copy-move-delete-safety-rollback.md)
17. [14-turn-file-change-tracker-scalability.md](14-turn-file-change-tracker-scalability.md)
18. [15-git-output-streaming-and-pagination.md](15-git-output-streaming-and-pagination.md)
19. [19-tool-observability-and-resource-metrics.md](19-tool-observability-and-resource-metrics.md)
20. [21-approval-batching-safe-read-policy.md](21-approval-batching-safe-read-policy.md)
21. [22-shell-output-coalescing-and-bulk-input-guard.md](22-shell-output-coalescing-and-bulk-input-guard.md)

### P2 — tối ưu nâng cao

22. [20-large-repo-index-and-batch-tools.md](20-large-repo-index-and-batch-tools.md)
23. [23-adversarial-tests-and-benchmarks.md](23-adversarial-tests-and-benchmarks.md)

## Quan hệ phụ thuộc chính

- Plan 09 cần `versionToken` từ plan 08.
- Plan 10 cung cấp `contentRef` cho plan 11 và các tool tạo/ghi file lớn.
- Plan 02 là contract chung cho các plan 03–06, 10, 13 và 15.
- Plan 16 cung cấp budget/cancellation dùng bởi traversal, Git, copy/move/delete và indexing.
- Plan 13 phải được phối hợp với plan 14 để không đưa full snapshot vào SQLite/WebSocket.
- Plan 07 nên được dùng chung bởi `fs_search`, `fs_find`, watcher và indexer.
- Plan 23 là suite tổng hợp; từng plan vẫn phải thêm unit/integration test ngay trong hạng mục của mình.

## Tiêu chí hoàn thành toàn chương trình

ChatCmdClient chỉ nên được coi là hỗ trợ tốt project/file lớn khi đạt đồng thời:

- Đọc một range nhỏ trong file rất lớn mà peak RAM không tỷ lệ với kích thước toàn file.
- Sửa range có optimistic concurrency và không ghi đè thay đổi đồng thời.
- Ghi binary/text lớn không nhét toàn bộ nội dung Base64 vào một JSON MCP request.
- Search/find/list có cursor, budget, early-stop và cancellation thực sự.
- Git/tool output lớn không được gom vô hạn vào RAM.
- Timeline/SQLite/WebSocket không lưu hoặc phát nguyên full content lớn.
- Copy/move/delete có bảo vệ symlink/TOCTOU, trạng thái partial rõ ràng và rollback/best-effort cleanup.
- Child agent mất kết nối không thể khóa parent vô hạn.
- Có benchmark/stress test tự động cho file lớn, repository lớn, concurrent writer và crash/interruption.
