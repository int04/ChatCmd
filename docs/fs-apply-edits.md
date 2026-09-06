# `fs_apply_edits`

`fs_apply_edits` là primitive chính để sửa file text lớn. Caller phải lấy `versionToken` từ `fs_stat` hoặc `fs_read_text_v2` và gửi lại dưới `expectedVersion`.

## Tọa độ

- `coordinateSystem: "byte"`: mỗi edit dùng `startByte` và `endByte`. Offset là 0-based, end-exclusive và bắt buộc nằm ở biên UTF-8.
- `coordinateSystem: "lineColumn"`: mỗi edit dùng `start`/`end`; line và column đều 1-based, column đếm Unicode scalar (`columnEncoding: "utf8CodePoint"`), end-exclusive. CRLF được xem là line ending; vị trí cuối nội dung dòng nằm trước `\r`.
- Edit được sort theo start. Adjacent edit hợp lệ; overlap bị từ chối. Retry với version cũ sau khi commit trả version mismatch, vì vậy không áp dụng edit hai lần.
- Hai edit có cùng range khác rỗng là overlap và bị từ chối. Nhiều insertion rỗng tại cùng offset được áp dụng ổn định theo thứ tự request.

## Transaction và giới hạn

Engine quét source theo chunk 64 KiB để validate UTF-8/tọa độ, sau đó stream source vào temp file cùng thư mục. Temp được flush + sync, nhận permissions của source, target identity/version được kiểm tra lại ngay trước persist. Rename cùng filesystem là commit point; lỗi/cancel trước đó không đổi target và temp tự cleanup. Parent directory được sync trên Unix. Windows dùng replace semantics của `tempfile::persist`; durability của directory metadata phụ thuộc OS.

`dryRun` thực hiện cùng validation nhưng không tạo temp, không đổi content hay mtime. Preview tối đa 8 KiB và không chứa full before/after. Budget giới hạn timeout, tổng bytes đọc/ghi và số edit. BOM được giữ nguyên vì source bytes không thuộc range được copy nguyên trạng; replacement luôn phải là UTF-8.

`fs_replace_text` vẫn tương thích cho caller cũ nhưng giới hạn file 8 MiB. File lớn phải dùng `fs_apply_edits`.
