//! Transition from the pre-0.4.1 identity, before any window state or settings are read.
use crate::{
    autostart::Autostart,
    desktop_entry::{self, DesktopEntry, OLD_ID},
};
use std::{
    fs,
    io::{self, Write},
    path::Path,
};
use tauri::{plugin::TauriPlugin, Manager, Wry};

fn copy_window_state(old: &Path, new: &Path) -> io::Result<()> {
    for name in ["window-position.json", ".window-state.json"] {
        let destination = new.join(name);
        match fs::symlink_metadata(&destination) {
            Ok(_) => continue,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let bytes = match fs::read(old.join(name)) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e),
        };
        fs::create_dir_all(new)?;
        let mut temp = tempfile::NamedTempFile::new_in(new)?;
        temp.write_all(&bytes)?;
        temp.as_file().sync_all()?;
        match temp.persist_noclobber(destination) {
            Ok(_) => {}
            Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.error),
        }
    }
    Ok(())
}

pub fn plugin() -> TauriPlugin<Wry> {
    tauri::plugin::Builder::new("app-id-migration")
        .setup(|app, _| {
            let new = app.path().app_config_dir()?;
            let old = app.path().config_dir()?.join(OLD_ID);
            copy_window_state(&old, &new)?;
            let exe = desktop_entry::executable()?;
            // Integration failures remain recoverable and are retried/reported by system_status.
            if let Err(e) = Autostart::from_env().and_then(|entry| entry.migrate(&exe)) {
                log::warn!("Autostart migration failed: {e}");
            }
            if let Err(e) = desktop_entry::home()
                .and_then(DesktopEntry::new)
                .and_then(|entry| entry.migrate(&exe))
            {
                log::warn!("Start-menu migration failed: {e}");
            }
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_both_states_without_changing_old_or_existing_new_files() {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(OLD_ID);
        let new = root.path().join(desktop_entry::ID);
        copy_window_state(&old, &new).unwrap();
        assert!(!new.exists());
        fs::create_dir_all(&old).unwrap();
        for name in ["window-position.json", ".window-state.json"] {
            fs::write(old.join(name), name).unwrap();
        }
        copy_window_state(&old, &new).unwrap();
        for name in ["window-position.json", ".window-state.json"] {
            assert_eq!(fs::read_to_string(new.join(name)).unwrap(), name);
            assert_eq!(fs::read_to_string(old.join(name)).unwrap(), name);
            fs::write(new.join(name), "new state").unwrap();
        }
        copy_window_state(&old, &new).unwrap();
        for name in ["window-position.json", ".window-state.json"] {
            assert_eq!(fs::read_to_string(new.join(name)).unwrap(), "new state");
            assert_eq!(fs::read_to_string(old.join(name)).unwrap(), name);
        }
    }

    #[test]
    fn runtime_id_matches_bundle_identifier() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(config["identifier"], desktop_entry::ID);
    }
}
