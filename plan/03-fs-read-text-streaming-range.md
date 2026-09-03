# Plan 03 — Viết lại `fs_read_text` thành đọc streaming/range thực sự

## Nhiệm vụ dùng cho chat mới

Trong project `/Users/ducnghia/Downloads/dev/ChatCmdClient`, hãy sửa `fs_read_text` để đọc file lớn theo range thực sự, không tải toàn file vào RAM rồi mới cắt. Giữ contract cũ tương thích trong giai đoạn chuyển tiếp và bổ sung contract mới nếu cần. Không sửa các tool mutation ngoài phần dùng chung metadata/version. Không commit.

## Ưu tiên

**P0 — blocker cho file lớn.** API hiện có `startLine`, `lineCount`, `maxCharacters`, nhưng implementation vẫn đọc và decode toàn bộ file.

## Bằng chứng hiện tại cần kiểm tra lại

- `crates/chatcmd-runtime/src/filesystem.rs:139-208`:
  - gọi `tokio::fs::read(&resolved)`;
  - tạo `String` cho toàn file;
  - gọi `content.lines().count()`;
  - với range còn collect toàn bộ `Vec<&str>` trước khi skip/take.
- `src/runtime_host/inputs.rs:195-222` chỉ có `path`, `maxCharacters`, `startLine`, `lineCount`.
- `crates/chatcmd-mcp/src/lib.rs:310-345` mô tả “prefer line ranges”, nhưng range hiện không giảm I/O/RAM tương ứng.
- `src/runtime_host/filesystem_dispatch.rs:153-160` snapshot gọi `read_text(path, 1_000_000)`, nên cùng nhược điểm.

## Mục tiêu

1. Đọc vài chục dòng trong file hàng trăm MB/GB mà peak RAM phụ thuộc vào buffer và output, không phụ thuộc toàn file.
2. Hỗ trợ hai chế độ rõ ràng:
   - range theo dòng cho LLM;
   - range theo byte cho continuation/large generated file.
3. Trả metadata đủ để tiếp tục: `nextCursor` hoặc `nextStartLine`/`nextByteOffset`, `truncated`, `bytesRead`, `versionToken`, encoding và line-ending detection.
4. Cancellation/time budget được kiểm tra trong vòng đọc, không chỉ ở wrapper `tokio::select!`.
5. Không phá CRLF, không chia đôi UTF-8 code point và không báo sai `endLine`/`totalLines`.
6. Có hành vi rõ với BOM, invalid UTF-8, binary và line cực dài.

## Phạm vi contract đề xuất

Ưu tiên thêm `fs_read_text_v2` hoặc mở rộng tương thích nếu plan 02 đã có migration strategy:

```json
{
  "path": "src/large.rs",
  "range": {
    "unit": "line",
    "start": 1000,
    "limit": 200
  },
  "maxBytes": 262144,
  "encoding": "auto",
  "includeLineEndings": false,
  "expectedVersion": null,
  "budget": {
    "timeoutMs": 10000,
    "maxBytesRead": 8388608
  }
}
```

Kết quả tối thiểu:

```json
{
  "content": "...",
  "range": { "startLine": 1000, "endLine": 1199 },
  "nextCursor": "...",
  "truncated": true,
  "truncationReason": "outputLimit",
  "bytesRead": 123456,
  "sizeBytes": 987654321,
  "versionToken": "...",
  "encoding": "utf-8",
  "bom": false,
  "lineEnding": "lf"
}
```

Không hứa `totalLines` miễn phí cho file lớn. Nếu muốn biết chính xác tổng dòng phải:

- tính trong lần scan toàn file có budget; hoặc
- trả `totalLines: null` cùng `totalLinesKnown: false`; hoặc
- dùng cached line index theo version token.

## Thiết kế implementation

### Đọc theo dòng

- Dùng `tokio::fs::File` + `BufReader`/`AsyncBufReadExt` hoặc blocking `BufRead` trong `spawn_blocking` có cooperative cancellation.
- Scan đến `startLine`, nhưng không lưu các dòng trước đó.
- Dừng ngay khi đủ `lineCount`, `maxBytes`, time budget hoặc cancellation.
- Với start line rất sâu, có thể dùng line-offset index cache theo `(canonicalPath, versionToken)`; cache là tối ưu tùy chọn, không được làm correctness phụ thuộc cache.

### Đọc theo byte

