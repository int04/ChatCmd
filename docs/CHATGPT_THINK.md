# ChatGPT Think / ChatCMD Think — capture v2

## Hành vi

Cả tin nhắn gõ trực tiếp trên trang ChatGPT (không gọi plugin/MCP) và tin nhắn gửi từ ChatCMD đều đi vào bộ ghi nhận công khai của trình duyệt. Mỗi lượt có hai nguồn trong bubble: ChatGPT Think là nội dung trang đã render; ChatCMD Think là progress/tool activity qua MCP. Khi chưa có nội dung MCP, UI hiển thị nguồn ChatGPT. Khi MCP có nội dung, UI ưu tiên ChatCMD nhưng vẫn giữ bản ghi trình duyệt. Final MCP không bị bản ghi trình duyệt ghi đè.

Conversation đã liên kết được ghi tiếp vào task hiện có. Conversation chưa liên kết tạo task lưu trữ riêng và một agent recorder bị tắt, không có quyền tool; việc thu thập không cấp quyền thực thi. Lượt mới được nhận diện bằng conversation ID và user message ID, không bằng nội dung câu hỏi. Gửi hai câu giống nhau vẫn là hai lượt riêng.

Bộ quan sát thu lượt cuối đang hiển thị khi mở trang và theo dõi những lượt tiếp theo; không quét toàn bộ lịch sử. Chỉ đọc nội dung DOM công khai, không truy cập reasoning ẩn. Nội dung đã thu vẫn được lưu trong SQLite khi ChatGPT thu gọn giao diện. Nội dung chưa từng được thu không thể tái tạo ngược.

## Lỗi đã sửa ở bản 0.1.4

- Bản 0.1.3 chỉ tạo observer từ lệnh gửi của ChatCMD, chưa có đường đăng ký lượt gửi trực tiếp trên ChatGPT.
- Nhánh missing-receiver trong background-io.js còn inject danh sách script cũ, thiếu runtime/transcript/observer/monitor. Tất cả đường inject hiện dùng danh sách trong manifest.
- Bộ đọc user bubble dùng textContent làm dính các đoạn văn và lấy cả nút Show more. Prompt không khớp nên capture không bắt đầu. Bộ đọc mới giữ ranh giới đoạn và loại control, hỗ trợ cả data-message-author-role và data-turn.
- Runner được bọc IIFE và kiểm tra context để inject lại không đụng khai báo biến hoặc để handler cũ xử lý request.
- Native follow-up không bị recorder của lượt trước đánh dấu nhầm là đã thu. UI chuyển sang running ngay khi nhận user turn mới, trước khi có MCP.
- Phát hiện final theo request/turn identity, không dùng text/time chung cho các câu hỏi lặp.

## Tab nền — sửa ở extension 0.1.5

Bản 0.1.4 vẫn đặt native enrollment, debounce snapshot và vòng chờ completion sau timer của trang. Khi timer tab nền bị trì hoãn, DOM có thể đã thay đổi nhưng recorder không đọc/gửi lại. Một mutation cuối rơi trong cửa sổ giới hạn gửi cũng có thể thiếu lịch gửi tiếp.

- `content-chatgpt-clock.js` quản lý deadline của riêng bộ thu. Local timer tiếp tục qua MessageChannel để không nối dài chuỗi timer. Khi document ở nền, deadline còn được đánh thức bằng phản hồi từ `background-clock.js`; không chờ timer của tab.
- Service worker chỉ nhận wake từ content script ChatGPT ở frame chính, tối đa 4 yêu cầu chờ/tab, mỗi lần không quá 1 giây. Không nhận nội dung hội thoại, đường dẫn hay quyền thực thi qua đường clock. Idle polling không giữ worker chạy liên tục.
- Mutation được gom thành microtask. Snapshot chưa ACK hoặc mutation cuối trong cửa sổ giới hạn gửi luôn được xếp lịch lại. Monitor dùng cùng clock nên completion/stop không phải đợi chuyển tab.
- Hủy deadline/observer khi rời document, bỏ callback của context cũ và kiểm tra `clockProtocol=1` khi khôi phục content script.

Bản 0.1.5 không thay API render của trang; nó chỉ vận chuyển nội dung DOM công khai đã có. Vì thế vẫn bỏ sót trường hợp lịch render của trang dừng trước khi DOM thay đổi. Tab bị trình duyệt đóng băng/discard hoàn toàn, máy ngủ hoặc ChatGPT không render thêm DOM là trường hợp khác với timer bị throttle; bộ thu không thể đảm bảo realtime trong các trạng thái đó.

