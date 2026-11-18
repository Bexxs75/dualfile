//! Read-only, bounded discovery of desktop network locations. No credential stores.
use crate::connections::{ConnectionProfile, ConnectionProtocol, ConnectionTls, SshTransport};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemPlace {
    pub sources: Vec<String>,
    pub name: String,
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub user: Option<String>,
    pub path: String,
    pub url: String,
    pub mounted_at: Option<PathBuf>,
}

fn decode(text: &str) -> Option<String> {
    let mut bytes = Vec::new();
    let mut iter = text.bytes();
    while let Some(b) = iter.next() {
        bytes.push(if b == b'%' {
            let a = (iter.next()? as char).to_digit(16)?;
            let b = (iter.next()? as char).to_digit(16)?;
            (a * 16 + b) as u8
        } else {
            b
        });
    }
    let value = String::from_utf8(bytes).ok()?;
    (!value.chars().any(char::is_control)).then_some(value)
}
fn encode(text: &str, slash: bool) -> String {
    text.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (slash && b == b'/') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
/// Parse before constructing any displayable value. Userinfo passwords are discarded.
pub fn parse_url(url: &str, source: &str, name: &str) -> Option<SystemPlace> {
    if url.len() > 8192 || url.chars().any(char::is_control) {
        return None;
    }
    let (scheme, rest) = url.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if ![
        "sftp", "fish", "ssh", "ftp", "ftps", "smb", "nfs", "webdav", "webdavs", "dav", "davs",
        "afp",
    ]
    .contains(&scheme.as_str())
    {
        return None;
    }
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if authority.contains(['?', '#']) || path.contains(['?', '#']) {
        return None;
    }
    let (user, server) = match authority.rsplit_once('@') {
        Some((info, host)) => (Some(decode(info.split(':').next()?)?), host),
        None => (None, authority),
    };
    let (host, port) = if server.starts_with('[') {
        let (host, tail) = server.split_once(']')?;
        (
            format!("{host}]"),
            if tail.is_empty() {
                None
            } else {
                Some(tail.strip_prefix(':')?.parse::<u16>().ok()?)
            },
        )
    } else if let Some((host, port)) = server.split_once(':') {
        (host.to_lowercase(), Some(port.parse::<u16>().ok()?))
    } else {
        (server.to_lowercase(), None)
    };
    if host.is_empty()
        || host.len() > 253
        || host.starts_with('-')
        || !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-_:[]".contains(&b))
        || port == Some(0)
    {
        return None;
    }
    let path = decode(&format!("/{path}"))?;
    if path.split('/').any(|p| p == ".." || p == ".") {
        return None;
    }
    let userinfo = user
        .as_ref()
        .filter(|s| !s.is_empty())
        .map(|u| format!("{}@", encode(u, false)))
        .unwrap_or_default();
    let port_text = port.map(|p| format!(":{p}")).unwrap_or_default();
    let clean = format!(
        "{scheme}://{userinfo}{host}{port_text}{}",
        encode(&path, true)
    );
    // Some desktop applications use the original URL as title. Never return that title.
    let name = if name.contains("://") || name.trim().is_empty() {
        host.clone()
    } else {
        name.chars().filter(|c| !c.is_control()).take(90).collect()
    };
    Some(SystemPlace {
        sources: vec![source.into()],
        name,
        scheme,
        host,
        port,
        user,
        path,
        url: clean,
        mounted_at: None,
    })
}

impl SystemPlace {
    pub fn profile(&self) -> ConnectionProfile {
        let protocol = match self.scheme.as_str() {
            "ssh" | "fish" | "sftp" => ConnectionProtocol::Sftp,
            "ftp" => ConnectionProtocol::Ftp,
            "ftps" => ConnectionProtocol::Ftps,
            "smb" => ConnectionProtocol::Smb,
            _ => ConnectionProtocol::System,
        };
        ConnectionProfile {
            name: self.name.clone(),
            protocol,
            transport: if protocol == ConnectionProtocol::Sftp {
                SshTransport::Openssh
            } else {
                SshTransport::Builtin
            },
            host: self.host.clone(),
            port: self.port.unwrap_or(match protocol {
                ConnectionProtocol::Sftp => 0,
                ConnectionProtocol::System => 0,
                ConnectionProtocol::Smb => 445,
                _ => 21,
            }),
            user: self.user.clone().unwrap_or_else(|| {
                if matches!(protocol, ConnectionProtocol::Ftp | ConnectionProtocol::Ftps) {
                    "anonymous".into()
                } else {
                    String::new()
                }
            }),
            share: if protocol == ConnectionProtocol::Smb {
                self.path
                    .trim_start_matches('/')
                    .split('/')
                    .next()
                    .unwrap_or("")
                    .into()
            } else {
                String::new()
            },
            start_folder: Some(if protocol == ConnectionProtocol::Smb {
                self.path
                    .trim_start_matches('/')
                    .split_once('/')
                    .map(|(_, rest)| format!("/{rest}"))
                    .unwrap_or_else(|| "/".into())
            } else {
                self.path.clone()
            }),
            tls: if protocol == ConnectionProtocol::Ftps {
                ConnectionTls::Explicit
            } else {
                ConnectionTls::None
            },
            system_url: (protocol == ConnectionProtocol::System).then(|| self.url.clone()),
            ..ConnectionProfile::default()
        }
    }
    pub fn matches(&self, profile: &ConnectionProfile) -> bool {
        let p = self.profile();
        let port = |p: &ConnectionProfile| {
            if p.port == 0 && p.protocol == ConnectionProtocol::Sftp {
                22
            } else {
                p.port
            }
        };
        p.protocol == profile.protocol
            && p.host.eq_ignore_ascii_case(&profile.host)
            && port(&p) == port(profile)
            && p.user == profile.user
            && p.share == profile.share
            && p.start_folder.as_deref().unwrap_or("/")
                == profile.start_folder.as_deref().unwrap_or("/")
            && p.system_url == profile.system_url
    }
}

/// Build the entire import before persistence: a stale selection never partially saves.
pub fn import_places(
    existing: &[ConnectionProfile],
    places: &[SystemPlace],
    urls: &[String],
) -> Result<Vec<ConnectionProfile>, &'static str> {
    use crate::connections::{name_key, new_profile_id, validate_connections};
    if urls.len() > 200 {
        return Err("Zu viele Netzwerkorte");
    }
    let mut profiles = existing.to_vec();
    for url in urls {
        let place = places
            .iter()
            .find(|p| &p.url == url)
            .ok_or("Netzwerkort nicht mehr verfügbar")?;
        if profiles.iter().any(|p| place.matches(p)) {
            continue;
        }
        let mut profile = place.profile();
        profile.id = new_profile_id(&profiles);
        let base = profile.name.clone();
        let mut suffix = 2;
        while profiles
            .iter()
            .any(|p| name_key(&p.name) == name_key(&profile.name))
        {
            profile.name = format!("{base} ({suffix})");
            suffix += 1;
        }
        profiles.push(profile);
    }
    validate_connections(&profiles)?;
    Ok(profiles)
}

