//! Answer routing independent of AppState's connection locks: connect can wait while an
//! answer command runs. No answer is emitted, serialized, logged or stored in a vault.
use dualfile_remote::askpass::{Prompt, PromptAnswer, PromptHandler, PromptKind};
use dualfile_remote::secret::Secret;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{mpsc, Mutex};
use std::time::Duration;
use tauri::Emitter;

#[derive(Clone, Serialize)]
pub struct SshPromptEvent {
    id: String,
    kind: PromptKind,
    text: String,
    secret: bool,
}
#[derive(Default)]
struct Pending {
    next: u64,
    closed: bool,
    answers: HashMap<String, mpsc::SyncSender<PromptAnswer>>,
}
pub struct SshPromptBridge {
    emit: Box<dyn Fn(SshPromptEvent) -> bool + Send + Sync>,
    pending: Mutex<Pending>,
    timeout: Duration,
}
impl SshPromptBridge {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self {
            emit: Box::new(move |event| app.emit_to("main", "ssh-prompt", event).is_ok()),
            pending: Mutex::default(),
            timeout: Duration::from_secs(120),
        }
    }
    pub fn answer(&self, id: &str, text: Option<String>) -> bool {
        let answer = match text.map(Secret::new) {
            Some(text)
                if text.expose().len() <= 4096 && !text.expose().contains(['\n', '\r', '\0']) =>
            {
                PromptAnswer::Text(text)
            }
            _ => PromptAnswer::Cancel,
        };
        let sender = self
            .pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .answers
            .remove(id);
        sender.is_some_and(|s| s.send(answer).is_ok())
    }
    pub fn close(&self) {
        let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
        pending.closed = true;
        pending.answers.clear();
    }
}
impl PromptHandler for SshPromptBridge {
    fn prompt(&self, prompt: Prompt) -> PromptAnswer {
        let (send, receive) = mpsc::sync_channel(1);
        let id = {
            let mut pending = self.pending.lock().unwrap_or_else(|p| p.into_inner());
            if pending.closed {
                return PromptAnswer::Cancel;
            }
            pending.next += 1;
            let id = pending.next.to_string();
            pending.answers.insert(id.clone(), send);
            id
        };
        let prompt = Prompt::new(&prompt.text);
        let event = SshPromptEvent {
            id: id.clone(),
            kind: prompt.kind,
            text: prompt.text,
            secret: prompt.secret,
        };
        let answer = if (self.emit)(event) {
            receive
                .recv_timeout(self.timeout)
                .unwrap_or(PromptAnswer::Cancel)
        } else {
            PromptAnswer::Cancel
        };
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .answers
            .remove(&id);
        answer
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[test]
    fn answer_timeout_close_and_late_answer() {
        let (send, events) = mpsc::channel();
        let bridge = Arc::new(SshPromptBridge {
            emit: Box::new(move |p| send.send(p).is_ok()),
            pending: Mutex::default(),
            timeout: Duration::from_millis(50),
        });
        let b = bridge.clone();
        let worker = std::thread::spawn(move || b.prompt(Prompt::new("Password:\n")));
        let event = events.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(event.text, "Password:");
        assert!(bridge.answer(&event.id, Some("marker".into())));
        assert!(matches!(worker.join().unwrap(), PromptAnswer::Text(s) if s.expose() == "marker"));
        assert!(!bridge.answer(&event.id, None));
        assert!(matches!(
            bridge.prompt(Prompt::new("OTP")),
            PromptAnswer::Cancel
        ));
        let b = bridge.clone();
        let worker = std::thread::spawn(move || b.prompt(Prompt::new("Password:")));
        bridge.close();
        assert!(matches!(worker.join().unwrap(), PromptAnswer::Cancel));
        assert!(bridge.pending.lock().unwrap().answers.is_empty());
    }
}
