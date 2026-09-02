use std::{
    collections::{BTreeMap, HashSet},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};

use axum::{Json, extract::State};
use reqwest::Method;
use serde::Serialize;
use serde_json::Value;

use crate::websocket::AppState;

use super::{Problem, auth};

const STATISTIC_PATH: &str = "/api/statistics/statistic";
const SKILL_USE_PATH: &str = "/api/statistics/skill-use";
const FLUSH_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
struct Counters {
    count_turn: u64,
    count_conversion: u64,
    count_agent: u64,
    count_tool_use: u64,
    count_skill: u64,
    skills: BTreeMap<String, u64>,
    seen_requests: HashSet<String>,
}

#[derive(Clone, Copy, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatisticSnapshot {
    count_turn: u64,
    count_conversion: u64,
    count_agent: u64,
    count_tool_use: u64,
    count_skill: u64,
}

fn counters() -> &'static Mutex<Counters> {
    static COUNTERS: OnceLock<Mutex<Counters>> = OnceLock::new();
    COUNTERS.get_or_init(|| Mutex::new(Counters::default()))
}

pub(crate) fn record_mcp_success(request_id: &str, tool: &str, arguments: &Value, output: &Value) {
    let Ok(mut state) = counters().lock() else {
        return;
    };
    if !state.seen_requests.insert(request_id.to_owned()) {
        return;
    }

    match tool {
        "agent_user_message" => {
            if !is_duplicate(output) {
                state.count_turn = state.count_turn.saturating_add(1);
                if output
                    .get("isFirstMessage")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                {
                    state.count_conversion = state.count_conversion.saturating_add(1);
                }
            }
        }
        "agent_subagent_start" => {
            if !is_duplicate(output) {
                state.count_agent = state.count_agent.saturating_add(1);
            }
        }
        _ if !tool.starts_with("agent_") => {
            state.count_tool_use = state.count_tool_use.saturating_add(1);
            if tool == "skill_read"
                && let Some(skill_name) = skill_name(arguments)
            {
                state.count_skill = state.count_skill.saturating_add(1);
                *state.skills.entry(skill_name.to_owned()).or_default() += 1;
            }
        }
        _ => {}
    }
}

pub(crate) fn start_flush_scheduler(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(FLUSH_INTERVAL).await;
            flush(&state).await;
        }
    });
}

pub(crate) async fn logout(State(state): State<Arc<AppState>>) -> Result<Json<Value>, Problem> {
    flush(&state).await;
    auth::logout(State(state)).await
}

async fn flush(state: &Arc<AppState>) {
    let statistic = statistic_snapshot();
    if has_statistic_data(statistic) && send_statistic(state, statistic).await {
        subtract_statistic(statistic);
    }

    let skills = skill_snapshot();
    if !skills.is_empty() && send_skill_use(state, &skills).await {
        subtract_skills(&skills);
    }
}

fn statistic_snapshot() -> StatisticSnapshot {
    counters().lock().map_or_else(
        |_| StatisticSnapshot::default(),
        |state| StatisticSnapshot {
            count_turn: state.count_turn,
            count_conversion: state.count_conversion,
            count_agent: state.count_agent,
            count_tool_use: state.count_tool_use,
            count_skill: state.count_skill,
        },
    )
}

fn skill_snapshot() -> BTreeMap<String, u64> {
    counters()
        .lock()
        .map_or_else(|_| BTreeMap::new(), |state| state.skills.clone())
}

fn has_statistic_data(value: StatisticSnapshot) -> bool {
    value.count_turn > 0
        || value.count_conversion > 0
        || value.count_agent > 0
        || value.count_tool_use > 0
        || value.count_skill > 0
}

async fn send_statistic(state: &Arc<AppState>, snapshot: StatisticSnapshot) -> bool {
    let Ok(body) = serde_json::to_vec(&snapshot) else {
        return false;
    };
    auth::authorized_request(state, Method::POST, STATISTIC_PATH, &body, None)
        .await
        .is_ok_and(|response| response.status().is_success())
}

async fn send_skill_use(state: &Arc<AppState>, skills: &BTreeMap<String, u64>) -> bool {
    let Ok(body) = serde_json::to_vec(skills) else {
        return false;
    };
    auth::authorized_request(state, Method::POST, SKILL_USE_PATH, &body, None)
        .await
        .is_ok_and(|response| response.status().is_success())
}

fn subtract_statistic(sent: StatisticSnapshot) {
    let Ok(mut state) = counters().lock() else {
        return;
    };
    state.count_turn = state.count_turn.saturating_sub(sent.count_turn);
    state.count_conversion = state.count_conversion.saturating_sub(sent.count_conversion);
    state.count_agent = state.count_agent.saturating_sub(sent.count_agent);
    state.count_tool_use = state.count_tool_use.saturating_sub(sent.count_tool_use);
    state.count_skill = state.count_skill.saturating_sub(sent.count_skill);
}

fn subtract_skills(sent: &BTreeMap<String, u64>) {
    let Ok(mut state) = counters().lock() else {
        return;
    };
    for (skill_name, sent_count) in sent {
        let Some(count) = state.skills.get_mut(skill_name) else {
            continue;
        };
        *count = count.saturating_sub(*sent_count);
        if *count == 0 {
            state.skills.remove(skill_name);
        }
    }
}

fn is_duplicate(output: &Value) -> bool {
    output
        .get("duplicate")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn skill_name(arguments: &Value) -> Option<&str> {
    arguments
        .get("skillId")
        .or_else(|| arguments.get("skill_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn records_turn_tool_and_skill_without_duplicate_requests() {
        let mut state = counters().lock().expect("telemetry counters");
        *state = Counters::default();
        drop(state);

        let first = json!({"duplicate": false, "isFirstMessage": true});
        record_mcp_success("r1", "agent_user_message", &json!({}), &first);
        record_mcp_success("r1", "agent_user_message", &json!({}), &first);
        record_mcp_success(
            "r2",
            "skill_read",
            &json!({"skillId": "rust-skills"}),
            &json!({}),
        );

        let state = counters().lock().expect("telemetry counters");
        assert_eq!(state.count_turn, 1);
        assert_eq!(state.count_conversion, 1);
        assert_eq!(state.count_tool_use, 1);
        assert_eq!(state.count_skill, 1);
        assert_eq!(state.skills.get("rust-skills"), Some(&1));
    }
}
