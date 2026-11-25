//! RAR, read only, through an external helper (`unrar`, `7zz` or `7z`) found at run time: on the
//! system, or the `7zz` shipped with the AppImage/Flatpak. The helper is started without a shell, with an argument list that ends option
//! parsing (`--`), an empty temporary working folder, a cleaned environment and no stdin.
//! A helper that shows no progress for `RarConfig::timeout` is killed.

use super::budget::{limit, map_io, Counted, ReadCtx, Stats, MSG_ENCRYPTED};
use super::index::{ArchiveLimits, IndexBuilder, Loc, RawEntry, RawKind};
use super::zipfmt::{days_from_civil, unix_time};
use super::{RarConfig, Source};
use crate::error::{Error, Result};
use crate::helpers::{self, HelperEnv, HelperKind};
use std::ffi::OsString;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub(crate) const MSG_NO_TOOL: &str = "RAR benötigt unrar oder 7z";
const MSG_NESTED: &str = "RAR-Archive in Archiven werden nicht unterstützt";
const MSG_TOOL_FAILED: &str = "Das RAR-Hilfsprogramm meldet einen Fehler";
const MSG_TOOL_TIMEOUT: &str = "Das RAR-Hilfsprogramm hat das Zeitlimit überschritten";
const MSG_TOOL_OUTPUT: &str = "Die Ausgabe des RAR-Hilfsprogramms ist zu groß";
const MSG_WILDCARD: &str = "Name mit Platzhaltern wird bei RAR nicht unterstützt";
const MSG_SPECIAL: &str = "Sonderdatei (nur Anzeige)";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolKind {
    Unrar,
    SevenZip,
}

#[derive(Clone, Debug)]
pub(crate) struct Tool {
    pub kind: ToolKind,
    pub program: PathBuf,
}

pub(crate) struct RarBackend {
    path: PathBuf,
    tool: Tool,
    config: RarConfig,
}

/// Finds `unrar` on the system, then the `7zz` shipped with the package, then `7zz` and `7z`
/// on the system (see [`crate::helpers`]).
pub(crate) fn find_tool(config: &RarConfig) -> Option<Tool> {
    let env = config.helper_env.clone().unwrap_or_else(HelperEnv::current);
    let spec = helpers::spec("rar")?;
    let found = helpers::locate(spec, config.search_path.as_deref(), &env)?;
    Some(Tool {
        kind: match found.kind {
            HelperKind::Unrar => ToolKind::Unrar,
            HelperKind::SevenZip => ToolKind::SevenZip,
        },
        program: found.program,
    })
}

/// A path argument that can never be read as an option.
fn safe_arg(path: &Path) -> OsString {
    if path.as_os_str().to_string_lossy().starts_with('-') {
        let mut arg = OsString::from("./");
        arg.push(path.as_os_str());
        arg
    } else {
        path.as_os_str().to_os_string()
    }
}

pub(crate) fn list_args(tool: &Tool, archive: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = match tool.kind {
        ToolKind::Unrar => vec!["lt".into(), "-p-".into()],
        ToolKind::SevenZip => vec!["l".into(), "-slt".into(), "-bd".into(), "-y".into()],
    };
    args.push("--".into());
    args.push(safe_arg(archive));
    args
}

pub(crate) fn extract_args(tool: &Tool, archive: &Path, name: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = match tool.kind {
        ToolKind::Unrar => vec!["p".into(), "-inul".into(), "-p-".into()],
        ToolKind::SevenZip => vec!["e".into(), "-so".into(), "-bd".into(), "-y".into()],
    };
    args.push("--".into());
    args.push(safe_arg(archive));
    args.push(name.into());
    args
}

