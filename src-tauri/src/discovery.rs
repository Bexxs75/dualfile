//! Opt-in Avahi discovery. No filesystem, listing or job-admission worker is used here.
use crate::{convert::CoreEventDto, state::EventSink};
use serde::Serialize;
use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv6Addr};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

const MAX_ENTRIES: usize = 64;
const MAX_SERVICES: usize = 128;
const SERVICE_TYPES: [&str; 5] = [
    "_smb._tcp",
    "_nfs._tcp",
    "_sftp-ssh._tcp",
    "_webdav._tcp",
    "_webdavs._tcp",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Service {
    pub key: String,
    pub name: String,
    pub protocol: String,
    pub host: String,
    pub port: u16,
    pub path: Option<String>,
}

fn invisible(c: char) -> bool {
    c.is_control()
        || matches!(c, '\u{ad}' | '\u{34f}' | '\u{61c}' | '\u{180b}'..='\u{180f}' | '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2060}'..='\u{206f}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{fff9}'..='\u{fffb}' | '\u{e0000}'..='\u{e01ef}')
}
fn dns_name(raw: &str) -> Option<String> {
    let name = raw.strip_suffix('.').unwrap_or(raw);
    if name.is_empty()
        || name.len() > 253
        || !name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        })
    {
        return None;
    }
    Some(name.to_ascii_lowercase())
}
fn host_name(raw: &str, address: &str, interface: i32) -> Option<String> {
    ip_address(raw, interface)
        .or_else(|| dns_name(raw))
        .or_else(|| ip_address(address, interface))
}
fn ip_address(raw: &str, interface: i32) -> Option<String> {
    if raw.len() > 254 {
        return None;
    }
    let raw = raw
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(raw);
    let (ip, scope) = raw
        .split_once('%')
        .map_or((raw, None), |(ip, scope)| (ip, Some(scope)));
    if let Ok(address) = ip.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(ip) if scope.is_none() => Some(ip.to_string()),
            IpAddr::V6(ip) => {
                let zone = match scope {
                    Some(s)
                        if !s.is_empty()
                            && s.len() <= 15
                            && s.bytes()
                                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')) =>
                    {
                        Some(s.to_owned())
                    }
                    Some(_) => return None,
                    None if link_local(ip) && interface > 0 => Some(interface.to_string()),
                    None if link_local(ip) => return None,
                    None => None,
                };
                Some(zone.map_or_else(|| ip.to_string(), |zone| format!("{ip}%{zone}")))
            }
            _ => None,
        };
    }
    None
}
fn link_local(ip: Ipv6Addr) -> bool {
    ip.segments()[0] & 0xffc0 == 0xfe80
}

