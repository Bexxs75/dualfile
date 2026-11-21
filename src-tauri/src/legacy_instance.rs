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

pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new("legacy-instance")
        .setup(|app, _| {
            let result = (|| -> zbus::Result<bool> {
                let name = format!("{OLD_ID}.SingleInstance");
                let path = format!("/{}", name.replace('.', "/"));
                let connection = Connection::session()?;
                connection.object_server().at(path.as_str(), LegacyEndpoint(app.clone()))?;
                let primary = acquire_or_forward(
                    || Ok(DBusProxy::new(&connection)?.name_has_owner(name.as_str().try_into()?)?),
                    || connection.request_name_with_flags(name.as_str(), zbus::fdo::RequestNameFlags::DoNotQueue.into()).map(|_| ()),
                    || {
                        connection.call_method(
                            Some(name.as_str()), path.as_str(), Some("org.SingleInstance.DBus"), "ExecuteCallback",
                            &(std::env::args_os().map(|arg| arg.to_string_lossy().into_owned()).collect::<Vec<_>>(), std::env::current_dir().unwrap_or_default().to_string_lossy().into_owned()),
                        )?;
                        Ok(())
                    },
                )?;
                if primary { app.resources_table().add(LegacyLock(connection)); }
                Ok(primary)
            })();
            match result {
                Ok(true) => Ok(()),
                Ok(false) => {
                    log::info!("Activated the running Dualfile instance; stopping before migration.");
                    eprintln!("Activated the running Dualfile instance; stopping before migration.");
                    app.cleanup_before_exit();
                    std::process::exit(0);
                },
                Err(e) => {
                    let message = format!("Dualfile could not safely check for an existing instance. Please close other Dualfile instances and check the session bus before trying again.\n\n{e}");
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
