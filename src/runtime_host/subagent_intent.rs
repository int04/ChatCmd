//! Conservative, clause-scoped delegation recognition. This is not an execution grant.
#[path = "subagent_intent_prose.rs"]
mod prose;
use prose::{command_prose, normalize};

pub(super) fn is_explicit_request(content: &str) -> bool {
    let mut requested = false;
    for sentence in command_prose(content).split(['\n', ';', '.', '!', '?', ',']) {
        let normalized = normalize(sentence);
        // A conjunction can introduce a second imperative, not just the first command in a turn.
        for clause in normalized
            .split(" va ")
            .flat_map(|part| part.split(" and "))
        {
            let clause = request_body(clause);
            let command =
                command_tail(clause.trim_start_matches(|c: char| c.is_ascii_digit() || c == ' '));
            if is_negative(command) {
                return false; // Explicit refusal wins over conflicting requests in the same turn.
            }
            requested |= is_delegation_command(command);
        }
    }
    requested
}

fn request_body(clause: &str) -> &str {
    if let Some((heading, body)) = clause.rsplit_once(':') {
        // Only a request heading introduces a command after a colon. Labels/logs do not.
        if [
            "yeu cau sau",
            "yeu cau",
            "thuc hien",
            "luu y",
            "cach lam",
            "request",
            "instructions",
        ]
        .iter()
        .any(|suffix| heading.trim_end().ends_with(suffix))
        {
            return body.trim();
        }
    }
    clause
}

fn strip_phrase<'a>(text: &'a str, phrase: &str) -> Option<&'a str> {
    text.strip_prefix(phrase).and_then(|rest| {
        if rest.is_empty() || rest.starts_with(' ') || rest.starts_with(':') {
            Some(rest.trim_start_matches([' ', ':']))
        } else {
            None
        }
    })
}

fn command_tail(mut text: &str) -> &str {
    for _ in 0..12 {
        let next = [
            "i would like you to",
            "i want you to",
            "can you",
            "could you",
            "please",
            "toi yeu cau",
            "toi muon",
            "minh muon",
            "vui long",
            "lam on",
            "nho ban",
            "giup toi",
            "giup t",
            "sau do",
            "bay gio",
            "ban",
            "hay",
            "gio",
            "thu",
            "nho",
            "then",
            "now",
            "try to",
            "try",
        ]
        .iter()
        .find_map(|prefix| strip_phrase(text, prefix));
        let Some(next) = next else { break };
        text = next;
    }
    text
}

fn action_target(command: &str) -> Option<&str> {
    [
        "su dung",
        "phan chia",
        "chia",
        "tach",
        "dung",
        "tao",
        "mo",
        "use",
        "spawn",
        "create",
        "launch",
        "start",
        "split",
        "delegate",
        "distribute",
        "assign",
    ]
    .iter()
    .find_map(|verb| strip_phrase(command, verb))
}

fn is_delegation_command(command: &str) -> bool {
    let Some(target) = action_target(command).filter(|target| is_agent_target(target)) else {
        return false;
    };
    if [
        "phan chia",
        "chia",
        "tach",
        "split",
        "spawn",
        "delegate",
        "distribute",
        "assign",
    ]
    .iter()
    .any(|verb| strip_phrase(command, verb).is_some())
    {
        return true;
    }
    // Choosing the current MCP agent or creating a settings profile is not delegation.
    // Ambiguous verbs (use/create/start) need an explicit plural or child object.
    let mut words = target.split_whitespace().peekable();
    let mut multiple = false;
    while let Some(word) = words.next() {
        let word = word.trim_end_matches(':');
        if matches!(word, "agent" | "agents" | "subagent" | "subagents") {
            return multiple || word != "agent" || words.peek() == Some(&"con");
        }
        multiple |= word.parse::<u32>().is_ok_and(|count| count > 1)
            || matches!(
                word,
                "multiple"
                    | "several"
                    | "two"
                    | "three"
                    | "four"
                    | "five"
                    | "nhieu"
                    | "vai"
                    | "cac"
                    | "hai"
                    | "ba"
                    | "bon"
                    | "nam"
                    | "sub"
                    | "multi"
            );
    }
    false
}

fn is_negative(command: &str) -> bool {
    let negative = [
        "khong can",
        "khong duoc",
        "khong muon",
        "khong",
        "ko can",
        "ko",
        "k can",
        "k",
        "chua can",
        "chua",
        "do not",
        "don't",
        "dont",
        "never",
        "without",
        "no",
        "avoid",
        "tranh",
        "stop using",
    ]
    .iter()
    .find_map(|prefix| strip_phrase(command, prefix));
    let Some(rest) = negative else { return false };
    let rest = command_tail(rest);
    is_agent_target(action_target(rest).unwrap_or(rest))
}

fn is_agent_target(target: &str) -> bool {
    let mut words = target.split_whitespace();
    // Restrict the command's object, not arbitrary later mentions of an agent module.
    for _ in 0..16 {
        let Some(word) = words.next() else {
            return false;
        };
        if matches!(
            word.trim_end_matches(':'),
            "agent" | "agents" | "subagent" | "subagents"
        ) {
            let next = words
                .next()
                .filter(|word| *word != "con")
                .or_else(|| words.next());
            return !next.is_some_and(|suffix| {
                matches!(
                    suffix,
                    "bi" | "la"
                        | "dang"
                        | "khong"
                        | "ko"
                        | "chua"
                        | "is"
                        | "are"
                        | "was"
                        | "were"
                        | "fails"
                        | "failed"
                        | "does"
                        | "doesn't"
                        | "settings"
                        | "setting"
                        | "page"
                        | "logic"
                        | "function"
                        | "profile"
                        | "profiles"
                        | "module"
                        | "modules"
                        | "class"
                        | "configuration"
                )
            });
        }
        if !word.chars().all(|c| c.is_ascii_digit())
            && !matches!(
                word,
                "ra" | "thanh"
                    | "cong"
                    | "viec"
                    | "nho"
                    | "cac"
                    | "nhieu"
                    | "vai"
                    | "mot"
                    | "hai"
                    | "ba"
                    | "bon"
                    | "nam"
                    | "cho"
                    | "giup"
                    | "t"
                    | "toi"
                    | "file"
                    | "files"
                    | "the"
                    | "this"
                    | "work"
                    | "task"
                    | "tasks"
                    | "review"
                    | "across"
                    | "between"
                    | "into"
                    | "among"
                    | "to"
                    | "multiple"
                    | "several"
                    | "parallel"
                    | "a"
                    | "an"
                    | "one"
                    | "two"
                    | "three"
                    | "four"
                    | "five"
                    | "sub"
                    | "multi"
                    | "any"
            )
        {
            return false;
        }
    }
    false
}
