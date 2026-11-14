# Contributing / Mitwirken

## English

Dualfile is a Linux file manager in active development. Open an issue describing the problem and expected behavior before proposing a major change. Product decisions, merges and releases require maintainer approval.

1. Use a branch and keep changes focused. Follow the [Code of Conduct](CODE_OF_CONDUCT.md).
2. Install the development prerequisites in [README.md](README.md#development), then run `npm ci`.
3. Run `npm run typecheck`, `npm test` and `npm run build` for frontend changes.
4. For Rust changes, run `cargo fmt --check`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` in each affected workspace. The repository root and `src-tauri` are separate workspaces.
5. For release-script changes, run `python3 scripts/test_make_latest_json.py`. Regenerate the license inventory with `python3 scripts/gen-third-party.py` when dependencies change; missing metadata must be resolved before distribution.
6. Update English/German UI strings and the documentation. Describe user-visible changes in `CHANGELOG.md` and include sanitized screenshots for UI work.
7. In a pull request, explain the problem, resulting behavior, tests, limitations and relevant issue. Report checks you could not run.

Test file operations with disposable fixtures. Include regression coverage for destructive operations, archive path handling, cancellation and remote failures when changing those behaviors. Never include credentials or private filenames in examples, logs or screenshots. Security vulnerabilities belong in the private channel described in [SECURITY.md](SECURITY.md).

Contributions are provided under the repository's MIT license; third-party components keep their own licenses. Do not assume the generated inventory is a distribution clearance.
