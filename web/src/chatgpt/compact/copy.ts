import { appLocale } from '../../i18n';

const copy = {
  checking: ['Checking saved compact progress…', 'Đang kiểm tra tiến trình thu gọn đã lưu…'],
  loadError: ['Could not read compact progress. Retry before sending.', 'Không đọc được tiến trình thu gọn. Hãy thử lại trước khi gửi.'],
  waiting: ['Progress is saved. You can reload this page without losing this job.', 'Tiến trình đã được lưu. Bạn có thể tải lại trang mà không mất lần thu gọn này.'],
  extensionMissing: ['Progress is saved, but the extension has not acknowledged it. Enable or update ChatCMD ChatGPT Bridge, reload this page, then choose Resume via extension.', 'Tiến trình đã lưu nhưng extension chưa xác nhận. Bật hoặc cập nhật ChatCMD ChatGPT Bridge, tải lại trang rồi chọn Tiếp tục qua extension.'],
  waking: ['Contacting the extension…', 'Đang kết nối extension…'],
  acknowledged: ['Waiting for ChatGPT. The extension will continue from the saved checkpoint.', 'Đang chờ ChatGPT. Extension sẽ tiếp tục từ bước đã lưu.'],
  resume: ['Resume via extension', 'Tiếp tục qua extension'],
  cancelJob: ['Cancel compact', 'Hủy thu gọn'],
  cancelled: ['Cancelled', 'Đã hủy'],
  completed: ['Completed', 'Hoàn tất'],
  completedDetail: ['The new chat is ready. This task, its history and queued messages are unchanged.', 'Cuộc trò chuyện mới đã sẵn sàng. Task, lịch sử và hàng đợi tin nhắn vẫn được giữ nguyên.'],
  cancelledDetail: ['Compact was cancelled. Your draft and queued messages are preserved.', 'Đã hủy thu gọn. Bản nháp và hàng đợi tin nhắn được giữ nguyên.'],
  preserved: ['Sending is paused. Your draft and queued messages are preserved.', 'Tạm dừng gửi tin nhắn. Bản nháp và hàng đợi của bạn vẫn được giữ nguyên.'],
  conflict: ['Progress changed before cancellation. Review the latest state and try again.', 'Tiến trình đã thay đổi trước khi hủy. Hãy kiểm tra trạng thái mới nhất rồi thử lại.'],
  empty: ['No compact history yet.', 'Chưa có lần thu gọn ngữ cảnh nào.'],
  reference: ['Open old conversation', 'Mở cuộc trò chuyện cũ'],
  newTab: ['(reference only, opens in a new tab)', '(chỉ để tham khảo, mở trong tab mới)'],
  oldId: ['Old conversation', 'Cuộc trò chuyện cũ'],
  newId: ['New conversation', 'Cuộc trò chuyện mới'],
  details: ['Conversation details', 'Chi tiết cuộc trò chuyện'],
  noUrl: ['Old conversation link unavailable.', 'Chưa có liên kết cuộc trò chuyện cũ.'],
  confirm: ['Confirm compact', 'Xác nhận thu gọn'],
  continueAfterCompact: ['Continue working after compact completes', 'Tiếp tục công việc sau khi compact xong'],
  continueHint: ['Off by default. Only when checked will ChatCMD send the continuation message after attaching the new chat.', 'Mặc định tắt. Chỉ khi tích, ChatCMD mới gửi tin nhắn tiếp tục công việc sau khi gắn chat mới.'],
  preparing: ['Preparing the existing conversation and its context.', 'Đang chuẩn bị cuộc trò chuyện và ngữ cảnh hiện tại.'],
  writing_handoff: ['ChatGPT is writing the handoff. Keep the ChatGPT tab available.', 'ChatGPT đang viết bản bàn giao. Hãy giữ tab ChatGPT trong trình duyệt.'],
  saving_handoff: ['Saving the handoff before opening a new chat.', 'Đang lưu bản bàn giao trước khi mở cuộc trò chuyện mới.'],
  opening_new_chat: ['Opening the new chat and reconnecting it to this same task.', 'Đang mở cuộc trò chuyện mới và kết nối lại với chính task này.'],
  bridgeSync: ['Waiting for the new conversation link before resuming messages…', 'Đang đồng bộ liên kết cuộc trò chuyện mới trước khi tiếp tục gửi…'],
} as const;

export function compactText(key: keyof typeof copy): string {
  return copy[key][appLocale().toLowerCase().startsWith('vi') ? 1 : 0];
}
