# Release checklist

The full release procedure (preflight script, CI gate, short VM test, draft and publish order) is in [release-process.md](release-process.md) (German). This checklist covers the content that must be right before it starts: texts, tests and approval.

- [ ] Set the approved release version and date in the package configuration and changelog.
- [ ] **release notes DE+EN**: add/update `src/release-notes.ts` for the exact version in `src-tauri/tauri.conf.json`, including prerelease versions when packaging one. Use short user-facing text in both languages and confirm the release date.
- [ ] Run `npm run typecheck && npm test -- --maxWorkers=2`; the release-note guard must pass.
- [ ] VM: run the short VM test from [release-process.md](release-process.md) on KDE and GNOME. When the release touches the updater or the release notes dialog, also install the previous version by hand, update to the new build, confirm the German dialog appears once, acknowledge and restart, then switch to English and reopen through Tools → What's new.
- [ ] Review the dialog screenshot and changelog, obtain independent review and maintainer approval before publishing.
