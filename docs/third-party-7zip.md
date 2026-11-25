# Shipped 7-Zip (RAR helper)

AppImage and tarball contain the static 7-Zip console build (`7zzs` from the official Linux
archive, installed as `usr/bin/7zz`) plus `usr/share/doc/7zip/License.txt`. Dualfile uses it
only as a separate program to read RAR archives (`crates/dualfile-core/src/helpers.rs`).

Lookup order for RAR: system `unrar`, then this shipped `7zz`, then `7zz` / `7z` from `PATH`.
Flatpak looks for `/app/bin/7zz`; a plain build ships nothing and relies on the system.

## Pinned version

| Item | Value |
|---|---|
| Version | 7-Zip 26.06 (2026-11-06), https://www.7-zip.org/ links to the GitHub release below |
| x86_64 | `7z2606-linux-x64.tar.xz`, SHA-256 `2b539cf067b2ad48e1baa5093dbaa8818b73542d1222a0ffde9dc5f2922ea62e` |
| aarch64 | `7z2606-linux-arm64.tar.xz`, SHA-256 `edcc408aacce77da5223728f2eceb5e9f6dc713b8a9df934ad21e2a7f7560627` |
| Source (LGPL offer) | `7z2606-src.tar.xz`, SHA-256 `a4e374c8f864f51df48c2cfd0e1413ec3b2ea030deae852d6a655402ebb61ac4` |
| Base URL | https://github.com/ip7z/7zip/releases/download/26.06/ |

7-zip.org itself publishes no checksums. The sums above were computed from the downloaded files
(25.11.2026) and equal the `digest` GitHub reports for these release assets.

## Updating

1. Read the new release notes (security fixes) and pick the new version.
2. Download all three files, compute `sha256sum`, compare with the `digest` GitHub lists for the assets.
3. Change URL and SHA-256 in `.github/workflows/release.yml` (both matrix entries), this file and the
   section in `THIRD-PARTY-LICENSES.md` / `scripts/gen-third-party.py` together.
4. Never execute the downloaded file outside the build; check `License.txt` still has the unRAR restriction text.

## Checks that only a release build can show

- `release.yml`: step "Download 7-Zip for RAR" passes `sha256sum -c` on both runners (x86_64, aarch64).
- `scripts/build-appimage.sh` finishes: it fails when `usr/bin/7zz` or the license file is missing, when
  `7zz` is not statically linked, or when the tarball lacks `usr/bin/7zz`.
- On a machine without `unrar`/`7z`: open a RAR4 and a RAR5 archive from the AppImage (VM test).
- aarch64: the `7zzs` of the arm64 archive runs on the Raspberry Pi / arm64 VM.
