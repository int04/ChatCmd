use crate::{
    OperationContext, PolicyContext, PolicyEngine, ProcessInfo, RuntimeError, RuntimeResult,
};
use tokio::process::Command;

#[derive(Clone)]
pub struct ProcessService {
    policy: PolicyEngine,
}

impl ProcessService {
    #[must_use]
    pub fn new(policy: PolicyEngine) -> Self {
        Self { policy }
    }
    pub async fn list(&self) -> RuntimeResult<Vec<ProcessInfo>> {
        let output = if cfg!(windows) {
            Command::new("tasklist.exe")
                .args(["/FO", "CSV", "/NH"])
                .output()
                .await
        } else {
            Command::new("ps")
                .args(["-eo", "pid=,comm=,args="])
                .output()
                .await
        }
        .map_err(command_error)?;
        if !output.status.success() {
            return Err(RuntimeError::new(
                "process_list_failed",
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut values = Vec::new();
        for line in text.lines().take(10_000) {
            if cfg!(windows) {
                let fields: Vec<_> = line.trim_matches('"').split("\",\"").collect();
                if fields.len() >= 2
                    && let Ok(pid) = fields[1].replace(',', "").parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: fields[0].into(),
                        details: line.into(),
                    });
                }
            } else {
                let mut parts = line.trim().splitn(3, char::is_whitespace);
                if let (Some(pid), Some(name)) = (parts.next(), parts.next())
                    && let Ok(pid) = pid.parse()
                {
                    values.push(ProcessInfo {
                        process_id: pid,
                        name: name.into(),
                        details: parts.next().unwrap_or_default().into(),
                    });
                }
            }
        }
        Ok(values)
    }
    pub async fn inspect(&self, process_id: u32) -> RuntimeResult<ProcessInfo> {
        self.list()
            .await?
            .into_iter()
            .find(|process| process.process_id == process_id)
            .ok_or_else(|| RuntimeError::new("process_not_found", "process was not found"))
    }
    pub async fn kill(
        &self,
        context: &OperationContext,
        process_id: u32,
        entire_tree: bool,
    ) -> RuntimeResult<()> {
        self.policy
            .authorize(&PolicyContext {
                agent_id: context.agent_id.clone(),
                tool_name: "process_kill".into(),
                root: None,
                destructive: true,
            })
            .await?;
        let output = if cfg!(windows) {
            let mut command = Command::new("taskkill.exe");
            command.args(["/PID", &process_id.to_string(), "/F"]);
            if entire_tree {
                command.arg("/T");
            }
            command.output().await
        } else {
            Command::new("kill")
                .args(["-TERM", &process_id.to_string()])
                .output()
                .await
        }
        .map_err(command_error)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(RuntimeError::new(
                "process_kill_failed",
                String::from_utf8_lossy(&output.stderr).into_owned(),
            ))
        }
    }
}

fn command_error(error: std::io::Error) -> RuntimeError {
    RuntimeError::new("process_start_failed", error.to_string())
}
