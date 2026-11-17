# Fix round 1 for sidebar step 4

Step 4 is already built and committed (WIP commit HEAD, base work order docs/plans/sidebar-step4-workorder.md, which still applies). Only fix the items below on top of it.

- SB4-R1-01 MEDIUM discovery.rs:275-283 publish emits CoreEventDto::PlacesChanged (300 ms tick :486-487) → frontend calls places_snapshot (places.rs:326-351) → state.refresh() → discover() (lsblk, mountinfo, fstab, GVfs, statvfs up to 1.5 s). While Avahi resolves ~40 services this triggers dozens of full refreshes; violates "discovery must not slow places refresh". FIX: separate DiscoveryChanged event + cheap discovery_snapshot command (mutex read only), frontend updates only the discovery section; places refresh unchanged. Test: a discovery publish never calls discover().
- SB4-R1-02 MEDIUM(PLAUSIBLE) discovery.rs:157-164 one TXT record that is non-UTF-8, >255 bytes or has invisible/control chars returns None for the whole service; TXT may legally be binary (RFC 6763). FIX: skip only the offending record; reject only a bad path/share value (service then shown without path). Keep the 32-record / 4 KiB caps. Test.
- SB4-R1-03 LOW host_name (:52-88): non-DNS host names (underscore, Windows/embedded) drop the service → fall back to the validated resolved address. Test.
- SB4-R1-04 LOW at the 128-identity cap new ItemNew are dropped for good (Avahi does not re-announce) → document in wiki DE+EN (toggle the setting to rescan), or keep a small backlog.
- SB4-R1-05 LOW zbus 5.19 with async-io also pulled in: features=["tokio"] is effectively a no-op; tokio "net" looks unneeded; a connection thread is created/dropped every 10 s while Avahi is missing. FIX: drop unneeded features (keep only what compiles and is used), one-line comment on the runtime choice.
- SB4-R1-06 LOW trim surrounding whitespace in TXT path/share values. Test.
- No report file; reply only.
