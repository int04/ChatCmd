use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

const fn default_width() -> u32 {
    1280
}

const fn default_height() -> u32 {
    800
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerSessionStartRequest {
    #[serde(default)]
    pub browser: ComputerBrowser,
    #[serde(default = "default_start_url")]
    pub start_url: String,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
}

fn default_start_url() -> String {
    "about:blank".to_owned()
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ComputerBrowser {
    #[default]
    Chrome,
    Edge,
    Brave,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ComputerActRequest {
    pub session_id: String,
    pub actions: Vec<ComputerAction>,
    #[serde(default = "default_true")]
    pub screenshot_after: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ComputerAction {
    Click {
        x: f64,
        y: f64,
        #[serde(default)]
        button: MouseButton,
    },
    DoubleClick {
        x: f64,
        y: f64,
        #[serde(default)]
        button: MouseButton,
    },
    Move {
        x: f64,
        y: f64,
    },
    Drag {
        start_x: f64,
        start_y: f64,
        end_x: f64,
        end_y: f64,
        #[serde(default = "default_drag_duration_ms")]
        duration_ms: u64,
    },
    Scroll {
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default)]
        delta_x: f64,
        delta_y: f64,
    },
    Keypress {
        keys: Vec<String>,
    },
    Type {
        text: String,
    },
    Navigate {
        url: String,
    },
    Wait {
        duration_ms: u64,
    },
    Screenshot,
}

const fn default_drag_duration_ms() -> u64 {
    500
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum MouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub(super) const fn as_cdp(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Right => "right",
            Self::Middle => "middle",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputerSessionInfo {
    pub session_id: String,
    pub browser: ComputerBrowser,
    pub width: u32,
    pub height: u32,
    pub headless: bool,
    pub isolated_profile: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputerObservation {
    pub session_id: String,
    pub url: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_warning: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_action_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub execution_warning: Option<ComputerExecutionWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ComputerExecutionWarning {
    pub completed_action_count: usize,
    pub retry_action: bool,
    pub message: String,
}
