# Release checklist

- [ ] Set the approved release version and date in the package configuration and changelog.
- [ ] **release notes DE+EN**: add/update `src/release-notes.ts` for the exact version in `src-tauri/tauri.conf.json`, including prerelease versions when packaging one. Use short user-facing text in both languages and confirm the release date.
- [ ] Run `npm run typecheck && npm test -- --maxWorkers=2`; the release-note guard must pass.
- [ ] VM: install 0.3.4, update to the 0.3.5 prerelease build, confirm the German dialog appears once, acknowledge and restart, then switch to English and reopen through Tools → What's new.
- [ ] Review the dialog screenshot and changelog, obtain independent review and maintainer approval before publishing.