// Never derive Debug for raw replies: TXT may contain credentials or other private data.
type Resolved = (
    i32,
    i32,
    String,
    String,
    String,
    String,
    i32,
    String,
    u16,
    Vec<Vec<u8>>,
    u32,
);
type Item = (i32, i32, String, String, String, u32);
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Identity {
    interface: i32,
    family: i32,
    name: String,
    kind: String,
    domain: String,
}
impl Identity {
    fn parse(item: Item) -> Option<Self> {
        let (interface, family, name, kind, domain, _) = item;
        if interface <= 0
            || ![0, 1].contains(&family)
            || name.is_empty()
            || name.len() > 63
            || name.chars().any(char::is_control)
            || !SERVICE_TYPES.contains(&kind.as_str())
        {
            return None;
        }
        Some(Self {
            interface,
            family,
            name,
            kind,
            domain: dns_name(&domain)?,
        })
    }
}
fn parse_service(reply: Resolved) -> Option<Service> {
    let (interface, family, name, kind, domain, host, _, address, port, txt, _) = reply;
    let identity = Identity::parse((interface, family, name, kind, domain, 0))?;
    if port == 0 || txt.len() > 32 || txt.iter().map(Vec::len).sum::<usize>() > 4096 {
        return None;
    }
    let name: String = identity.name.chars().filter(|c| !invisible(*c)).collect();
    if name.trim().is_empty() {
        return None;
    }
    let host = host_name(&host, &address, interface)?;
    let protocol = match identity.kind.as_str() {
        "_smb._tcp" => "smb",
        "_nfs._tcp" => "nfs",
        "_sftp-ssh._tcp" => "sftp",
        "_webdav._tcp" => "webdav",
        "_webdavs._tcp" => "webdavs",
        _ => return None,
    };
    let mut paths = BTreeMap::new();
    for record in txt {
        if record.len() > 255 {
            continue;
        }
        let Ok(record) = std::str::from_utf8(&record) else {
            continue;
        };
        if record.chars().any(invisible) {
            continue;
        }
        let Some((key, value)) = record.split_once('=') else {
            continue;
        };
        // No accounts, passwords, URLs, query strings or arbitrary TXT fields leave this parser.
        if !["path", "share"].contains(&key) {
            continue;
        }
        let value = value.trim();
        if value.is_empty()
            || value.contains(['\\', '?', '#', '@', '%', ':'])
            || value.split('/').any(|p| p == "." || p == "..")
        {
            continue;
        }
        paths
            .entry(key.to_owned())
            .or_insert_with(|| value.to_owned());
    }
    let path = if protocol == "smb" {
        paths
            .remove("share")
            .or_else(|| paths.remove("path"))
            .map(|s| s.trim_start_matches('/').to_owned())
    } else {
        paths.remove("path").map(|s| {
            if s.starts_with('/') {
                s
            } else {
                format!("/{s}")
            }
        })
    };
    let path = path.filter(|value| !value.is_empty());
    Some(Service {
        // Stable across app restarts so persisted hidden-entry keys keep working.
        key: format!("{host}|{protocol}|{port}"),
        name,
        protocol: protocol.into(),
        host,
        port,
        path,
    })
}

#[derive(Default)]
struct Cache {
    generation: u64,
    enabled: bool,
    stopped: bool,
    entries: Vec<Service>,
}
pub struct Discovery {
    cache: Arc<Mutex<Cache>>,
    changed: watch::Sender<u64>,
}
impl Default for Discovery {
    fn default() -> Self {
        Self {
            cache: Arc::default(),
            changed: watch::channel(0).0,
        }
    }
}
impl Discovery {
    pub fn set_enabled(&self, enabled: bool, sink: &dyn EventSink) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        if cache.enabled == enabled || cache.stopped {
            return;
        }
        cache.enabled = enabled;
        cache.generation += 1;
        cache.entries.clear();
        self.changed.send_replace(cache.generation);
        drop(cache);
        sink.emit(CoreEventDto::DiscoveryChanged);
    }
    pub fn snapshot(&self) -> Vec<Service> {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entries
            .clone()
    }
    pub fn stop(&self) {
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.stopped = true;
        cache.enabled = false;
        cache.entries.clear();
        cache.generation += 1;
        self.changed.send_replace(cache.generation);
    }
    pub fn start(&self, sink: Arc<dyn EventSink>) {
        #[cfg(target_os = "linux")]
        {
            let cache = self.cache.clone();
            let receiver = self.changed.subscribe();
            let _ = std::thread::Builder::new()
                .name("avahi-browse".into())
                .spawn(move || {
                    match tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                    {
                        Ok(runtime) => runtime.block_on(avahi::run(cache, receiver, sink)),
                        Err(_) => log::warn!("LAN discovery unavailable"),
                    }
                });
        }
        #[cfg(not(target_os = "linux"))]
        let _ = sink;
    }
}
fn publish(cache: &Mutex<Cache>, generation: u64, entries: Vec<Service>, sink: &dyn EventSink) {
    let mut cache = cache.lock().unwrap_or_else(|e| e.into_inner());
    if cache.enabled && !cache.stopped && cache.generation == generation && cache.entries != entries
    {
        cache.entries = entries;
        drop(cache);
        sink.emit(CoreEventDto::DiscoveryChanged);
    }
}
struct Pending {
    serial: u64,
    started: bool,
    service: Option<Service>,
}
#[derive(Default)]
struct Browse {
    items: BTreeMap<Identity, Pending>,
    serial: u64,
}
impl Browse {
    fn add(&mut self, identity: Identity) {
        if self.items.len() >= MAX_SERVICES || self.items.contains_key(&identity) {
            return;
        }
        self.serial += 1;
        self.items.insert(
            identity,
            Pending {
                serial: self.serial,
                started: false,
                service: None,
            },
        );
    }
    fn remove(&mut self, identity: &Identity) {
        self.items.remove(identity);
    }
    fn next(&mut self) -> Option<(Identity, u64)> {
        let (identity, pending) = self.items.iter_mut().find(|(_, p)| !p.started)?;
        pending.started = true;
        Some((identity.clone(), pending.serial))
    }
    fn resolved(&mut self, identity: &Identity, serial: u64, service: Option<Service>) {
        if let Some(pending) = self.items.get_mut(identity).filter(|p| p.serial == serial) {
            pending.service = service;
        }
    }
    fn snapshot(&self) -> Vec<Service> {
        let mut unique = BTreeMap::new();
        for service in self.items.values().filter_map(|p| p.service.as_ref()) {
            unique
                .entry(service.key.clone())
                .or_insert_with(|| service.clone());
        }
        unique.into_values().take(MAX_ENTRIES).collect()
    }
}

