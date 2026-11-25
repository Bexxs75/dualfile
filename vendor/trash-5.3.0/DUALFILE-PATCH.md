# Dualfile integration / Dualfile-Integration

Source: local Cargo registry source `trash-5.3.0`, copied without network access.
Quelle: lokale Cargo-Registry-Quelle `trash-5.3.0`, ohne Netzwerkzugriff kopiert.

The checkout did not contain the vendor directory named in the frozen order.
This path dependency is shared by the core and the separate Tauri workspace.
Der Checkout enthielt das im Auftrag genannte Vendor-Verzeichnis nicht.
Core und separater Tauri-Workspace verwenden diese Pfadabhängigkeit.

Changes / Änderungen:

- `freedesktop.rs::home_trash()`: the added `packaging() == Flatpak` branch
  resolves the home trash through the shared host data directory.
  Der ergänzte Flatpak-Zweig löst den Home-Papierkorb über den Host-Datenpfad auf.
- `freedesktop.rs::get_mount_points()` (Linux): the added `packaging() == Flatpak`
  branch reads and parses the host mount table through `dualfile-platform`.
  Der ergänzte Flatpak-Zweig liest und parst die Host-Mountliste über
  `dualfile-platform`. Beide Ergänzungen gelten ausschließlich für Flatpak.
- `freedesktop.rs` (#56, marked `dualfile patch:`): `eval_trash_folders()` lists a
  per-mount trash only if `dualfile_platform::trash_dir::owned_trash_dir` trusts it
  (no symlink, owned by the effective user, not group/other-writable, also for
  `files/` and `info/`); `purge_all()`/`restore_all()` reopen the item's trash through
  `open_item_trash()` and remove/rename/unlink only via those handles without
  following symlinks. The home trash stays trusted as it is.
  Pro-Mount-Papierkörbe werden nur nach der gemeinsamen Eigentümer-/Modusprüfung
  gelistet; Löschen/Wiederherstellen laufen nur über geprüfte Verzeichnis-Handles.
- `Cargo.toml`: local dependency on `dualfile-platform`.
  Lokale Abhängigkeit von `dualfile-platform`.
- The default `freedesktop` feature remains enabled. The separate portal feature
  cannot list/restore/purge trash and is not enabled or altered.
  Das Standardfeature `freedesktop` bleibt aktiv. Das separate Portalfeature kann
  nicht auflisten/wiederherstellen/leeren und wird weder aktiviert noch geändert.

MIT license retained in `LICENSE.txt`; already listed in `THIRD-PARTY-LICENSES.md`.
MIT-Lizenz in `LICENSE.txt`; bereits in `THIRD-PARTY-LICENSES.md` aufgeführt.