Test `content-chatgpt-background.test.cjs` khóa toàn bộ timer tab và giữ visibility=hidden: kiểm tra streaming, trailing update, retry, callback đến muộn, đổi conversation, cancel/reinject và giới hạn worker. Test HTTP trong `capture-integration.cjs` cũng giữ cả hai tab ở hidden và không chạy timer nào của tab; vẫn yêu cầu snapshot trước final và hoàn tất không MCP. Đây là kiểm thử DOM/Chrome transport mô phỏng với HTTP Rust/SQLite thật, không phải kiểm thử phiên Chrome đăng nhập thực tế.

Tham chiếu kỹ thuật: Chrome Developers, “Heavy throttling of chained JS timers beginning in Chrome 88” và “The extension service worker lifecycle”; đối chiếu thêm helper `later`/`sleep` và `watchTranscript` trong repo chat-on-steroids.

## Render khi tab không active — extension 0.1.6

Bản 0.1.5 chỉ xử lý timer của content script. Chrome ngừng requestAnimationFrame của trang khi tab ở nền; clock vẫn chạy nhưng DOM có thể chưa nhận nội dung mới. Đây là một lớp khác, đã được tái hiện bằng Chrome thật với trang kiểm thử dùng requestAnimationFrame đã cache. Không khẳng định đã kiểm tra implementation nội bộ của phiên ChatGPT đang đăng nhập.

- page-chatgpt-render.js được manifest nạp ở MAIN world, document_start, trước khi trang cache API. Wrapper giữ ID native, timestamp của frame visible và cancellation; khi hidden, pulse có lease của lượt đang thu cho phép callback frame chờ chạy theo batch giới hạn. Callback reentrant chờ pulse kế tiếp; một exception không chặn cả batch.
- content-chatgpt-render.js nối clock isolated với helper MAIN bằng metadata version/path/active, không truyền user text, task ID, credentials, code hoặc quyền thực thi. User gesture gửi tin có lease khởi động ngắn để không mắc vòng chờ user bubble chưa render.
- Clock cũ của service worker tiếp tục đánh thức bộ thu; pulse render chạy trước khi scan DOM. Không thay document.hidden, visibilityState, focus; không đọc Fiber/model state hoặc chặn/sao chép network stream.
- Mọi đường inject/recover đều nạp MAIN và ISOLATED đúng world. Health yêu cầu renderProtocol=1. MAIN helper giữ nguyên khi reinject để không làm mất callback đang chờ. Tab đã load từ trước cần reload một lần vì helper không thể thu hồi tham chiếu native mà trang đã cache trước đó.

### Bằng chứng và giới hạn

render-browser-smoke.cjs mở Chrome bằng profile tạm riêng và pipe CDP, không dùng profile/cookie đang đăng nhập và không bật cờ vô hiệu hóa background throttling. Nó nạp đúng extension unpacked, giữ tab thật hidden=true và hasFocus=false, không sửa hai thuộc tính này. Trang/API local trong phép thử này là fixture; transport extension/content script/service worker và Chrome scheduling là thật.

Negative control tắt riêng render bridge: timer capture vẫn chạy, callback frame và snapshot không tiến triển khi hidden; chuyển focus mới tiến triển. Positive control cho cả native và ChatCMD dispatch: callback đã cache chạy, DOM/snapshot cập nhật và final hoàn tất khi tab vẫn hidden. Test Rust/SQLite riêng tiếp tục kiểm tra API, persistence và lịch sử mã hóa.

Một tab bị Chrome freeze/discard hoàn toàn hoặc máy ngủ vẫn nằm ngoài bảo đảm realtime. Nếu phiên bản ChatGPT khác chặn xử lý bằng cơ chế khác ngoài animation frame, cần chẩn đoán riêng; không giả mạo trạng thái hiển thị để bỏ qua nó. Phép thử Chrome là fixture UI, không phải xác nhận trực tiếp trên phiên ChatGPT của người dùng.

