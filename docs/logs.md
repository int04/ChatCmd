# ChatCMD diagnostic logs

Tài liệu này mô tả helper log dùng chung của ChatCMD để ghi các lỗi hoặc trạng thái bất thường theo format dễ đọc cho người và AI.

## Format chuẩn

Mỗi log là một dòng:

```text
H:MM DD/MM/YYYY [\đường\dẫn\file.rs - line]: nội dung vấn đề
```

Ví dụ:

```text
8:40 26/06/2024 [\src\main.rs - 100]: Show lỗi ra ở đây
```

Giờ được lấy theo timezone local của máy chạy ChatCMD. Nếu runtime không xác định được local offset thì helper fallback về UTC thay vì làm hỏng luồng chính.

## Helper hiện tại

Helper nằm tại:

```text
src/log_helper.rs
```

API dùng chung:

```rust
crate::log_helper::log_issue(file!(), line!(), "Nội dung cần log");
```

Với dữ liệu động:

```rust
crate::log_helper::log_issue(
    file!(),
    line!(),
    &format!("request failed; taskId={task_id}; turnId={turn_id}"),
);
```

`file!()` và `line!()` phải được truyền tại đúng call site cần ghi log. Không bọc chúng trong một helper khác vì khi đó file/line sẽ trỏ vào helper thay vì vị trí phát sinh vấn đề.

## Nơi lưu log

Mặc định helper append vào:

```text
logs/chatcmd.log
```

Có thể đổi đường dẫn bằng biến môi trường:

```text
CHATCMD_LOG_PATH
```

Ví dụ:

```text
CHATCMD_LOG_PATH=D:\ChatCMD\logs\runtime.log
```

Helper tự tạo thư mục cha nếu chưa tồn tại và append từng dòng để giữ lại lịch sử cho việc debug hoặc xây UI xem log sau này.

## Quy tắc sử dụng

Chỉ log khi có dữ liệu hữu ích cho debug, ví dụ lỗi runtime, trạng thái bất thường, reject do race/validation, watchdog hoặc lỗi tích hợp. Nội dung nên nói rõ `vấn đề gì xảy ra` và correlation ID cần thiết như `taskId`, `turnId`, `requestId` nếu các ID đó thực sự giúp truy vết.

Không ghi password, access token, MCP token, Authorization header, secret key, cookie, nội dung nhạy cảm hoặc dữ liệu cá nhân không cần thiết. Nếu một giá trị có thể chứa secret thì phải redact trước khi truyền vào `log_issue`.

Không dùng helper này thay cho `Result`/error handling. Helper chỉ ghi chẩn đoán; luồng hiện tại vẫn phải return/propagate lỗi đúng cách.

## `active_tools_running`

`agent_turn_complete` hiện sử dụng helper khi finalizer bị reject vì trong cùng `taskId` + `turnId` vẫn còn activity đang chạy. Log có dạng tương tự:

```text
13:47 30/08/2026 [\src\runtime_host\agent_lifecycle.rs - 19]: active_tools_running: agent_turn_complete rejected; taskId=...; turnId=...
```

Mục đích là phân biệt trường hợp ChatGPT không gửi finalizer với trường hợp finalizer đã đến MCP server nhưng bị runtime reject.

## Khi bổ sung log cho chức năng khác

1. Xác định đúng điểm phát sinh lỗi hoặc trạng thái bất thường.
2. Gọi `crate::log_helper::log_issue(file!(), line!(), ...)` ngay tại điểm đó.
3. Nội dung phải đủ ngữ cảnh để AI đọc một dòng log vẫn hiểu lỗi đang xảy ra ở flow nào.
4. Chỉ thêm correlation ID cần thiết; không đưa secret vào message.
5. Giữ một event trên một dòng để UI/API sau này có thể đọc theo line.
6. Nếu thêm format mới, cập nhật tài liệu này và test formatter trong `src/log_helper.rs`.

## Hướng mở rộng UI/API sau này

Do log hiện là text append-only, UI xem log có thể triển khai theo hướng đọc `logs/chatcmd.log` hoặc đường dẫn `CHATCMD_LOG_PATH`, phân trang từ cuối file và stream các dòng mới qua WebSocket. Parser chỉ cần tách timestamp, source file, line và message dựa trên format chuẩn ở đầu tài liệu.

Nếu sau này cần filter theo level/module/task, nên mở rộng helper theo hướng thêm metadata có cấu trúc nhưng vẫn giữ format text hiện tại để AI và người có thể đọc trực tiếp.
