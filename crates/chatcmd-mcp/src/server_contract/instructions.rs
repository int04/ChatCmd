use sha2::{Digest, Sha256};
use std::sync::LazyLock;

pub(crate) const INSTRUCTIONS_VERSION: &str = "coding-core-v2";

const CODING_CORE: &str = include_str!("instructions/coding.md");
const ROLE_PROMPTS: &str = include_str!("instructions/roles.md");
const SUBAGENT_ROLE: &str = include_str!("instructions/subagent.md");

static CORE_HASH: LazyLock<String> = LazyLock::new(|| {
    let mut hasher = Sha256::new();
    hasher.update(CODING_CORE.as_bytes());
    hasher.update(b"\n--roles--\n");
    hasher.update(ROLE_PROMPTS.as_bytes());
    format!("{:x}", hasher.finalize())
});

pub(crate) fn core_hash() -> &'static str {
    CORE_HASH.as_str()
}

pub(crate) fn parent_bundle(protocol: &str, workspace: &str) -> String {
    format!(
        "CHATCMD_INSTRUCTIONS_VERSION={} CHATCMD_INSTRUCTIONS_HASH={}\n\n{}\n\n{}\n\n{}\n\n{}",
        INSTRUCTIONS_VERSION,
        core_hash(),
        protocol,
        workspace,
        CODING_CORE,
        ROLE_PROMPTS
    )
}

pub(crate) fn child_core() -> String {
    format!(
        "CHATCMD_INSTRUCTIONS_VERSION={} CHATCMD_INSTRUCTIONS_HASH={}\n\n{}\n\n{}\n\n{}",
        INSTRUCTIONS_VERSION,
        core_hash(),
        CODING_CORE,
        ROLE_PROMPTS,
        SUBAGENT_ROLE
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn coding_rule_ids_are_complete_and_unique() {
        let ids = CODING_CORE
            .split_whitespace()
            .filter_map(|word| word.strip_prefix("COD-"))
            .filter_map(|suffix| suffix.get(..2))
            .filter(|suffix| suffix.chars().all(|character| character.is_ascii_digit()))
            .map(|suffix| format!("COD-{suffix}"))
            .collect::<Vec<_>>();
        let unique = ids.iter().cloned().collect::<HashSet<_>>();
        assert_eq!(ids.len(), 16);
        assert_eq!(unique.len(), 16);
        for number in 1..=16 {
            assert!(unique.contains(&format!("COD-{number:02}")));
        }
    }

    #[test]
    fn parent_and_child_share_identical_core_identity() {
        let parent = parent_bundle("protocol", "workspace");
        let child = child_core();
        for marker in [INSTRUCTIONS_VERSION, core_hash(), "COD-01", "COD-16"] {
            assert!(parent.contains(marker));
            assert!(child.contains(marker));
        }
        assert!(child.contains("DELEGATED CHILD ROLE"));
        assert!(!child.contains("FIRST TOOL RULE"));
    }

    #[test]
    fn hash_is_stable_sha256_hex() {
        assert_eq!(core_hash().len(), 64);
        assert!(core_hash().bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_eq!(core_hash(), core_hash());
    }

    #[test]
    fn empty_skill_discovery_never_gates_initialize_or_child_core() {
        let empty_skills = Vec::<String>::new();
        assert!(empty_skills.is_empty());
        let initialize = parent_bundle("protocol", "workspace");
        let text_child = crate::subagent_protocol::child_system_prompt("text", true, &[]);
        let tool_child = crate::subagent_protocol::child_system_prompt("tools", false, &[]);
        for bundle in [initialize, text_child, tool_child] {
            assert!(bundle.contains(INSTRUCTIONS_VERSION));
            assert!(bundle.contains(core_hash()));
            assert!(bundle.contains("COD-01") && bundle.contains("COD-16"));
        }
    }
}