Tham chiếu: Chrome Developers, Background tabs in Chrome 57 (https://developer.chrome.com/blog/background_tabs); Manifest content scripts / execution world (https://developer.chrome.com/docs/extensions/reference/manifest/content-scripts).

## Kiến trúc

```text
ChatGPT DOM
  -> content-chatgpt-native.js: nhận diện user turn, không nhấn Send và không gọi MCP
  -> background-capture.js -> POST /api/local/chatgpt/capture/turns
  -> request/tab binding
  -> content-chatgpt-transcript.js + content-chatgpt-observer.js
  -> background-io.js -> POST /api/local/chatgpt/bridge/{request_id}/observation
  -> SQLite timeline_events, snapshot revisioned theo request
  -> WebSocket local / API lịch sử
  -> ChatGPT Think / ChatCMD Think trong cùng bubble
```

Lượt gửi từ ChatCMD bỏ qua bước native enrollment và dùng request đã tạo. content-chatgpt-monitor.js là monitor dùng chung. content-chatgpt-resume.js nhận lại request có checkpoint đúng user/conversation, không gửi lại prompt. Snapshot mới thay thế cùng event ID, không tạo event cho từng token. src/chatgpt_transcript.rs giữ liên kết với turn MCP.

GET /api/local/chatgpt/capture/capabilities trả captureProtocol=2. Service worker kiểm tra tab thật và tự chọn local API đã cấu hình; trang ChatGPT không được truyền task/agent/quyền thực thi/localBaseUrl. Endpoint native chỉ nhận identity và nội dung user. Allowlist extension mở đúng các route capture, không nới route quản trị hoặc mã hóa API GUI.

Giới hạn snapshot: 128 phần, tổng 100.000 ký tự Unicode. Một request đang gửi tại một thời điểm; revision chống replay/out-of-order; ACK chỉ sau khi ghi SQLite. Trạng thái completed không bị snapshot streaming đến muộn mở lại. Đổi conversation hoặc user turn ngừng recorder cũ.

## Quan sát lỗi

Extension version: **0.1.6**. Capture protocol: **2**. Background clock protocol: **1**. Render protocol: **1**. Document root có data-chatcmd-render-protocol và data-chatcmd-render-frames để quan sát helper đã nạp và số callback nền. Khi có lỗi API/phiên bản hoặc snapshot không được ACK, xem Extension logs, source capture. Document root có data-chatcmd-capture-state và data-chatcmd-capture-error. Không coi extension có receiver đơn thuần là đã có đủ module capture.

## Nghiên cứu repo tham chiếu

Đã tham khảo D:\DEV\chat-on-steroids: docs/chatgpt-turn-signals.md, extension/chatgpt-dom.js, extension/content.js và src/main/session/recorder.ts. Áp dụng các ý: tách quan sát khỏi lifecycle, ràng buộc conversation/user turn, upsert bản ghi ổn định, giữ nội dung đã thu và ghi bền trước ACK. Không sao chép lớp Electron/Fiber/private model state. Completion hiện vẫn là heuristic DOM ổn định, nút Stop/composer và trạng thái request, không phải tín hiệu model end_turn.

## Kiểm tra

```powershell
# Tại repo root. Cần Node và npm ci trong web để có jsdom.
node --test chatgpt-extension/content-chatgpt.test.cjs chatgpt-extension/content-chatgpt-observer.test.cjs chatgpt-extension/content-chatgpt-native.test.cjs chatgpt-extension/content-chatgpt-background.test.cjs chatgpt-extension/page-chatgpt-render.test.cjs
# Chrome thật, profile tạm; CHATCMD_TEST_CHROME có thể chỉ đường dẫn Chrome khác.
node chatgpt-extension/render-browser-smoke.cjs
cargo test -p chat-cmd-client full_stack_capture_from_native_and_chatcmd_without_mcp --target-dir target/chatgpt-think-verification -- --nocapture
cargo test --workspace --target-dir target/chatgpt-think-verification
cargo check --workspace --all-targets --target-dir target/chatgpt-think-verification
cargo clippy --workspace --all-targets --target-dir target/chatgpt-think-verification -- -D warnings
cargo fmt --all -- --check

cd web
npm test -- --run
npm run lint
npm run build
```

Test full-stack nạp các script content/background được phát hành theo manifest, kiểm tra missing-receiver và reinjection, đi qua HTTP router Rust, SQLite và API lịch sử GUI mã hóa. Trang ChatGPT và chrome.* là fixture JSDOM/mock, không phải phiên Chrome đăng nhập thực tế. Cả hai đường được yêu cầu phải có snapshot trước final và có 0 hoạt động MCP. Tests khác bao phủ trùng câu hỏi, callback muộn, giới hạn, permissions và giữ dữ liệu.

Cache target mặc định từng báo thiếu API runtime vẫn có trên source. Dùng target/chatgpt-think-verification để tránh metadata cũ, không cần xóa dữ liệu ứng dụng.

## Kích hoạt

Với backend capture v2 đã chạy từ đợt 0.1.4, bản sửa tab nền chỉ cần reload extension unpacked chatgpt-extension lên **0.1.6**, rồi bắt buộc tải lại các tab ChatGPT một lần để helper MAIN được chạy ở document_start. Không cần build/restart Rust riêng cho bản sửa này. Khi nâng từ bản cũ hơn capture v2, vẫn cần backend có native enrollment/capabilities v2. Không tự restart ứng dụng đang phục vụ MCP trong lúc sửa.

Sau khi tải lại, gửi câu hỏi rồi chuyển ngay sang ChatCMD/tab khác và giữ ChatGPT ở nền đến khi hoàn tất. Kiểm tra ChatGPT Think cập nhật mà không cần quay lại tab nguồn, sau đó refresh task để kiểm tra dữ liệu đã lưu.

Smoke test trên Chrome: gửi trực tiếp một câu không gọi plugin; gửi câu nhiều đoạn từ ChatCMD; kiểm tra nguồn ChatGPT xuất hiện trước MCP; hoàn tất rồi refresh task để đọc lịch sử; gửi câu giống nhau hai lần; chuyển chat giữa lúc trả lời và kiểm tra không trộn nội dung.

## Nhiều hội thoại đồng thời

Kiểm tra bổ sung trên extension 0.1.6: hai và ba conversation riêng, tất cả tab nguồn ở nền, có cả native và ChatCMD-dispatched turn. Dùng cùng prompt và cùng user/assistant message ID giữa các tab nhưng nội dung đánh dấu riêng để bắt lỗi trộn. Một snapshot được cố tình giữ rồi trả 503; những chat khác vẫn phải stream. Một kịch bản đóng một tab giữa lượt xác nhận hai tab còn lại vẫn hoàn tất. Sau khi hoàn tất, không gửi snapshot mới hoặc tiếp tục đánh thức service worker trong cửa sổ idle đo được.

Mỗi document có recorder/clock riêng; worker định tuyến theo tabId và requestId, backend suy ra task/turn từ request. Chỉ một snapshot đang chờ ACK trong mỗi recorder; cập nhật trong lúc chờ được gom vào bản mới nhất. UI chi tiết chỉ hợp nhất event của task đang xem; dữ liệu task khác vẫn được lưu ở backend và tải khi chọn task đó. Muốn thu song song ba conversation, giữ ba tab ChatGPT nguồn riêng: chuyển sang conversation khác trong cùng một tab không giữ DOM của conversation trước để tiếp tục thu. Hai tab mở cùng một conversation không thuộc kịch bản được xác nhận ở đây.

### Sửa tranh chấp SQLite

Test ba native enrollment đồng thời đã phát hiện HTTP 500 với SQLite code 5, database is locked. Ba transaction của enroll, persist_observation và persist_browser_completion đọc trước khi ghi bằng BEGIN deferred. Đã đổi riêng ba transaction ghi này sang BEGIN IMMEDIATE, sử dụng busy_timeout có sẵn để chờ quyền ghi trước khi lấy snapshot đọc. Quyền ghi chỉ được giữ trong transaction SQL ngắn; không bao gồm chờ network, browser hoặc MCP. Các transaction chỉ đọc, schema, quyền thực thi, và extension production không đổi. Tham chiếu: SQLite Isolation (https://sqlite.org/isolation.html), SQLx 0.8.6 Pool::begin_with.

Kết quả kiểm tra: 35 test liên quan ChatGPT đạt; test concurrency mới chạy lặp 20/20 lần đạt sau sửa. 54 test extension đạt. Smoke Chrome thật chạy ba kịch bản: 2 chat, 3 chat, 3 chat rồi đóng 1. Chrome/extension/HTTP transport là thật; trang rAF và API trong smoke là fixture. SQLite/router và API lịch sử GUI mã hóa được kiểm tra riêng bằng Rust. Không kiểm thử phiên ChatGPT đăng nhập, không đo CPU/RAM dài hạn hoặc suy rộng cho hàng chục tab.

```powershell
node chatgpt-extension/multi-chat-browser-smoke.cjs
cargo test -p chat-cmd-client three_concurrent_native_conversations_stay_isolated_in_sqlite --target-dir target/chatgpt-think-verification -- --nocapture
```

**Kích hoạt phần sửa concurrency:** giữ extension 0.1.6; build/chạy backend Rust mới. Reload extension đơn thuần không áp dụng được thay đổi SQLite này. Không tự restart ứng dụng đang phục vụ MCP và không tự commit.
