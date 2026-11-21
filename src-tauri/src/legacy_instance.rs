//! Keep the old single-instance endpoint alive as a compatibility alias. A file lock would
//! not protect against 0.4.0, which does not take it. Both identities use the same session bus.
use crate::{desktop_entry::OLD_ID, system};
use tauri::{plugin::TauriPlugin, Manager, Wry};
use zbus::blocking::{fdo::DBusProxy, Connection};

struct LegacyEndpoint(tauri::AppHandle);

#[zbus::interface(name = "org.SingleInstance.DBus")]
impl LegacyEndpoint {
    fn execute_callback(&self, _argv: Vec<String>, _cwd: String) {
        if let Some(window) = self.0.get_webview_window("main") {
            system::bring_to_front(&window);
        }
    }
}

// A resource (not managed state) is dropped by cleanup_before_exit, including updater restart.
struct LegacyLock(Connection);
impl tauri::Resource for LegacyLock {}
impl Drop for LegacyLock {
    fn drop(&mut self) {
        let _ = self.0.release_name(format!("{OLD_ID}.SingleInstance"));
    }
}

/// The probe and atomic claim are separate: another process may start between them.
fn acquire_or_forward(
    has_owner: impl FnOnce() -> zbus::Result<bool>,
    claim: impl FnOnce() -> zbus::Result<()>,
    forward: impl FnOnce() -> zbus::Result<()>,
) -> zbus::Result<bool> {
    if has_owner()? {
        forward()?;
        return Ok(false);
    }
    match claim() {
        Ok(()) => Ok(true),
        Err(zbus::Error::NameTaken) => {
            forward()?;
            Ok(false)
        }
        Err(e) => Err(e),
    }
}

/// Only a session without any bus may skip the check; a configured bus that refuses us
/// (limits, auth, protocol) could still host a running 0.4.0.
fn bus_configured() -> bool {
    std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some_and(|v| !v.is_empty())
        || std::env::var_os("XDG_RUNTIME_DIR")
            .is_some_and(|dir| std::path::Path::new(&dir).join("bus").exists())
        // zbus also falls back to /run/user/<euid>/bus; /proc/self belongs to our euid.
        // If even that cannot be read, assume a bus may exist and stay on the safe side.
        || std::fs::metadata("/proc/self").map_or(true, |meta| {
            let uid = std::os::unix::fs::MetadataExt::uid(&meta);
            std::path::Path::new(&format!("/run/user/{uid}/bus")).exists()
        })
}

pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new("legacy-instance")
        .setup(|app, _| {
            let handle = app.clone();
            // zbus is built for Tokio: its blocking API needs a runtime context and must not run
            // on an async worker, so the check runs on Tauri's blocking pool and setup waits.
            let check = move || -> zbus::Result<(bool, Option<Connection>)> {
                let name = format!("{OLD_ID}.SingleInstance");
                let path = format!("/{}", name.replace('.', "/"));
                // Without any session bus the regular single-instance plugin cannot work
                // either, so only this case may continue; every later failure stops below.
                let connection = match Connection::session() {
                    Ok(connection) => connection,
                    Err(e) if !bus_configured() => {
                        log::warn!("No session bus, skipping the legacy instance check: {e}");
                        eprintln!("No session bus, skipping the legacy instance check: {e}");
                        return Ok((true, None));
                    }
                    Err(e) => return Err(e),
                };
                connection
                    .object_server()
                    .at(path.as_str(), LegacyEndpoint(handle))?;
                let primary = acquire_or_forward(
                    || Ok(DBusProxy::new(&connection)?.name_has_owner(name.as_str().try_into()?)?),
                    || {
                        connection
                            .request_name_with_flags(
                                name.as_str(),
                                zbus::fdo::RequestNameFlags::DoNotQueue.into(),
                            )
                            .map(|_| ())
                    },
                    || {
                        connection.call_method(
                            Some(name.as_str()),
                            path.as_str(),
                            Some("org.SingleInstance.DBus"),
                            "ExecuteCallback",
                            &(
                                std::env::args_os()
                                    .map(|arg| arg.to_string_lossy().into_owned())
                                    .collect::<Vec<_>>(),
                                std::env::current_dir()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .into_owned(),
                            ),
                        )?;
                        Ok(())
                    },
                )?;
                Ok((primary, primary.then_some(connection)))
            };
            let result = tauri::async_runtime::block_on(tauri::async_runtime::spawn_blocking(check))
                .map_err(|e| zbus::Error::Failure(format!("legacy instance check: {e}")))
                .and_then(|checked| checked);
            match result {
                Ok((true, lock)) => {
                    if let Some(connection) = lock {
                        app.resources_table().add(LegacyLock(connection));
                    }
                    Ok(())
                }
                Ok((false, _)) => {
                    log::info!(
                        "Activated the running Dualfile instance; stopping before migration."
                    );
                    eprintln!(
                        "Activated the running Dualfile instance; stopping before migration."
                    );
                    app.cleanup_before_exit();
                    std::process::exit(0);
                }
                // A confirmed or unknown old owner must never run beside us: both would write
                // the same settings.json with only process-local locks.
                Err(e) => {
                    let message = format!("Dualfile could not hand over to an already running Dualfile. Please close other Dualfile windows (also in the tray) and start again.\n\n{e}");
                    log::error!("{message}");
                    eprintln!("{message}");
                    use gtk::prelude::*;
                    let dialog = gtk::MessageDialog::builder()
                        .message_type(gtk::MessageType::Error)
                        .buttons(gtk::ButtonsType::Close)
                        .text(&message)
                        .build();
                    dialog.run();
                    dialog.close();
                    Err(message.into())
                }
            }
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn existing_owner_is_activated_without_claiming_or_starting() {
        let forwarded = Cell::new(false);
        assert!(!acquire_or_forward(
            || Ok(true),
            || panic!("must not claim"),
            || {
                forwarded.set(true);
                Ok(())
            }
        )
        .unwrap());
        assert!(forwarded.get());
    }

    #[test]
    fn only_an_atomic_claim_allows_startup_and_races_forward() {
        assert!(
            acquire_or_forward(|| Ok(false), || Ok(()), || panic!("must not forward")).unwrap()
        );
        assert!(
            !acquire_or_forward(|| Ok(false), || Err(zbus::Error::NameTaken), || Ok(())).unwrap()
        );
    }

    #[test]
    fn bus_errors_and_failed_handoff_never_allow_startup() {
        assert!(acquire_or_forward(
            || Err(zbus::Error::Failure("offline".into())),
            || panic!(),
            || panic!()
        )
        .is_err());
        assert!(acquire_or_forward(
            || Ok(false),
            || Err(zbus::Error::Failure("offline".into())),
            || panic!()
        )
        .is_err());
        assert!(acquire_or_forward(
            || Ok(true),
            || panic!(),
            || Err(zbus::Error::Failure("handoff failed".into()))
        )
        .is_err());
    }
}