- Seek trực tiếp tới offset.
- Nếu output yêu cầu UTF-8, điều chỉnh biên về code point hợp lệ và báo số byte thực tế.
- Cho phép chế độ raw/content reference khi file không phải text; không lossy-decode im lặng.

### Encoding/BOM/newline

- Nhận diện UTF-8 BOM và không trả BOM trong content mặc định, nhưng metadata phải báo `bom=true`.
- Invalid UTF-8 trả lỗi có offset gần vị trí lỗi hoặc `encodingUnsupported`; không đọc lại toàn file chỉ để tìm lỗi.
- Phân biệt `lf`, `crlf`, `cr`, `mixed`, `none/unknown` dựa trên sample hoặc range; nếu chỉ sample thì metadata phải nói detection không toàn file.
- Không normalize newline trong read result trừ khi caller yêu cầu rõ.

### Version consistency

- Stat file trước và sau range read.
- Nếu identity/size/mtime/version thay đổi trong lúc đọc, trả `fileChangedDuringRead` hoặc partial result với warning theo contract đã chọn.
- `versionToken` dùng chung với plan 08.

## Các bước triển khai

1. Tách `read_text_range` khỏi `filesystem.rs` sang module riêng nếu file sẽ vượt 500 dòng.
2. Tạo typed request/result cho v2; giữ adapter tool cũ.
3. Implement streaming line reader với output buffer bounded.
4. Implement byte-range reader và cursor continuation.
5. Tích hợp cancellation + budget từ `OperationContext`/plan 16.
6. Tích hợp `versionToken` từ plan 08; nếu chưa có, tạo abstraction để plan 08 thay thế mà không đổi API lần nữa.
7. Chuyển snapshot helper sang bounded-prefix/sampled reader; không gọi API đọc toàn file.
8. Cập nhật schema MCP, catalog capability và docs.
9. Kiểm tra UI hiển thị range/truncation mà không hiểu `endLine` là cuối file khi chỉ là cuối range.

## Test bắt buộc

- File UTF-8 10 MB, 100 MB và fixture sparse/streamed tương đương 1 GB; đọc 20 dòng đầu/giữa/cuối.
- Peak allocation/RSS không tăng tuyến tính theo kích thước file cho range nhỏ.
- Line cực dài lớn hơn `maxBytes` dừng có lý do đúng, không OOM.
- UTF-8 multi-byte nằm sát byte boundary không bị cắt sai.
- UTF-8 BOM, CRLF, LF, CR, mixed newline.
- Invalid UTF-8 trước range, trong range và sau range; behavior phải đúng theo contract.
- File thay đổi giữa read: append, truncate, replace atomically.
- Cancellation và timeout khi đang scan đến line sâu.
- Symlink bị xử lý theo workspace policy hiện tại; không follow ngoài scope.
- Cursor dùng lại sau khi file đổi phải bị từ chối hoặc cảnh báo version mismatch.
- Adapter tool cũ vẫn trả shape cũ và đúng `truncated` trong giai đoạn compatibility.

## Benchmark bắt buộc

Tạo benchmark hoặc integration perf test đo:

- wall time;
- bytes thực đọc;
- peak RSS/allocation gần đúng;
- output bytes;
- file 10 MB/100 MB/1 GB, range đầu và range sâu.

Ghi baseline implementation cũ nếu có thể và chứng minh cải thiện. Không đặt threshold quá phụ thuộc máy CI; tập trung invariant `bytesBuffered`/allocation bounded và timeout hữu hạn.

## Tiêu chí nghiệm thu

- Không còn `tokio::fs::read`/`read_to_string` toàn file trong đường chạy range chính.
- Đọc range nhỏ không collect toàn bộ lines.
- Có continuation metadata và truncation reason.
- Cancellation làm vòng đọc dừng nhanh và đóng file.
- Encoding/BOM/newline behavior được test.
- File change trong lúc đọc không tạo result giả vờ nhất quán.
- Docs không còn mô tả range như tối ưu nếu implementation không thật sự bounded.

## Validation tối thiểu

```bash
cargo fmt --check
cargo check --workspace
cargo test -p chatcmd-runtime
cargo test -p chatcmd-mcp
cargo test --workspace
```

Chạy thêm benchmark/perf fixture của plan và báo số liệu.

## Kết quả AI phải trả về

- File đã đổi và module mới.
- Contract request/result cuối.
- Cách đảm bảo RAM/I/O bounded.
- Encoding/newline/version behavior.
- Test/benchmark và số liệu.
- Tương thích với `fs_read_text` cũ.
