# Plan: "What's new" window after an update (target 0.3.5)

Owner request (2026-11-16): after every update, show once on the first start a window listing what changed and improved, in the UI language (German or English).

## Behaviour
- New setting `last_seen_version: Option<String>`, missing key loads without `Repaired` (same rule as the sidebar keys).
- On start, after settings load:
  - outcome `Fresh` (no settings file = new install): no window; set `last_seen_version` to the current version.
  - `last_seen_version` missing (update from ≤ 0.3.4) or semver-lower than the app version: show the window with every version `> last_seen` (or only the current one if the key is missing) and `<= current`, newest first. Compare with real semver, not strings (`0.3.10 > 0.3.9`).
  - Equal or higher (downgrade): nothing.
- `last_seen_version` is written when the window is closed (button, Escape, X), not when it opens, so a crash during start shows it again.
- Not shown while `NewerVersion` read-only mode is active (settings cannot be saved, it would appear on every start).
- Reopen any time via menu/palette entry "Neuigkeiten" / "What's new".
- Modal with heading "Neu in Dualfile X" / "What's new in Dualfile X", groups Neu/Verbessert/Behoben (New/Improved/Fixed), link "Vollständiger Changelog auf GitHub" (opens the release page via `open_url`), button "Alles klar"/"Got it", focus on the button, Escape closes. When several versions were skipped, a subtitle says so and each version gets its own heading with date. Long lists scroll inside the dialog.

## Content
- New file `src/release-notes.ts` (or JSON): per version `{date, de:{new,improved,fixed}, en:{…}}`, short user-facing sentences (not the full CHANGELOG text). Bundled into the frontend; works offline.
- Seed with 0.3.3, 0.3.4 and 0.3.5.
- Vitest guard: the version in `tauri.conf.json` must have an entry with non-empty DE and EN text; otherwise CI fails. Release checklist gets a step "release notes DE+EN".

## Files
- `crates/dualfile-core/src/settings.rs` (+ `convert.rs`, `validate.rs`: ≤ 32 chars, semver pattern), `src/ipc/types.ts`, mock defaults.
- `src/state/releaseNotes.ts` (pure: select versions to show; semver compare; unit tests incl. fresh install, missing key, skipped versions, downgrade, newer-version mode).
- `src/components/dialogs/WhatsNewDialog.tsx`, `src/app/state.ts` (dialog kind `whatsNew`), `src/app/controller.ts` (decide after `start()` once phase is ready; mark seen on close), `src/app/commands.ts` (palette/menu entry), i18n strings DE/EN.
- `CHANGELOG.md`, wiki page (settings/updates), screenshot of the dialog.

## Tests / VM
- Vitest: selection logic, dialog renders in DE and EN, close stores the version, not shown on fresh install, reopen via palette.
- VM: install 0.3.4 → update to 0.3.5 pre-release build → window appears once in German; restart → not again; switch to English and reopen via menu.

Mockup approved by the owner (2026-11-16): https://claude.ai/artifact/KSr1QFah2GbTNS8qoMuhuP

## Amendment after Codex review (2026-11-17, mandatory before building)
- **WN-01 (medium):** keep the version marker monotonic. On close, update `last_seen_version` only if the current version is semver-higher than the stored marker or the marker is missing. Define manual reopening (menu/palette) separately from the automatic selection filter: it always shows the notes of the current version (and, if useful, older ones), regardless of the marker. Regression test: confirm 0.3.6 → start 0.3.5, open and close manually → start 0.3.6 again: no window.
- Integration with 0.3.5: the seed for 0.3.5 contains the already built points "Terminal closes with Ctrl+D" (#2) and "Window size and position are remembered" (#3, Wayland: size and maximized state only), plus this window itself. Write the text short and user-facing, in DE and EN.