struct WorkDir(PathBuf);
impl WorkDir {
    fn create() -> io::Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos());
            let dir = std::env::temp_dir().join(format!(
                "dualfile-rar-{}-{nanos}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::create_dir(&dir) {
                Ok(()) => return Ok(Self(dir)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::other("Arbeitsordner nicht anlegbar"))
    }
}
impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kill_tree(child: &Mutex<Child>) {
    let mut child = child.lock().unwrap();
    #[cfg(unix)]
    if let Some(pid) = rustix::process::Pid::from_raw(child.id() as i32) {
        // The helper runs in its own process group: also stops anything it started.
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
}

/// stdout of a running helper. Reading blocks only while the helper is silent; a watchdog
/// kills it when that lasts longer than the timeout.
struct Process {
    child: Arc<Mutex<Child>>,
    stdout: ChildStdout,
    waiting_since: Arc<Mutex<Option<Instant>>>,
    timed_out: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
    status_checked: bool,
    timeout: Duration,
    _work: WorkDir,
}

impl Process {
    fn spawn(tool: &Tool, args: &[OsString], timeout: Duration) -> io::Result<Self> {
        let work = WorkDir::create()?;
        let mut command = Command::new(&tool.program);
        command
            .args(args)
            .env_clear()
            .env("LC_ALL", "C")
            .env("LANG", "C")
            .env("HOME", &work.0)
            .current_dir(&work.0)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn()?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("Keine Ausgabe"))?;
        let child = Arc::new(Mutex::new(child));
        let waiting_since = Arc::new(Mutex::new(None::<Instant>));
        let timed_out = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        {
            let (child, waiting, timed_out, finished) = (
                child.clone(),
                waiting_since.clone(),
                timed_out.clone(),
                finished.clone(),
            );
            let tick = (timeout / 5).clamp(Duration::from_millis(5), Duration::from_millis(100));
            std::thread::spawn(move || loop {
                std::thread::sleep(tick);
                if finished.load(Ordering::Relaxed) {
                    return;
                }
                let silent = waiting
                    .lock()
                    .unwrap()
                    .is_some_and(|t| t.elapsed() >= timeout);
                if silent {
                    timed_out.store(true, Ordering::Relaxed);
                    kill_tree(&child);
                    return;
                }
            });
        }
        Ok(Self {
            child,
            stdout,
            waiting_since,
            timed_out,
            finished,
            status_checked: false,
            timeout,
            _work: work,
        })
    }

    fn check_exit(&mut self) -> io::Result<()> {
        if self.status_checked {
            return Ok(());
        }
        self.status_checked = true;
        if self.timed_out.load(Ordering::Relaxed) {
            return Err(limit(MSG_TOOL_TIMEOUT));
        }
        // The output is closed; the program should end now. Do not wait longer than the timeout.
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = self.child.lock().unwrap().try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                kill_tree(&self.child);
                return Err(limit(MSG_TOOL_TIMEOUT));
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        // 0 = success, 1 = warning (both programs); everything else is a failure.
        match status.code() {
            Some(0 | 1) => Ok(()),
            _ => Err(limit(MSG_TOOL_FAILED)),
        }
    }
}

impl Read for Process {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        *self.waiting_since.lock().unwrap() = Some(Instant::now());
        let result = self.stdout.read(buf);
        *self.waiting_since.lock().unwrap() = None;
        match result {
            Ok(0) if !buf.is_empty() => self.check_exit().map(|()| 0),
            other => other,
        }
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.finished.store(true, Ordering::Relaxed);
        kill_tree(&self.child);
        let _ = self.child.lock().unwrap().wait();
    }
}

fn run_capture(tool: &Tool, args: &[OsString], config: &RarConfig) -> Result<String> {
    let path = Path::new("");
    let mut process = Process::spawn(tool, args, config.timeout).map_err(|e| map_io(e, path))?;
    let mut data = Vec::new();
    (&mut process)
        .take(config.output_limit + 1)
        .read_to_end(&mut data)
        .map_err(|e| map_io(e, path))?;
    if data.len() as u64 > config.output_limit {
        return Err(Error::Archive(MSG_TOOL_OUTPUT));
    }
    Ok(String::from_utf8_lossy(&data).into_owned())
}

#[derive(Debug, Default, Clone, PartialEq)]
pub(crate) struct RarItem {
    pub name: String,
    pub is_dir: bool,
    pub symlink: bool,
    pub special: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub mode: Option<u32>,
    pub encrypted: bool,
}

fn parse_time(text: &str) -> Option<SystemTime> {
    let (date, time) = text.trim().split_once(' ')?;
    let mut d = date.split('-');
    let (year, month, day) = (
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
        d.next()?.parse::<i64>().ok()?,
    );
    let time = time.split(['.', ',']).next()?;
    let mut t = time.split(':');
    let (hour, minute, second) = (
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
        t.next()?.parse::<i64>().ok()?,
    );
    unix_time(days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second)
}

/// `-rwxr-xr-x` style permission string (type character included) to (type, mode bits).
fn parse_unix_attributes(text: &str) -> Option<(char, u32)> {
    let token = text.split_whitespace().find(|token| {
        token.len() == 10
            && token.chars().next().is_some_and(|c| "-dlcbps".contains(c))
            && token[1..].chars().all(|c| "rwxsStT-".contains(c))
    })?;
    let chars: Vec<char> = token.chars().collect();
    let mut mode = 0;
    for (i, c) in chars[1..].iter().enumerate() {
        if "rwxst".contains(*c) {
            mode |= 1 << (8 - i);
        }
    }
    Some((chars[0], mode))
}

