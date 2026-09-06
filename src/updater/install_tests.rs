use std::path::PathBuf;

use uuid::Uuid;

use super::install::verify_extension_payload;

#[test]
fn current_extension_layout_satisfies_update_validator() {
    let extension = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("chatgpt-extension");
    if let Err(error) = verify_extension_payload(&extension) {
        panic!("current chatgpt-extension should be a valid update payload: {error:#}");
    }
}

#[test]
fn extension_manifest_cannot_escape_payload_root() {
    let root = std::env::temp_dir().join(format!("chatcmd-update-test-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&root).expect("create updater test directory");
    let manifest = r#"{
        "manifest_version": 3,
        "background": { "service_worker": "../outside.js" },
        "content_scripts": []
    }"#;
    std::fs::write(root.join("manifest.json"), manifest).expect("write updater test manifest");
    let result = verify_extension_payload(&root);
    let _ = std::fs::remove_dir_all(&root);
    assert!(result.is_err());
}