#[derive(Clone, Debug)]
pub struct DiscoveryPaths {
    pub home: PathBuf,
    pub data: PathBuf,
    pub config: PathBuf,
    pub runtime: PathBuf,
}
impl DiscoveryPaths {
    pub fn system() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/nonexistent"));
        let xdg = |key, fallback| {
            std::env::var_os(key)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or(fallback)
        };
        Self {
            data: xdg("XDG_DATA_HOME", home.join(".local/share")),
            config: xdg("XDG_CONFIG_HOME", home.join(".config")),
            runtime: PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw())),
            home,
        }
    }
}
fn read(path: &Path) -> String {
    use std::os::unix::fs::OpenOptionsExt;
    let Ok(file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW)
        .open(path)
    else {
        return String::new();
    };
    if !file
        .metadata()
        .is_ok_and(|m| m.is_file() && m.len() <= 262144)
    {
        return String::new();
    }
    let mut out = String::new();
    if file.take(262145).read_to_string(&mut out).is_err() || out.len() > 262144 {
        return String::new();
    }
    out
}
fn dirs(path: &Path) -> Vec<PathBuf> {
    let mut paths: Vec<_> = fs::read_dir(path)
        .into_iter()
        .flatten()
        .take(4096)
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths.truncate(200);
    paths
}
fn xml(text: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = text;
    while let Some((before, tail)) = rest.split_once('&') {
        out.push_str(before);
        let (entity, after) = tail.split_once(';')?;
        out.push(match entity {
            "amp" => '&',
            "quot" => '"',
            "apos" => '\'',
            "lt" => '<',
            "gt" => '>',
            _ => {
                let number = if let Some(hex) = entity.strip_prefix("#x") {
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    entity.strip_prefix('#')?.parse().ok()?
                };
                char::from_u32(number)?
            }
        });
        rest = after;
    }
    out.push_str(rest);
    Some(out)
}
fn push(out: &mut Vec<SystemPlace>, place: Option<SystemPlace>) {
    let Some(place) = place else {
        return;
    };
    if let Some(old) = out.iter_mut().find(|p| p.url == place.url) {
        for s in place.sources {
            if !old.sources.contains(&s) {
                old.sources.push(s);
            }
        }
        if place.mounted_at.is_some() {
            old.mounted_at = place.mounted_at;
        }
    } else if out.len() < 200 {
        out.push(place);
    }
}
/// Network locations the desktop remembers (Dolphin network folders and places, GTK
/// bookmarks), whether or not they are mounted. Reads small local files only and never
/// enters a mount, so it is safe to call on every sidebar refresh.
pub fn bookmarks(paths: &DiscoveryPaths) -> Vec<SystemPlace> {
    let mut out = Vec::new();
    for path in dirs(&paths.data.join("remoteview")) {
        if path.extension().is_none_or(|s| s != "desktop") {
            continue;
        }
        let text = read(&path);
        let mut section = false;
        let mut name = "";
        let mut url = "";
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                section = line == "[Desktop Entry]";
            }
            if section {
                if let Some(v) = line.strip_prefix("Name=") {
                    name = v;
                }
                if let Some(v) = line.strip_prefix("URL=") {
                    url = v;
                }
            }
        }
        push(
            &mut out,
            parse_url(
                url,
                "dolphin",
                &name.replace("\\s", " ").replace("\\\\", "\\"),
            ),
        );
    }
    let text = read(&paths.data.join("user-places.xbel"));
    // Bounded subset of XBEL; no DTD/entity expansion or external resources.
    let bookmarks = regex::Regex::new(r#"(?s)<bookmark\s+([^>]+)>(.*?)</bookmark\s*>"#).unwrap();
    let href = regex::Regex::new(r#"\bhref\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap();
    let title = regex::Regex::new(r#"(?s)<title\s*>(.*?)</title\s*>"#).unwrap();
    let hidden = regex::Regex::new(r#"(?is)<(?:\w+:)?IsHidden\b[^>]*>\s*(?:true|1)\s*</"#).unwrap();
    for b in bookmarks.captures_iter(&text).take(200) {
        if hidden.is_match(&b[2]) {
            continue;
        }
        let Some(h) = href.captures(&b[1]) else {
            continue;
        };
        let url = xml(h.get(1).or_else(|| h.get(2)).unwrap().as_str()).unwrap_or_default();
        let name = title
            .captures(&b[2])
            .and_then(|t| xml(&t[1]))
            .unwrap_or_default();
        push(&mut out, parse_url(&url, "dolphin", &name));
    }
    for file in [
        paths.config.join("gtk-3.0/bookmarks"),
        paths.home.join(".gtk-bookmarks"),
    ] {
        for line in read(&file).lines().take(1000) {
            let (url, name) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
            push(&mut out, parse_url(url, "gnome", name.trim()));
        }
    }
    out
}
/// Bookmarks for the sidebar: a location that is already mounted is listed there as a
/// mounted place, so its bookmark is dropped instead of appearing a second time. Uses the
/// same authority and path rules as [`matching_mount`] and touches no filesystem.
pub fn unmounted_bookmarks(
    bookmarks: Vec<SystemPlace>,
    mounted: &[SystemPlace],
) -> Vec<SystemPlace> {
    bookmarks
        .into_iter()
        .filter(|place| matching_mount(place, mounted).is_none())
        .collect()
}
pub fn discover(paths: &DiscoveryPaths) -> Vec<SystemPlace> {
    let mut out = bookmarks(paths);
    let mounted = mounts(paths);
    for place in &mut out {
        if let Some(path) = match_mount(place, &mounted, &|p| p.is_dir()) {
            place.mounted_at = Some(path);
            if !place.sources.iter().any(|s| s == "mounted") {
                place.sources.push("mounted".into());
            }
        }
    }
    for place in mounted {
        push(&mut out, Some(place));
    }
    out
}
pub fn mounts(paths: &DiscoveryPaths) -> Vec<SystemPlace> {
    mounts_with(paths, &dirs, &|path| path.is_dir())
}
/// Each directory read and existence probe owns a separate bounded slot. A path still
/// executing after timeout is skipped until it returns; subsequent refreshes cannot pile up.
pub fn mounts_bounded(
    paths: &DiscoveryPaths,
    pool: &crate::places_pool::Pool,
    budget: std::time::Duration,
) -> Vec<SystemPlace> {
    probe_mounts(paths, pool, budget).0
}
/// The completion flag prevents a timed-out probe from being treated as an unmount.
pub fn probe_mounts(
    paths: &DiscoveryPaths,
    pool: &crate::places_pool::Pool,
    budget: std::time::Duration,
) -> (Vec<SystemPlace>, bool) {
    let complete = std::cell::Cell::new(true);
    let until = std::time::Instant::now() + budget;
    let deadline = || {
        until
            .saturating_duration_since(std::time::Instant::now())
            .min(std::time::Duration::from_millis(100))
    };
    let mounts = mounts_with(
        paths,
        &|path| {
            let p = path.to_owned();
            let result = pool.run_keyed(p.to_string_lossy().into_owned(), deadline(), move || {
                dirs(&p)
            });
            if result.is_none() {
                complete.set(false);
            }
            result.unwrap_or_default()
        },
        &|path| {
            let p = path.to_owned();
            let result = pool.run_keyed(p.to_string_lossy().into_owned(), deadline(), move || {
                p.is_dir()
            });
            if result.is_none() {
                complete.set(false);
            }
            result.unwrap_or(false)
        },
    );
    (mounts, complete.get())
}
fn mounts_with(
    paths: &DiscoveryPaths,
    dirs: &impl Fn(&Path) -> Vec<PathBuf>,
    is_dir: &impl Fn(&Path) -> bool,
) -> Vec<SystemPlace> {
    let mut out = Vec::new();
    for path in dirs(&paths.runtime.join("gvfs")) {
        if !is_dir(&path) {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some((kind, params)) = name.split_once(':') else {
            continue;
        };
        let fields: std::collections::BTreeMap<_, _> = params
            .split(',')
            .filter_map(|p| p.split_once('='))
            .collect();
        if fields.keys().any(|k| matches!(*k, "password" | "pass")) {
            continue;
        }
        let get = |key| fields.get(key).copied().unwrap_or("");
        if decode(get("user")).is_none_or(|u| u.contains([':', '@'])) {
            continue;
        }
        let scheme = match kind {
            "smb-share" => "smb",
            "dav" if get("ssl") == "true" => "davs",
            _ => kind,
        };
        let host = if get("host").is_empty() {
            get("server")
        } else {
            get("host")
        };
        let user = if get("user").is_empty() {
            String::new()
        } else {
            format!("{}@", get("user"))
        };
        let port = if get("port").is_empty() {
            String::new()
        } else {
            format!(":{}", get("port"))
        };
        let remote_path = if kind == "dav" {
            get("prefix").trim_start_matches('/')
        } else {
            get("share")
        };
        let decoded_path = decode(remote_path).unwrap_or_default();
        let remote_path = encode(decoded_path.trim_start_matches('/'), true);
        let url = format!("{scheme}://{user}{host}{port}/{remote_path}");
        if let Some(mut p) = parse_url(&url, "mounted", "") {
            p.mounted_at = Some(path);
            push(&mut out, Some(p));
        }
    }
    for root in dirs(&paths.runtime).into_iter().filter(|p| {
        p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("kio-fuse-"))
    }) {
        for scheme in dirs(&root) {
            for host in dirs(&scheme) {
                let authority = host.file_name().unwrap().to_string_lossy();
                if authority
                    .rsplit_once('@')
                    .is_some_and(|(info, _)| info.contains(':'))
                {
                    continue;
                }
                let url = format!(
                    "{}://{}/",
                    scheme.file_name().unwrap().to_string_lossy(),
                    host.file_name().unwrap().to_string_lossy()
                );
                if is_dir(&host) {
                    if let Some(mut p) = parse_url(&url, "mounted", "") {
                        p.mounted_at = Some(host);
                        push(&mut out, Some(p));
                    }
                }
            }
        }
    }
    out
}
/// Resolve descendants of a mount without trusting a persisted local path.
pub fn mounted_path(place: &SystemPlace, paths: &DiscoveryPaths) -> Option<PathBuf> {
    match_mount(place, &mounts(paths), &|p| p.is_dir())
}
pub fn mounted_path_bounded(place: &SystemPlace, paths: &DiscoveryPaths) -> Option<PathBuf> {
    let pool = crate::places_pool::mount_checks();
    let mounted = mounts_bounded(paths, pool, std::time::Duration::from_millis(500));
    match_mount(place, &mounted, &|path| {
        let p = path.to_owned();
        pool.run_keyed(
            p.to_string_lossy().into_owned(),
            std::time::Duration::from_millis(100),
            move || p.is_dir(),
        )
        .unwrap_or(false)
    })
}
/// Compare parsed authorities using the same rules as mounted-path discovery.
fn same_server(p: &SystemPlace, place: &SystemPlace) -> bool {
    let scheme = |p: &SystemPlace| match p.scheme.as_str() {
        "webdav" | "dav" => "dav",
        "webdavs" | "davs" => "davs",
        "fish" | "ssh" | "sftp" => "sftp",
        _ => "other",
    };
    let port = |p: &SystemPlace| {
        p.port.unwrap_or(match p.scheme.as_str() {
            "sftp" | "ssh" | "fish" => 22,
            "dav" | "webdav" => 80,
            "davs" | "webdavs" => 443,
            "smb" => 445,
            "ftp" | "ftps" => 21,
            _ => 0,
        })
    };
    (p.scheme == place.scheme || (scheme(p) != "other" && scheme(p) == scheme(place)))
        && p.host.eq_ignore_ascii_case(&place.host)
        && p.user == place.user
        && port(p) == port(place)
}
/// Locate the mount containing a profile path, without touching that filesystem.
pub fn matching_mount<'a>(
    place: &SystemPlace,
    mounted: &'a [SystemPlace],
) -> Option<&'a SystemPlace> {
    mounted
        .iter()
        .filter(|p| same_server(p, place))
        .filter(|p| {
            let root = p.path.trim_end_matches('/');
            place
                .path
                .strip_prefix(root)
                .is_some_and(|s| s.is_empty() || s.starts_with('/'))
        })
        .max_by_key(|p| p.path.len())
}

fn match_mount(
    place: &SystemPlace,
    mounted: &[SystemPlace],
    is_dir: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    mounted
        .iter()
        .filter(|p| same_server(p, place))
        .filter_map(|p| {
            let root = p.path.trim_end_matches('/');
            let suffix = place.path.strip_prefix(root)?;
            if !suffix.is_empty() && !suffix.starts_with('/') {
                return None;
            }
            Some(p.mounted_at.as_ref()?.join(suffix.trim_start_matches('/')))
        })
        .find(|p| is_dir(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (tempfile::TempDir, DiscoveryPaths) {
        let tmp = tempfile::tempdir().unwrap();
        let p = DiscoveryPaths {
            home: tmp.path().join("home"),
            data: tmp.path().join("data"),
            config: tmp.path().join("config"),
            runtime: tmp.path().join("run"),
        };
        for dir in [
            &p.home,
            &p.data.join("remoteview"),
            &p.config.join("gtk-3.0"),
            &p.runtime.join("gvfs"),
        ] {
            fs::create_dir_all(dir).unwrap();
        }
        (tmp, p)
    }
    #[test]
    fn profile_mount_matching_normalizes_authority_and_path() {
        let mounted = vec![parse_url("sftp://alice@HOST/share/", "", "").unwrap()];
        for url in [
            "ssh://alice@host:22/share",
            "sftp://alice:password@Host/share/",
            "sftp://alice@host/share/sub",
        ] {
            assert!(matching_mount(&parse_url(url, "", "").unwrap(), &mounted).is_some());
        }
        for (scheme, port) in [
            ("smb", 445),
            ("dav", 80),
            ("davs", 443),
            ("ftp", 21),
            ("ftps", 21),
        ] {
            let mounted = vec![parse_url(&format!("{scheme}://alice@host/share"), "", "").unwrap()];
            let profile =
                parse_url(&format!("{scheme}://alice@HOST:{port}/share/"), "", "").unwrap();
            assert!(matching_mount(&profile, &mounted).is_some());
        }
        for url in [
            "sftp://other@host/share",
            "sftp://alice@host:23/share",
            "sftp://alice@host/shares",
        ] {
            assert!(matching_mount(&parse_url(url, "", "").unwrap(), &mounted).is_none());
        }
    }
    #[test]
    fn blocked_gvfs_probe_does_not_hide_kio_and_recovers_after_answer() {
        let (_tmp, paths) = fixture();
        let gvfs = paths.runtime.join("gvfs");
        fs::create_dir_all(gvfs.join("smb-share:server=nas,share=team")).unwrap();
        fs::create_dir_all(paths.runtime.join("kio-fuse-test/sftp/host")).unwrap();
        let pool = crate::places_pool::Pool::new(2);
        let (release, blocked) = std::sync::mpsc::channel();
        pool.run_keyed(
            gvfs.to_string_lossy().into_owned(),
            std::time::Duration::from_millis(20),
            move || blocked.recv().unwrap(),
        );
        let (mounts, complete) = probe_mounts(&paths, &pool, std::time::Duration::from_secs(1));
        assert!(!complete);
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].host, "host");
        release.send(()).unwrap();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(1);
        loop {
            let (mounts, complete) = probe_mounts(&paths, &pool, std::time::Duration::from_secs(1));
            if complete {
                assert_eq!(mounts.len(), 2);
                break;
            }
            assert!(std::time::Instant::now() < until);
        }
    }
    #[test]
    fn smb_import_splits_share_and_folder_and_discards_password() {
        let p = parse_url("smb://alice:secret@nas/team/folder", "gnome", "NAS")
            .unwrap()
            .profile();
        assert_eq!(p.protocol, ConnectionProtocol::Smb);
        assert_eq!(
            (&*p.host, &*p.user, &*p.share, p.port),
            ("nas", "alice", "team", 445)
        );
        assert_eq!(p.start_folder.as_deref(), Some("/folder"));
        assert!(!p.guest && !p.save_secret && p.system_url.is_none());
        assert!(!serde_json::to_string(&p).unwrap().contains("alice:secret"));
    }

    #[test]
    fn passwords_never_reach_serialization_debug_profiles_or_url_titles() {
        for url in [
            "sftp://user:supersecret@host/a%20b",
            "smb://user:supersecret@host/share",
            "ftp://user:supersecret@host/",
        ] {
            let p = parse_url(url, "gnome", url).unwrap();
            assert!(!format!(
                "{p:?} {:?} {}",
                p.profile(),
                serde_json::to_string(&p).unwrap()
            )
            .contains("supersecret"));
            assert!(!p.url.contains(":supersecret"));
        }
        assert_eq!(
            parse_url("sftp://u:p@host:2222/%C3%A4", "", "Ä Ö")
                .unwrap()
                .path,
            "/ä"
        );
        for url in [
            "file:///tmp",
            "broken",
            "smb://host/../x",
            "smb://host/%2e%2e/x",
            "smb://host/%00",
            "sftp://-bad/",
            "ftp://host:0/",
        ] {
            assert!(parse_url(url, "", "").is_none());
        }
    }
    #[test]
    fn real_formats_hidden_entries_missing_corrupt_and_duplicates() {
        let (_tmp, p) = fixture();
        fs::write(
            p.data.join("remoteview/a.desktop"),
            "[Desktop Entry]\nName=Büro Server\nURL=sftp://u:secret@host/a%20b\n",
        )
        .unwrap();
        fs::write(p.data.join("remoteview/b.desktop"), "broken").unwrap();
        fs::write(p.data.join("user-places.xbel"), r#"<?xml version="1.0"?><xbel><bookmark href="sftp://u:secret@host/a%20b"><title>Büro &amp; mehr</title></bookmark><bookmark href='smb://nas/share'><title>Öffentlich &amp; Team</title><info><metadata owner="http://www.kde.org"><IsHidden>false</IsHidden></metadata></info></bookmark><bookmark href="smb://hidden/share"><info><metadata><IsHidden>true</IsHidden></metadata></info></bookmark></xbel>"#).unwrap();
        fs::write(
            p.config.join("gtk-3.0/bookmarks"),
            "sftp://u:secret@host/a%20b Büro\nfile:///home/local Local\nnot-a-url\n",
        )
        .unwrap();
        fs::write(p.home.join(".gtk-bookmarks"), "").unwrap();
        let mount = p.runtime.join("gvfs/smb-share:server=nas,share=share");
        fs::create_dir_all(&mount).unwrap();
        let places = discover(&p);
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].sources, ["dolphin", "gnome"]);
        assert_eq!(places[0].name, "Büro Server");
        assert_eq!(places[1].name, "Öffentlich & Team");
        assert_eq!(places[1].mounted_at.as_ref(), Some(&mount));
        assert!(!serde_json::to_string(&places).unwrap().contains("secret"));
        assert_eq!(discover(&p), places);
        fs::remove_dir_all(&p.data).unwrap();
        fs::remove_dir_all(&p.config).unwrap();
        fs::remove_dir_all(&p.runtime).unwrap();
        assert!(discover(&p).is_empty());
    }
    #[test]
    fn caps_entries_and_maps_protocols_and_existing_targets() {
        let (_tmp, p) = fixture();
        fs::write(
            p.home.join(".gtk-bookmarks"),
            (0..300)
                .map(|i| format!("sftp://h{i}/ N{i}\n"))
                .collect::<String>(),
        )
        .unwrap();
        assert_eq!(discover(&p).len(), 200);
        for (scheme, expected) in [
            ("fish", ConnectionProtocol::Sftp),
            ("ssh", ConnectionProtocol::Sftp),
            ("sftp", ConnectionProtocol::Sftp),
            ("ftp", ConnectionProtocol::Ftp),
            ("ftps", ConnectionProtocol::Ftps),
            ("smb", ConnectionProtocol::Smb),
            ("nfs", ConnectionProtocol::System),
            ("davs", ConnectionProtocol::System),
        ] {
            let place = parse_url(&format!("{scheme}://host/folder"), "", "Name").unwrap();
            let mut profile = place.profile();
            profile.id = "p1".into();
            assert_eq!(profile.protocol, expected);
            assert!(profile.validate().is_ok(), "{profile:?}");
            assert!(!profile.save_secret);
            assert!(place.matches(&profile));
            if expected == ConnectionProtocol::Sftp {
                assert_eq!(profile.transport, SshTransport::Openssh);
                profile.port = 22;
                assert!(place.matches(&profile));
            }
            profile.host = "other".into();
            assert!(!place.matches(&profile));
        }
    }
    #[test]
    fn kio_and_gvfs_mount_descendants_and_boundaries() {
        let (_tmp, p) = fixture();
        let kio = p.runtime.join("kio-fuse-123/nfs/host/export/Unter ordner");
        fs::create_dir_all(&kio).unwrap();
        let place = parse_url("nfs://host/export/Unter%20ordner", "", "").unwrap();
        assert_eq!(mounted_path(&place, &p), Some(kio));
        let gvfs = p.runtime.join("gvfs/sftp:host=host,user=u");
        fs::create_dir_all(gvfs.join("sub")).unwrap();
        assert_eq!(
            mounted_path(&parse_url("sftp://u@host/sub", "", "").unwrap(), &p),
            Some(gvfs.join("sub"))
        );
        assert!(mounted_path(&parse_url("sftp://other@host/sub", "", "").unwrap(), &p).is_none());
    }
    #[test]
    fn import_is_atomic_idempotent_and_disambiguates_names() {
        let a = parse_url("sftp://host/", "", "Same").unwrap();
        let b = parse_url("smb://host/share", "", "Same").unwrap();
        let places = [a.clone(), b.clone()];
        let urls = [a.url.clone(), b.url.clone()];
        let profiles = import_places(&[], &places, &urls).unwrap();
        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[1].name, "Same (2)");
        assert_eq!(import_places(&profiles, &places, &urls).unwrap(), profiles);
        assert!(import_places(&[], &places, &[a.url, "smb://unknown/share".into()]).is_err());
        assert!(import_places(&[], &places, &vec![b.url; 201]).is_err());
    }
    #[test]
    fn mounted_paths_do_not_leak_passwords_and_dav_aliases_resolve() {
        let (_tmp, p) = fixture();
        fs::create_dir_all(p.runtime.join("kio-fuse-x/smb/u:secret@host/share")).unwrap();
        fs::create_dir_all(p.runtime.join("gvfs/sftp:host=host,user=u:secret")).unwrap();
        fs::create_dir_all(
            p.runtime
                .join("gvfs/smb-share:server=host,share=share,password=secret"),
        )
        .unwrap();
        assert!(discover(&p).is_empty());
        let mounted = p
            .runtime
            .join("gvfs/dav:host=cloud,ssl=true,prefix=%2Fdocs");
        fs::create_dir_all(mounted.join("sub")).unwrap();
        let place = parse_url("webdavs://cloud:443/docs/sub", "", "Cloud").unwrap();
        assert_eq!(mounted_path(&place, &p), Some(mounted.join("sub")));
    }
    #[test]
    fn system_profile_rejects_passwords_and_legacy_settings_migrate() {
        let place = parse_url("smb://u@host/share", "", "Share").unwrap();
        let mut profile = place.profile();
        profile.id = "p1".into();
        assert!(profile.validate().is_ok());
        profile.system_url = Some("smb://u:secret@host/share".into());
        assert!(profile.validate().is_err());
        let (_tmp, paths) = fixture();
        fs::write(paths.home.join("settings.json"), r#"{"version":5,"connections":[{"id":"p1","name":"SSH","protocol":"sftp","transport":"openssh","host":"host","port":0,"user":"","tls":"none"}]}"#).unwrap();
        let (settings, _) = crate::settings::load(&paths.home);
        assert_eq!(settings.version, 6);
        assert_eq!(settings.connections.len(), 1);
        assert!(settings.connections[0].system_url.is_none());
    }
    #[test]
    fn bookmarks_list_desktop_places_without_secrets_and_drop_mounted_ones() {
        let (_tmp, paths) = fixture();
        fs::write(
            paths.data.join("user-places.xbel"),
            r#"<xbel><bookmark href="sftp://austausch/daten/"><title>Austausch</title></bookmark>
<bookmark href="smb://nas:geheim@homebox.example/"><title>smb://nas:geheim@homebox.example/</title></bookmark>
<bookmark href="file:///home/me"><title>Home</title></bookmark></xbel>"#,
        )
        .unwrap();
        fs::write(
            paths.config.join("gtk-3.0/bookmarks"),
            "davs://cloud.example/files Cloud\nsftp://austausch/daten/ Austausch\n",
        )
        .unwrap();
        // A mount that exists is ignored here: bookmarks never look at mounts.
        fs::create_dir_all(paths.runtime.join("gvfs/sftp:host=austausch")).unwrap();
        let found = bookmarks(&paths);
        assert_eq!(
            found.iter().map(|p| p.url.as_str()).collect::<Vec<_>>(),
            [
                "sftp://austausch/daten/",
                "smb://nas@homebox.example/",
                "davs://cloud.example/files"
            ]
        );
        assert_eq!(found[0].sources, ["dolphin", "gnome"]);
        assert!(found.iter().all(|p| p.mounted_at.is_none()));
        // A title that repeats the URL (with its password) is replaced by the host name.
        assert_eq!(found[1].name, "homebox.example");
        let public = format!("{found:?} {}", serde_json::to_string(&found).unwrap());
        assert!(!public.contains("geheim"));

        let mounted = mounts(&paths);
        assert_eq!(mounted.len(), 1);
        let shown = unmounted_bookmarks(found, &mounted);
        assert_eq!(
            shown.iter().map(|p| p.host.as_str()).collect::<Vec<_>>(),
            ["homebox.example", "cloud.example"]
        );
        // Another account or another share on the same server is a different place.
        let other = vec![
            parse_url("smb://guest@homebox.example/", "mounted", "").unwrap(),
            parse_url("davs://cloud.example/other", "mounted", "").unwrap(),
        ];
        assert_eq!(unmounted_bookmarks(shown.clone(), &other), shown);
    }
}
