use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWindowInfo {
    pub window_id: String,
    pub title: String,
    pub application: String,
    pub process_id: u32,
    pub width: u32,
    pub height: u32,
    pub minimized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopWindowList {
    pub windows: Vec<DesktopWindowInfo>,
    pub excluded_window_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopObserveRequest {
    pub window_id: String,
    #[serde(default = "default_true")]
    pub include_screenshot: bool,
    #[serde(default = "default_true")]
    pub include_elements: bool,
}

const fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopObservation {
    pub observation_id: String,
    pub window: DesktopWindowInfo,
    pub elements: Vec<DesktopElement>,
    pub elements_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screenshot_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopElement {
    pub element_id: String,
    pub name: String,
    pub automation_id: String,
    pub control_type: String,
    pub class_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bounds: Option<DesktopRect>,
    pub enabled: bool,
    pub offscreen: bool,
    pub password: bool,
    pub supported_actions: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopElementActRequest {
    pub observation_id: String,
    pub element_id: String,
    #[serde(flatten)]
    pub action: DesktopElementAction,
}

impl<'de> Deserialize<'de> for DesktopElementActRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let wire = DesktopElementActWire::deserialize(deserializer)?;
        Ok(match wire {
            DesktopElementActWire::Invoke {
                observation_id,
                element_id,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::Invoke,
            },
            DesktopElementActWire::SetValue {
                observation_id,
                element_id,
                text,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::SetValue { text },
            },
            DesktopElementActWire::Toggle {
                observation_id,
                element_id,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::Toggle,
            },
            DesktopElementActWire::Select {
                observation_id,
                element_id,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::Select,
            },
            DesktopElementActWire::Expand {
                observation_id,
                element_id,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::Expand,
            },
            DesktopElementActWire::Collapse {
                observation_id,
                element_id,
            } => Self {
                observation_id,
                element_id,
                action: DesktopElementAction::Collapse,
            },
        })
    }
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
enum DesktopElementActWire {
    Invoke {
        observation_id: String,
        element_id: String,
    },
    SetValue {
        observation_id: String,
        element_id: String,
        text: String,
    },
    Toggle {
        observation_id: String,
        element_id: String,
    },
    Select {
        observation_id: String,
        element_id: String,
    },
    Expand {
        observation_id: String,
        element_id: String,
    },
    Collapse {
        observation_id: String,
        element_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum DesktopElementAction {
    Invoke,
    SetValue { text: String },
    Toggle,
    Select,
    Expand,
    Collapse,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopActionResult {
    pub completed: bool,
    pub observation_invalidated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopInputBeginRequest {
    pub window_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInputSessionInfo {
    pub input_session_id: String,
    pub window_id: String,
    pub active: bool,
    pub escape_to_stop: bool,
    pub overlay_visible: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopInputActRequest {
    pub input_session_id: String,
    pub actions: Vec<DesktopInputAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(
    tag = "type",
    rename_all = "snake_case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum DesktopInputAction {
    Click {
        x: i32,
        y: i32,
        #[serde(default)]
        button: DesktopMouseButton,
    },
    DoubleClick {
        x: i32,
        y: i32,
        #[serde(default)]
        button: DesktopMouseButton,
    },
    Move {
        x: i32,
        y: i32,
    },
    Drag {
        start_x: i32,
        start_y: i32,
        end_x: i32,
        end_y: i32,
        #[serde(default = "default_drag_duration_ms")]
        duration_ms: u64,
    },
    Scroll {
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
    },
    Keypress {
        keys: Vec<String>,
    },
    Type {
        text: String,
    },
    Wait {
        duration_ms: u64,
    },
}

const fn default_drag_duration_ms() -> u64 {
    500
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DesktopMouseButton {
    #[default]
    Left,
    Right,
    Middle,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DesktopInputEndRequest {
    pub input_session_id: String,
}
