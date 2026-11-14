//! Bounded server observations, independent of the lossless document result FIFO.
use crate::lsp::Encoding;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
    time::Instant,
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProgressToken {
    Number(i32),
    String(String),
}
#[derive(Clone, Debug)]
pub struct WorkProgress {
    pub token: ProgressToken,
    pub title: String,
    pub message: Option<String>,
    pub percentage: Option<u8>,
    pub updated_at: Instant,
}
impl WorkProgress {
    pub fn label(&self) -> String {
        let mut label = self.title.clone();
        if let Some(message) = &self.message {
            label.push_str(": ");
            label.push_str(message);
        }
        if let Some(percent) = self.percentage {
            label.push_str(&format!(" ({percent}%)"));
        }
        label
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Error,
    Warning,
    Info,
    Log,
    Debug,
}
#[derive(Clone, Debug)]
pub struct ServerMessage {
    pub kind: MessageKind,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ProjectStatus {
    pub health: String,
    pub quiescent: bool,
    pub message: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ServerActivity {
    pub process_id: Option<u32>,
    pub server_name: Option<String>,
    pub server_version: Option<String>,
    pub encoding: Option<Encoding>,
    pub synchronization: Option<&'static str>,
    pub analysis_epoch: u64,
    pub project_status: Option<ProjectStatus>,
    pub supports_definition: bool,
    pub supports_hover: bool,
    pub supports_completion: bool,
    pub supports_completion_resolve: bool,
    pub completion_triggers: Vec<String>,
    pub documents: usize,
    pub pending_requests: usize,
    pub progress: Vec<WorkProgress>,
    pub messages: VecDeque<ServerMessage>,
    pub stderr: Arc<str>,
    pub last_activity: Option<Instant>,
}
impl Default for ServerActivity {
    fn default() -> Self {
        Self {
            process_id: None,
            server_name: None,
            server_version: None,
            encoding: None,
            synchronization: None,
            analysis_epoch: 0,
            project_status: None,
            supports_definition: false,
            supports_hover: false,
            supports_completion: false,
            supports_completion_resolve: false,
            completion_triggers: Vec::new(),
            documents: 0,
            pending_requests: 0,
            progress: Vec::new(),
            messages: VecDeque::new(),
            stderr: "".into(),
            last_activity: None,
        }
    }
}
pub(crate) struct Activity {
    pub value: ServerActivity,
    tokens: HashSet<ProgressToken>,
    pub dirty: bool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            value: ServerActivity::default(),
            tokens: HashSet::new(),
            dirty: true,
        }
    }
}
pub(crate) fn bounded(text: &str) -> String {
    text.chars().take(4096).collect()
}
fn token(value: &Value) -> Result<ProgressToken, String> {
    if value.as_str().is_some_and(|s| s.len() > 4096) {
        return Err("Progress token exceeds limit".into());
    }
    serde_json::from_value(value.clone()).map_err(|_| "Invalid progress token".into())
}
impl Activity {
    pub fn analysis_changed(&mut self) {
        self.value.analysis_epoch = self
            .value
            .analysis_epoch
            .checked_add(1)
            .expect("LSP analysis epochs exhausted");
        self.dirty = true;
    }
    pub fn project_status(&mut self, params: &Value) -> Result<(), String> {
        let health = params["health"]
            .as_str()
            .filter(|h| matches!(*h, "ok" | "warning" | "error"))
            .ok_or("Invalid server health")?;
        let quiescent = params["quiescent"]
            .as_bool()
            .ok_or("Invalid server quiescence")?;
        let message = params
            .get("message")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .map(bounded)
                    .ok_or("Invalid server status message")
            })
            .transpose()?;
        if quiescent
            && !self
                .value
                .project_status
                .as_ref()
                .is_some_and(|s| s.quiescent)
        {
            self.analysis_changed();
        }
        self.value.project_status = Some(ProjectStatus {
            health: health.into(),
            quiescent,
            message,
        });
        self.dirty = true;
        Ok(())
    }

    pub fn create(&mut self, value: &Value) -> Result<(), String> {
        let token = token(value)?;
        if self.tokens.contains(&token) {
            return Err("Progress token already exists".into());
        }
        if self.tokens.len() >= 128 {
            return Err("Too many progress tokens".into());
        }
        self.tokens.insert(token);
        Ok(())
    }
    pub fn log(&mut self, kind: MessageKind, text: &str) {
        if self.value.messages.len() == 64 {
            self.value.messages.pop_front();
        }
        self.value.messages.push_back(ServerMessage {
            kind,
            text: bounded(text),
        });
        self.dirty = true;
    }
    pub fn message(&mut self, params: &Value) {
        let Some(text) = params["message"].as_str() else {
            return;
        };
        let kind = match params["type"].as_u64() {
            Some(1) => MessageKind::Error,
            Some(2) => MessageKind::Warning,
            Some(3) => MessageKind::Info,
            Some(5) => MessageKind::Debug,
            _ => MessageKind::Log,
        };
        self.log(kind, text);
    }
    pub fn progress(&mut self, params: &Value) -> Result<(), String> {
        let token = token(&params["token"])?;
        if !self.tokens.contains(&token) {
            return Ok(());
        }
        let value = &params["value"];
        let percentage = match value.get("percentage") {
            None => None,
            Some(v) => Some(
                v.as_u64()
                    .filter(|n| *n <= 100)
                    .ok_or("Invalid progress percentage")? as u8,
            ),
        };
        let message = match value.get("message") {
            None => None,
            Some(v) => Some(bounded(v.as_str().ok_or("Invalid progress message")?)),
        };
        match value["kind"].as_str() {
            Some("begin") => {
                let title = bounded(value["title"].as_str().ok_or("Missing progress title")?);
                if self.value.progress.len() >= 64 {
                    return Err("Too many active progress operations".into());
                }
                if self.value.progress.iter().any(|p| p.token == token) {
                    return Err("Progress already started".into());
                }
                self.value.progress.push(WorkProgress {
                    token,
                    title,
                    message,
                    percentage,
                    updated_at: Instant::now(),
                });
            }
            Some("report") => {
                if let Some(p) = self.value.progress.iter_mut().find(|p| p.token == token) {
                    if message.is_some() {
                        p.message = message;
                    }
                    if percentage.is_some() {
                        p.percentage = percentage;
                    }
                    p.updated_at = Instant::now();
                }
            }
            Some("end") => {
                self.tokens.remove(&token);
                let before = self.value.progress.len();
                self.value.progress.retain(|p| p.token != token);
                if self.value.progress.len() != before {
                    self.analysis_changed();
                }
            }
            _ => return Err("Invalid progress kind".into()),
        }
        self.dirty = true;
        Ok(())
    }
    pub fn stopped(&mut self) {
        self.value.progress.clear();
        self.tokens.clear();
        self.value.pending_requests = 0;
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn progress_preserves_omitted_fields_and_separates_number_and_string_tokens() {
        let mut a = Activity::default();
        a.create(&json!(1)).unwrap();
        a.create(&json!("1")).unwrap();
        for token in [json!(1), json!("1")] {
            a.progress(&json!({"token":token,"value":{"kind":"begin","title":"Indexing","message":"src","percentage":10}})).unwrap();
        }
        a.progress(&json!({"token":1,"value":{"kind":"report","percentage":50}}))
            .unwrap();
        assert_eq!(a.value.progress[0].message.as_deref(), Some("src"));
        assert_eq!(a.value.progress[0].percentage, Some(50));
        assert!(
            a.progress(&json!({"token":1,"value":{"kind":"report","percentage":101}}))
                .is_err()
        );
        a.progress(&json!({"token":1,"value":{"kind":"end"}}))
            .unwrap();
        assert_eq!(a.value.progress.len(), 1);
        a.stopped();
        assert!(a.value.progress.is_empty());
    }
    #[test]
    fn progress_and_message_retention_are_bounded() {
        let mut a = Activity::default();
        for i in 0..128 {
            a.create(&json!(i)).unwrap();
        }
        assert!(a.create(&json!(129)).is_err());
        for _ in 0..100 {
            a.log(MessageKind::Info, &"x".repeat(5000));
        }
        assert_eq!(a.value.messages.len(), 64);
        assert_eq!(a.value.messages[0].text.len(), 4096);
    }
}
