use super::is_explicit_multi_agent_request as requested;

#[test]
fn subagent_intent_accepts_wrapped_and_multiline_requests() {
    for text in [
        "Sử dụng plugin @rust_test\n\nThư mục dự án: D:\\DEV\\CmdGPT\\ChatCmdClient\n\nđể thực hiện yêu cầu sau: Chia agent rà soát source",
        "Sử dụng plugin @rust_test để thực hiện yêu cầu sau:\nChia ra 3 agent để audit song song",
        "Kiểm tra source.\n- Chia agent đọc backend\n- Tổng hợp kết quả",
        "Yêu cầu:\n1. Chia agent đọc file\n2. Tổng hợp kết quả",
        "Rà soát source; chia agent kiểm tra từng phần",
        "Kiểm tra source, dùng nhiều agent để review",
        "Rà soát source và chia agent đọc các module",
        "## Yêu cầu\n**Chia agent** để kiểm tra source",
        "Chia agent.\n[[CHATCMD-REQUEST:example]]\nChatCMD routing: use turnId=example; do not create another conversation.",
    ] {
        assert!(requested(text), "explicit request rejected: {text}");
    }
}

#[test]
fn subagent_intent_accepts_common_vietnamese_and_english_commands() {
    for text in [
        "Chia 2 agent đọc code giúp tôi",
        "Chia thành 3 agent để xử lý",
        "Bạn chia ra 2 agent giúp t nhé",
        "Giờ chia agent để kiểm tra",
        "Tôi muốn chia agent review repo",
        "Hãy chia công việc cho 3 agent",
        "Tách ra 2 agent để audit",
        "Dùng sub agent để kiểm tra source",
        "Sử dụng sub-agent để review",
        "Tạo agent con đọc backend",
        "Hay chia ra 2 agent de ra soat",
        "CHIA   AGENT\tđể kiểm tra",
        "Please use multiple agents to review this repo",
        "Can you split this work across 2 agents?",
        "Could you create a sub-agent to read the backend?",
        "Use subagents to review this change",
        "Spawn two agents to review the code",
        "Delegate this review to subagents",
        "Split into agents to review the repo",
    ] {
        assert!(requested(text), "explicit request rejected: {text}");
    }
}

#[test]
fn subagent_intent_rejects_negation_without_blocking_unrelated_constraints() {
    for text in [
        "Không chia agent, làm trong cuộc trò chuyện hiện tại",
        "Dung sub agent de review. Khong can chia agent",
        "Đừng chia ra 3 agent",
        "Đu\u{031b}\u{0300}ng chia agent",
        "Đư\u{0300}ng dùng nhiều agent",
        "Không dùng sub-agent",
        "Không cần agent con",
        "Chưa cần tạo subagent",
        "Do not use multiple agents",
        "Don't use multiple agents; use multiple agents is just a label",
        "Don’t spawn sub-agents",
        "Never delegate to agents",
        "Without subagents, review this code",
        "No subagents. Use multiple agents to review",
    ] {
        assert!(!requested(text), "negation granted delegation: {text}");
    }
    assert!(requested("Không sửa file, chia agent chỉ đọc code"));
    assert!(requested("Chia agent để review; không commit hoặc push"));
}

#[test]
fn subagent_intent_does_not_treat_mentions_or_quoted_data_as_commands() {
    for text in [
        "Rà soát toàn bộ source code, sub agent, để tìm lỗi",
        "Kiểm tra logic chia agent hiện tại",
        "Khi t yêu cầu chia agent thì đang bị lỗi, sửa giúp t",
        "Chia agent bị lỗi, hãy kiểm tra nguyên nhân",
        "Chia agent là gì?",
        "Review nhãn: chia agent",
        "Review chuỗi `chia agent` trong source",
        "Review nhãn \"thử chia agent\" trong UI",
        "Review nhãn “hãy chia agent” trong UI",
        "Review nhãn ‘chia agent’ trong UI",
        "Ví dụ:\n```text\nChia agent\n```\nSửa code",
        "Ví dụ:\n~~~text\nChia agent\n~~~\nSửa code",
        "Review log:\n> Chia agent để sửa code",
        "Review log:\n    Chia agent để sửa code",
        "Review this file\n[[CHATCMD-REQUEST:example]]\nChia agent",
        "Read code.\nChatCMD routing: please use multiple agents",
        "Chia agentic components thành module",
        "Tách ra các module, không phải agent",
        "Use the agent settings page",
        "Sử dụng agent @rust_test để kiểm tra source",
        "Use agent @rust_test to inspect this project",
        "Tạo agent mới trong trang cài đặt",
        "Create multiple agent profiles in the settings UI",
        "Chia `hidden text`agent trong source",
    ] {
        assert!(!requested(text), "data/mention granted delegation: {text}");
    }
}
