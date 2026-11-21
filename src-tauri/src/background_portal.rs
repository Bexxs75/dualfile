//! Flatpak login startup. The saved mode is the last portal-confirmed choice;
//! the Background portal has no query for the current autostart command.
use crate::autostart::{Mode, MINIMIZED_FLAG};
use futures_util::StreamExt;
use std::collections::HashMap;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

fn state_path() -> std::path::PathBuf {
    dualfile_core::settings::config_dir().join("portal-autostart.json")
}
pub fn status() -> (Mode, Option<String>) {
    match std::fs::read(state_path()) {
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(mode) => (mode, None),
            Err(_) => (
                Mode::Off,
                Some("Autostart-Status nicht lesbar / Cannot read autostart status".into()),
            ),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Mode::Off, None),
        Err(_) => (
            Mode::Off,
            Some("Autostart-Status nicht lesbar / Cannot read autostart status".into()),
        ),
    }
}
fn commandline(executable: String, mode: Mode) -> Vec<String> {
    let mut args = vec![executable];
    if mode == Mode::Minimized {
        args.push(MINIMIZED_FLAG.into());
    }
    args
}
fn accepted(code: u32, values: &HashMap<String, OwnedValue>, mode: Mode) -> bool {
    code == 0
        && values.get("autostart").and_then(|v| bool::try_from(v).ok()) == Some(mode != Mode::Off)
}
pub async fn set(mode: Mode) -> Result<Mode, String> {
    // Serialize portal prompts and the persisted answer.
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _lock = LOCK.lock().await;
    request(mode)
        .await
        .map_err(|e| format!("Autostart-Portal fehlgeschlagen / Autostart portal failed: {e}"))?;
    let path = state_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| {
        format!("Autostart-Status nicht gespeichert / Cannot save autostart status: {e}")
    })?;
    crate::desktop_entry::write_atomic(
        &path,
        &serde_json::to_vec(&mode).map_err(|e| e.to_string())?,
    )
    .map_err(|e| {
        format!("Autostart-Status nicht gespeichert / Cannot save autostart status: {e}")
    })?;
    Ok(mode)
}
async fn request(mode: Mode) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let connection = zbus::Connection::session().await?;
    let portal = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Background",
    )
    .await?;
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let token = format!(
        "dualfile_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let sender = connection
        .unique_name()
        .ok_or("Missing bus name")?
        .as_str()
        .trim_start_matches(':')
        .replace('.', "_");
    let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let request = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        path.as_str(),
        "org.freedesktop.portal.Request",
    )
    .await?;
    // Subscribe before the method call: a quick response must not be lost.
    let mut responses = request.receive_signal("Response").await?;
    let exe = std::env::current_exe()?
        .into_os_string()
        .into_string()
        .map_err(|_| "Invalid executable")?;
    let mut options: HashMap<&str, Value<'_>> = HashMap::new();
    options.insert("handle_token", Value::from(token.as_str()));
    options.insert(
        "reason",
        Value::from("Dualfile nach der Anmeldung starten / Start Dualfile after login"),
    );
    options.insert("autostart", Value::from(mode != Mode::Off));
    options.insert("commandline", Value::from(commandline(exe, mode)));
    options.insert("dbus-activatable", Value::from(false));
    let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        let returned: OwnedObjectPath = portal.call("RequestBackground", &("", options)).await?;
        if returned.as_str() != path {
            return Err("Unexpected portal request path".into());
        }
        let message = responses.next().await.ok_or("Portal disconnected")?;
        let (code, values): (u32, HashMap<String, OwnedValue>) = message.body().deserialize()?;
        if !accepted(code, &values, mode) {
            return Err("Abgelehnt oder abgebrochen / Denied or cancelled".into());
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })
    .await;
    match result {
        Ok(result) => result,
        Err(_) => {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                request.call::<_, _, ()>("Close", &()),
            )
            .await;
            Err("Zeitüberschreitung / Timed out".into())
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn flatpak_minimized_and_portal_denial() {
        assert_eq!(
            commandline("/app/bin/dualfile".into(), Mode::Minimized),
            ["/app/bin/dualfile", "--minimized"]
        );
        assert_eq!(
            commandline("/app/bin/dualfile".into(), Mode::Shown),
            ["/app/bin/dualfile"]
        );
        let values = HashMap::from([("autostart".into(), OwnedValue::from(true))]);
        assert!(accepted(0, &values, Mode::Shown));
        assert!(!accepted(1, &values, Mode::Shown));
        assert!(!accepted(0, &values, Mode::Off));
        assert!(!accepted(0, &HashMap::new(), Mode::Shown));
    }
}
