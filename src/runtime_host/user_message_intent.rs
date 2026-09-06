use serde_json::{Value, json};

pub(super) fn is_plan_mode_request(content: &str) -> bool {
    let normalized = intent_prose(content)
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let negated = [
        "không cần lên kế hoạch",
        "không cần lập kế hoạch",
        "không lên kế hoạch",
        "không lập kế hoạch",
        "đừng lên kế hoạch",
        "đừng lập kế hoạch",
        "do not plan",
        "don't plan",
        "no plan needed",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase));
    !negated
        && (normalized.contains("lên kế hoạch")
            || normalized.contains("lập kế hoạch")
            || normalized.split_whitespace().any(|word| word == "#plan"))
}

pub(super) fn intent_hint(content: &str) -> Value {
    let normalized = intent_prose(content).to_lowercase();
    let workflow_kind = if is_plan_mode_request(content) {
        "plan"
    } else if [
        "chỉ review",
        "chỉ đánh giá",
        "review only",
        "do not edit",
        "đừng sửa",
    ]
    .iter()
    .any(|phrase| normalized.contains(phrase))
    {
        "review"
    } else if ["debug", "sửa lỗi", "fix bug"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        "debug"
    } else if ["commit", "tạo commit"]
        .iter()
        .any(|phrase| normalized.contains(phrase))
    {
        "commit"
    } else {
        "implement"
    };
    json!({
        "workflowKind": workflow_kind,
        "authoritative": false,
        "grantsExecutionPermission": false,
        "note": "Language classification is a workflow hint only; effective effects come from task policy and authenticated user decisions."
    })
}

fn intent_prose(content: &str) -> String {
    let mut prose = String::with_capacity(content.len());
    let mut fenced = false;
    for line in content.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let mut delimiter = None;
        for character in line.chars() {
            if matches!(character, '`' | '"' | '\'') {
                delimiter = match delimiter {
                    Some(active) if active == character => None,
                    None => Some(character),
                    active => active,
                };
            } else if delimiter.is_none() {
                prose.push(character);
            }
        }
        prose.push(' ');
    }
    prose
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_trigger_ignores_negation_quotes_and_code() {
        assert!(is_plan_mode_request("Lên kế hoạch cho tôi mua quà"));
        assert!(is_plan_mode_request("LẬP   KẾ HOẠCH\nwebsite bán hàng"));
        assert!(is_plan_mode_request("Xây website giúp tôi #PLAN"));
        assert!(!is_plan_mode_request("Cho tôi xem kế hoạch hiện tại"));
        assert!(!is_plan_mode_request("Dùng planner để theo dõi công việc"));
        assert!(!is_plan_mode_request("Không cần lên kế hoạch, sửa luôn"));
        assert!(!is_plan_mode_request(
            "Đừng lập kế hoạch; implement trực tiếp"
        ));
        assert!(!is_plan_mode_request("Log ghi `#plan` nhưng hãy sửa lỗi"));
        assert!(!is_plan_mode_request(
            "Ví dụ:\n```text\nlập kế hoạch\n```\nSửa code"
        ));
        assert!(!is_plan_mode_request("Review chuỗi \"lên kế hoạch\""));
    }

    #[test]
    fn intent_hint_never_grants_execution_permission() {
        let review = intent_hint("Chỉ review, đừng sửa");
        assert_eq!(review["workflowKind"], "review");
        assert_eq!(review["authoritative"], false);
        assert_eq!(review["grantsExecutionPermission"], false);
    }
}