#[cfg(target_os = "linux")]
mod avahi {
    use super::*;
    use futures_util::{stream::FuturesUnordered, StreamExt};
    use std::time::Duration;
    use zbus::{message::Type, Connection, MatchRule, MessageStream};
    const AVAHI: &str = "org.freedesktop.Avahi";
    const SERVER: &str = "org.freedesktop.Avahi.Server";
    const BROWSER: &str = "org.freedesktop.Avahi.ServiceBrowser";
    const DEADLINE: Duration = Duration::from_secs(2);
    async fn bounded<T>(
        future: impl std::future::Future<Output = zbus::Result<T>>,
    ) -> Result<T, ()> {
        tokio::time::timeout(DEADLINE, future)
            .await
            .map_err(|_| ())?
            .map_err(|_| ())
    }
    pub(super) async fn run(
        cache: Arc<Mutex<Cache>>,
        changed: watch::Receiver<u64>,
        sink: Arc<dyn EventSink>,
    ) {
        let session_cache = cache.clone();
        let session_sink = sink.clone();
        run_sessions(
            cache,
            changed,
            sink,
            Duration::from_secs(10),
            move |generation| {
                let cache = session_cache.clone();
                let sink = session_sink.clone();
                async move { session(&cache, generation, sink.as_ref()).await }
            },
        )
        .await;
    }
    pub(super) async fn run_sessions<F: std::future::Future<Output = Result<(), ()>>>(
        cache: Arc<Mutex<Cache>>,
        mut changed: watch::Receiver<u64>,
        sink: Arc<dyn EventSink>,
        retry: Duration,
        mut browse: impl FnMut(u64) -> F,
    ) {
        let mut logged = false;
        loop {
            changed.borrow_and_update();
            let (generation, enabled, stopped) = {
                let cache = cache.lock().unwrap_or_else(|e| e.into_inner());
                (cache.generation, cache.enabled, cache.stopped)
            };
            if stopped {
                return;
            }
            if !enabled {
                if changed.changed().await.is_err() {
                    return;
                }
                continue;
            }
            tokio::select! {
                biased;
                result = changed.changed() => { if result.is_err() { return; } continue; }
                _ = browse(generation) => {}
            }
            publish(&cache, generation, vec![], sink.as_ref());
            if !logged {
                log::warn!("LAN discovery unavailable; Avahi or D-Bus did not respond");
                logged = true;
            }
            // A fixed retry interval also recovers after daemon and system-bus restarts.
            tokio::select! {
                result = changed.changed() => { if result.is_err() { return; } }
                _ = tokio::time::sleep(retry) => {}
            }
        }
    }
    async fn session(
        cache: &Mutex<Cache>,
        generation: u64,
        sink: &dyn EventSink,
    ) -> Result<(), ()> {
        let connection = bounded(Connection::system()).await?;
        // Subscribe before creating browsers; Avahi may emit cached ItemNew before the reply.
        let owners = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")
            .map_err(|_| ())?
            .interface("org.freedesktop.DBus")
            .map_err(|_| ())?
            .member("NameOwnerChanged")
            .map_err(|_| ())?
            .add_arg(AVAHI)
            .map_err(|_| ())?
            .build();
        let mut owners =
            bounded(MessageStream::for_match_rule(owners, &connection, Some(8))).await?;
        let owner_reply = bounded(connection.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "GetNameOwner",
            &(AVAHI,),
        ))
        .await?;
        let owner: String = owner_reply.body().deserialize().map_err(|_| ())?;
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(owner.as_str())
            .map_err(|_| ())?
            .interface(BROWSER)
            .map_err(|_| ())?
            .build();
        let mut signals =
            bounded(MessageStream::for_match_rule(rule, &connection, Some(256))).await?;
        let mut browsers = Vec::new();
        for kind in SERVICE_TYPES {
            let reply = bounded(connection.call_method(
                Some(owner.as_str()),
                "/",
                Some(SERVER),
                "ServiceBrowserNew",
                &(-1i32, -1i32, kind, "local", 0u32),
            ))
            .await?;
            let path: zbus::zvariant::OwnedObjectPath =
                reply.body().deserialize().map_err(|_| ())?;
            browsers.push(path);
        }
        let mut browse = Browse::default();
        let mut resolves = FuturesUnordered::new();
        let mut tick = tokio::time::interval(Duration::from_millis(300));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                // Cancellation is handled by run(). Owner changes invalidate the entire cache.
                _ = owners.next() => return Err(()),
                _ = connection.closed() => return Err(()),
                message = signals.next() => {
                    let message = message.ok_or(())?.map_err(|_| ())?;
                    if message.body().len() > 4096 { continue; }
                    let header = message.header();
                    if !browsers.iter().any(|p| header.path().is_some_and(|v| v.as_str() == p.as_str())) { continue; }
                    let member = header.member().map(|v| v.as_str()).unwrap_or("");
                    if member == "Failure" { return Err(()); }
                    if !["ItemNew", "ItemRemove"].contains(&member) { continue; }
                    let Some(identity) = message.body().deserialize::<Item>().ok().and_then(Identity::parse) else { continue; };
                    if member == "ItemNew" { browse.add(identity); } else { browse.remove(&identity); }
                }
                Some((identity, serial, service)) = resolves.next(), if !resolves.is_empty() => {
                    browse.resolved(&identity, serial, service);
                }
                _ = tick.tick() => {
                    publish(cache, generation, browse.snapshot(), sink);
                    // At most two outstanding requests and one new request per 300 ms. An item is
                    // resolved only once per presence, including failed/invalid replies.
                    if resolves.len() < 2 {
                        if let Some((identity, serial)) = browse.next() {
                            resolves.push(resolve(connection.clone(), owner.clone(), identity, serial));
                        }
                    }
                }
            }
        }
        // Dropping this dedicated connection frees its Avahi browser objects, including on cancel.
    }
    async fn resolve(
        connection: Connection,
        owner: String,
        identity: Identity,
        serial: u64,
    ) -> (Identity, u64, Option<Service>) {
        let args = (
            identity.interface,
            identity.family,
            identity.name.as_str(),
            identity.kind.as_str(),
            identity.domain.as_str(),
            -1i32,
            0u32,
        );
        let service = match bounded(connection.call_method(
            Some(owner.as_str()),
            "/",
            Some(SERVER),
            "ResolveService",
            &args,
        ))
        .await
        {
            Ok(reply) if reply.body().len() <= 16384 => reply
                .body()
                .deserialize::<Resolved>()
                .ok()
                .and_then(|reply| {
                    let response = Identity::parse((
                        reply.0,
                        reply.1,
                        reply.2.clone(),
                        reply.3.clone(),
                        reply.4.clone(),
                        0,
                    ))?;
                    if response != identity {
                        return None;
                    }
                    parse_service(reply)
                }),
            _ => None,
        };
        (identity, serial, service)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    #[derive(Default)]
    struct Sink(AtomicUsize);
    impl EventSink for Sink {
        fn emit(&self, event: CoreEventDto) {
            // PlacesChanged would send the frontend through places_snapshot -> discover().
            assert_eq!(event, CoreEventDto::DiscoveryChanged);
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    fn reply(kind: &str, txt: &[&str]) -> Resolved {
        (
            2,
            0,
            "Office NAS".into(),
            kind.into(),
            "local".into(),
            "NAS.local.".into(),
            0,
            "192.168.1.2".into(),
            445,
            txt.iter().map(|s| s.as_bytes().to_vec()).collect(),
            0,
        )
    }
    fn identity(name: &str, interface: i32) -> Identity {
        Identity::parse((
            interface,
            0,
            name.into(),
            SERVICE_TYPES[0].into(),
            "local".into(),
            0,
        ))
        .unwrap()
    }
    #[test]
    fn protocols_paths_and_credentials_allowlist() {
        for (kind, protocol, expected) in [
            ("_smb._tcp", "smb", "documents"),
            ("_nfs._tcp", "nfs", "/documents"),
            ("_sftp-ssh._tcp", "sftp", "/documents"),
            ("_webdav._tcp", "webdav", "/documents"),
            ("_webdavs._tcp", "webdavs", "/documents"),
        ] {
            let service = parse_service(reply(
                kind,
                &[
                    "path=documents",
                    "password=top-secret",
                    "user=alice",
                    "url=smb://alice:secret@host",
                ],
            ))
            .unwrap();
            assert_eq!(service.protocol, protocol);
            assert_eq!(service.host, "nas.local");
            assert_eq!(service.path.as_deref(), Some(expected));
            let public = format!("{service:?}{}", serde_json::to_string(&service).unwrap());
            for secret in ["top-secret", "alice", "password="] {
                assert!(!public.contains(secret));
            }
        }
        assert_eq!(
            parse_service(reply("_smb._tcp", &["path=wrong", "share=right"]))
                .unwrap()
                .path
                .as_deref(),
            Some("right")
        );
    }
    #[test]
    fn bounds_and_untrusted_text() {
        for host in [
            "bad/host",
            "user@host",
            "-option",
            "bad host",
            "host\n",
            "a..local",
            "a\u{202e}local",
        ] {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.5 = host.into();
            assert_eq!(parse_service(r).unwrap().host, "192.168.1.2", "{host:?}");
        }
        for name in [
            "".to_owned(),
            "x".repeat(64),
            "a\nb".into(),
            "\u{200b}".into(),
        ] {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.2 = name;
            assert!(parse_service(r).is_none());
        }
        let mut r = reply(SERVICE_TYPES[0], &[]);
        r.2 = "a\u{202e}\u{200b}\u{feff}b".into();
        assert_eq!(parse_service(r).unwrap().name, "ab");
        for txt in [vec![b"x=y".to_vec(); 33], vec![vec![b'x'; 255]; 17]] {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.9 = txt;
            assert!(parse_service(r).is_none());
        }
        for path in [
            "smb://user:pass@host",
            "../secret",
            "a/../b",
            "path?token=secret",
            "%0a",
            "a\\b",
        ] {
            assert!(
                parse_service(reply(SERVICE_TYPES[0], &[&format!("path={path}")]))
                    .unwrap()
                    .path
                    .is_none()
            );
        }
        let mut r = reply(SERVICE_TYPES[0], &[]);
        r.8 = 0;
        assert!(parse_service(r).is_none());
        let mut r = reply(SERVICE_TYPES[0], &[]);
        r.8 = u16::MAX;
        assert!(parse_service(r).is_some());
        assert!(parse_service(reply("_http._tcp", &[])).is_none());
        assert!(dns_name(&format!("{}.local", "x".repeat(64))).is_none());
        assert!(dns_name(&vec!["x".repeat(63); 5].join(".")).is_none());
    }
    #[test]
    fn invalid_txt_records_do_not_discard_services_or_valid_paths() {
        for record in [
            vec![0xff],
            vec![b'x'; 256],
            b"vendor=bad\0value".to_vec(),
            "vendor=a\u{200b}b".as_bytes().to_vec(),
            b"path=bad\0name".to_vec(),
            b"share=\xff".to_vec(),
            format!("path={}", "x".repeat(256)).into_bytes(),
            "path=a\u{202e}b".as_bytes().to_vec(),
        ] {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.9 = vec![record];
            assert!(parse_service(r.clone()).unwrap().path.is_none());
            r.9.push(b"share=documents".to_vec());
            assert_eq!(parse_service(r).unwrap().path.as_deref(), Some("documents"));
        }
    }
    #[test]
    fn txt_paths_trim_surrounding_whitespace_before_validation() {
        for (kind, txt, expected) in [
            ("_smb._tcp", "share=  /documents  ", Some("documents")),
            ("_smb._tcp", "path=  documents  ", Some("documents")),
            ("_nfs._tcp", "path=  /documents  ", Some("/documents")),
            (
                "_webdav._tcp",
                "path=\u{a0}documents\u{a0}",
                Some("/documents"),
            ),
            ("_smb._tcp", "share=   ", None),
            ("_smb._tcp", "share= / ", None),
            ("_nfs._tcp", "path=  ../secret  ", None),
            ("_nfs._tcp", "path=\tdocuments\t", None),
        ] {
            assert_eq!(
                parse_service(reply(kind, &[txt])).unwrap().path.as_deref(),
                expected
            );
        }
    }
    #[test]
    fn non_dns_hosts_fall_back_only_to_validated_addresses() {
        for host in ["WINDOWS_NAS.local", "embedded device", "", &"x".repeat(255)] {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.5 = host.into();
            assert_eq!(parse_service(r.clone()).unwrap().host, "192.168.1.2");
            r.7 = "fe80::2".into();
            assert_eq!(parse_service(r.clone()).unwrap().host, "fe80::2%2");
            for address in [
                "bad/host",
                "other.local",
                "192.0.2.1%2",
                "fe80::2%bad/zone",
                "",
            ] {
                r.7 = address.into();
                assert!(parse_service(r.clone()).is_none());
            }
        }
    }
    #[test]
    fn ipv4_ipv6_and_link_local_scope() {
        for (raw, expected) in [
            ("192.0.2.3", "192.0.2.3"),
            ("2001:DB8::1", "2001:db8::1"),
            ("[fe80::1]", "fe80::1%2"),
            ("fe80::1%eth0", "fe80::1%eth0"),
        ] {
            assert_eq!(host_name(raw, "", 2).as_deref(), Some(expected));
        }
        assert_eq!(host_name("", "fe80::2", 3).as_deref(), Some("fe80::2%3"));
        assert!(host_name("fe80::1", "", -1).is_none());
        assert!(host_name("fe80::1%bad/zone", "", 2).is_none());
        assert!(host_name("192.0.2.3%2", "", 2).is_none());
    }
    #[test]
    fn duplicate_announcements_remove_and_late_resolution() {
        let mut browse = Browse::default();
        let a = identity("one", 2);
        let b = identity("two", 3);
        browse.add(a.clone());
        browse.add(a.clone());
        let (_, serial) = browse.next().unwrap();
        assert!(browse.next().is_none());
        let service = parse_service(reply(SERVICE_TYPES[0], &[])).unwrap();
        browse.resolved(&a, serial, Some(service.clone()));
        browse.add(b.clone());
        let (_, other) = browse.next().unwrap();
        browse.resolved(&b, other, Some(service.clone()));
        assert_eq!(browse.snapshot(), vec![service.clone()]);
        browse.remove(&a);
        assert_eq!(browse.snapshot().len(), 1);
        browse.remove(&b);
        assert!(browse.snapshot().is_empty());
        browse.add(a.clone());
        browse.resolved(&a, serial, Some(service));
        assert!(browse.snapshot().is_empty());
    }
    #[test]
    fn failed_resolves_are_not_retried_and_both_caps_hold() {
        let mut browse = Browse::default();
        for n in 0..200 {
            browse.add(identity(&format!("host-{n}"), 2));
        }
        assert_eq!(browse.items.len(), MAX_SERVICES);
        while let Some((id, serial)) = browse.next() {
            let mut r = reply(SERVICE_TYPES[0], &[]);
            r.5 = format!("{}.local", id.name);
            browse.resolved(
                &id,
                serial,
                if serial == 1 { None } else { parse_service(r) },
            );
        }
        assert_eq!(browse.snapshot().len(), MAX_ENTRIES);
        assert!(browse.next().is_none());
    }
    #[test]
    fn opt_in_disable_and_restart_invalidate_cache_and_emit_changes() {
        let d = Discovery::default();
        let sink = Sink::default();
        let service = parse_service(reply(SERVICE_TYPES[0], &[])).unwrap();
        publish(&d.cache, 0, vec![service.clone()], &sink);
        assert!(d.snapshot().is_empty());
        d.set_enabled(true, &sink);
        publish(&d.cache, 1, vec![service.clone()], &sink);
        publish(&d.cache, 1, vec![service.clone()], &sink);
        assert_eq!(sink.0.load(Ordering::SeqCst), 2);
        // Owner loss clears all entries; a new browser starts from an empty set.
        publish(&d.cache, 1, vec![], &sink);
        assert!(d.snapshot().is_empty());
        publish(&d.cache, 1, vec![service.clone()], &sink);
        d.set_enabled(false, &sink);
        assert!(d.snapshot().is_empty());
        d.set_enabled(true, &sink);
        publish(&d.cache, 1, vec![service], &sink);
        assert!(d.snapshot().is_empty());
        d.stop();
        assert!(d.snapshot().is_empty());
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn missing_avahi_retries_without_blocking_and_disable_cancels_browse() {
        use std::time::Duration;
        struct Active(Arc<AtomicUsize>);
        impl Drop for Active {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let d = Discovery::default();
        let sink = Arc::new(Sink::default());
        let (started, mut starts) = tokio::sync::mpsc::unbounded_channel();
        let active = Arc::new(AtomicUsize::new(0));
        let active_worker = active.clone();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_worker = attempts.clone();
        let worker = tokio::spawn(avahi::run_sessions(
            d.cache.clone(),
            d.changed.subscribe(),
            sink.clone(),
            Duration::from_millis(10),
            move |_| {
                let started = started.clone();
                let active = active_worker.clone();
                let attempts = attempts_worker.clone();
                async move {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst);
                    if attempt == 0 {
                        started.send(false).unwrap();
                        return Err(());
                    }
                    active.fetch_add(1, Ordering::SeqCst);
                    let _active = Active(active);
                    started.send(true).unwrap();
                    std::future::pending::<Result<(), ()>>().await
                }
            },
        ));
        tokio::task::yield_now().await;
        assert_eq!(attempts.load(Ordering::SeqCst), 0);
        d.set_enabled(true, sink.as_ref());
        assert!(!tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap());
        assert!(d.snapshot().is_empty());
        assert!(tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap());
        assert_eq!(active.load(Ordering::SeqCst), 1);
        d.set_enabled(false, sink.as_ref());
        tokio::task::yield_now().await;
        assert_eq!(active.load(Ordering::SeqCst), 0);
        d.set_enabled(true, sink.as_ref());
        assert!(tokio::time::timeout(Duration::from_secs(1), starts.recv())
            .await
            .unwrap()
            .unwrap());
        d.stop();
        tokio::time::timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
