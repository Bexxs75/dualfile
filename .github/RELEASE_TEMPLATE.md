## Highlights

<!-- Replace with verified changes for this version before publication. -->

- Pending release review. See [CHANGELOG](https://github.com/Bexxs75/dualfile/blob/main/CHANGELOG.md).

## Download

| Platform | Asset | Notes |
|---|---|---|
| Linux x86_64 | `Dualfile-VERSION-x86_64.AppImage` | Make executable and run; FUSE or extraction may be required. |

Replace VERSION and verify the attached filenames before publishing.

## Checksums

Download `SHA256SUMS` and the desired assets into the same directory:

```sh
sha256sum --check --ignore-missing SHA256SUMS
```

A checksum detects changes against this release's checksum file; it is not a publisher signature.

## Updates

The native updater is prepared; its UI/startup integration remains incomplete in this checkout.
The workflow requires a signing key and generates `latest.json` after signing the final repacked AppImage.
Do not reuse the signature of a different or pre-repack build. Manual downloads remain available.

## Known limitations

- Development snapshot; no distribution-wide compatibility guarantee.
- Signing is configured; verify the actual signed artifact and native updater before publication.
- Complete the dependency-license review and include required notices before distribution.
- Record version-specific limitations and validation results here before publication.
