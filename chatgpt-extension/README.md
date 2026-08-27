# ChatCMD ChatGPT Bridge

Extension Manifest V3 cho Chrome/Edge, dùng tab `chatgpt.com` mà người dùng đã đăng nhập để gửi/tiếp tục/dừng tin nhắn từ ChatCMD local web.

## Cài development

1. Mở `chrome://extensions` hoặc `edge://extensions`.
2. Bật **Developer mode**.
3. Chọn **Load unpacked**.
4. Chọn folder `chatgpt-extension` này.
5. Đăng nhập `https://chatgpt.com` trong cùng profile trình duyệt.
6. Reload trang ChatCMD local web rồi vào **Tasks → Tin nhắn mới**.

## Quyền và bảo mật

- Extension **không** có quyền `cookies` và không đọc/ghi token đăng nhập.
- Nó thao tác DOM của trang ChatGPT đang đăng nhập, tương tự thao tác của người dùng.
- Nếu chưa có tab ChatGPT, bridge tạo tab nền không giành focus và tự đóng tab đó sau khi request hoàn tất; tab ChatGPT do người dùng mở sẵn sẽ chỉ được tái sử dụng và không tự đóng.
- Callback chỉ được gửi tới HTTP origin `localhost` hoặc `127.0.0.1` và luôn kèm `X-ChatCmdClient: chatgpt-extension`.
- Selector ChatGPT được cô lập trong `content-chatgpt.js`; nếu ChatGPT đổi UI thì sửa adapter này.

## Model

`Auto` giữ nguyên model hiện tại của ChatGPT. Với model khác, extension mở model switcher và chọn theo text hiển thị. Tên model trên ChatGPT có thể thay đổi theo tài khoản/plan, vì vậy ô model trong ChatCMD cho phép nhập label tùy ý.

## Lưu ý

Đây là browser UI bridge, không phải OpenAI API chính thức. Việc gửi và đọc phản hồi phụ thuộc vào DOM hiện tại của `chatgpt.com`; nên kiểm thử lại extension sau các thay đổi lớn của giao diện ChatGPT.