fn apply_attributes(item: &mut RarItem, attributes: &str) {
    if let Some((kind, mode)) = parse_unix_attributes(attributes) {
        item.mode = Some(mode);
        match kind {
            'd' => item.is_dir = true,
            'l' => item.symlink = true,
            'c' | 'b' | 'p' | 's' => item.special = true,
            _ => {}
        }
    }
}

/// Parses the output of `7z l -slt`.
pub(crate) fn parse_7z_listing(text: &str) -> Vec<RarItem> {
    let mut items = Vec::new();
    let mut current: Option<RarItem> = None;
    let mut in_entries = false;
    let flush = |item: &mut Option<RarItem>, items: &mut Vec<RarItem>| {
        if let Some(item) = item.take() {
            if !item.name.is_empty() {
                items.push(item);
            }
        }
    };
    for line in text.lines() {
        if !in_entries {
            in_entries = line.starts_with("----------");
            continue;
        }
        if line.trim().is_empty() {
            flush(&mut current, &mut items);
            continue;
        }
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        if key == "Path" {
            flush(&mut current, &mut items);
            current = Some(RarItem {
                name: value.to_string(),
                ..RarItem::default()
            });
            continue;
        }
        let Some(item) = current.as_mut() else {
            continue;
        };
        match key {
            "Folder" => item.is_dir = value.trim() == "+",
            "Size" => item.size = value.trim().parse().unwrap_or(0),
            "Modified" => item.modified = parse_time(value),
            "Attributes" => apply_attributes(item, value),
            "Encrypted" => item.encrypted = value.trim() == "+",
            _ => {}
        }
    }
    flush(&mut current, &mut items);
    items
}

/// Parses the output of `unrar lt`.
pub(crate) fn parse_unrar_listing(text: &str) -> Vec<RarItem> {
    let mut items: Vec<RarItem> = Vec::new();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key == "Name" {
            items.push(RarItem {
                name: value.to_string(),
                ..RarItem::default()
            });
            continue;
        }
        let Some(item) = items.last_mut() else {
            continue;
        };
        match key {
            "Type" => {
                let lower = value.to_lowercase();
                item.is_dir = lower.contains("directory");
                item.symlink = lower.contains("symbolic");
                item.special = !item.symlink && lower.contains("link");
            }
            "Size" => item.size = value.parse().unwrap_or(0),
            "mtime" => item.modified = parse_time(value),
            "Attributes" => apply_attributes(item, value),
            "Flags" => item.encrypted = value.to_lowercase().contains("encrypted"),
            _ => {}
        }
    }
    items
}

impl RarBackend {
    pub(crate) fn open_entry(
        &self,
        name: &str,
        size: u64,
        ctx: &ReadCtx,
    ) -> io::Result<Box<dyn Read + Send>> {
        let args = extract_args(&self.tool, &self.path, name);
        let process = Process::spawn(&self.tool, &args, self.config.timeout)?;
        // The helper's own compression is invisible here: no ratio guard, but the declared
        // size, the per-file cap and the budgets apply to what actually arrives.
        let stats = Arc::new(Stats::default());
        stats.compressed.store(size.max(1), Ordering::Relaxed);
        Ok(Box::new(Counted::for_entry(
            process,
            ctx,
            stats,
            Some(size),
        )))
    }
}

fn has_wildcard(name: &str) -> bool {
    name.starts_with('@') || name.chars().any(|c| matches!(c, '*' | '?' | '[' | ']'))
}

pub(crate) fn index(
    source: &Source,
    limits: &ArchiveLimits,
    builder: &mut IndexBuilder,
) -> Result<RarBackend> {
    let Source::File(path) = source else {
        return Err(Error::Archive(MSG_NESTED));
    };
    let tool = find_tool(&limits.rar).ok_or(Error::Archive(MSG_NO_TOOL))?;
    let output = run_capture(&tool, &list_args(&tool, path), &limits.rar)?;
    let items = match tool.kind {
        ToolKind::Unrar => parse_unrar_listing(&output),
        ToolKind::SevenZip => parse_7z_listing(&output),
    };
    for item in items {
        let kind = if item.is_dir {
            RawKind::Dir
        } else if item.symlink {
            RawKind::Symlink
        } else if item.special {
            RawKind::Special(MSG_SPECIAL)
        } else {
            RawKind::File
        };
        let unreadable = if item.encrypted {
            Some(MSG_ENCRYPTED)
        } else if has_wildcard(&item.name) {
            Some(MSG_WILDCARD)
        } else {
            None
        };
        builder.add(RawEntry {
            loc: Loc::Rar(item.name.clone()),
            path: item.name,
            kind,
            size: item.size,
            modified: item.modified,
            mode: item.mode,
            link_target: None,
            unreadable,
        })?;
    }
    Ok(RarBackend {
        path: path.clone(),
        tool,
        config: limits.rar.clone(),
    })
}
