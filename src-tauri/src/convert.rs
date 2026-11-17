//! Type conversion between `dualfile-core` and the web UI contract (`src/ipc/types.ts`).
//! Everything here is a pure function or a plain data type, so it is unit-tested without Tauri.

use dualfile_core::archive::{Format, TarCompression};
use dualfile_core::compare::{MSG_BINARY, MSG_TOO_LARGE};
use dualfile_core::jobs::{JobState, JobStatus};
use dualfile_core::lister::{Direction, FileInfo, FileKind, SearchOutcome};
use dualfile_core::ops::{self, Conflict, DeletePlan, OpReport};
use dualfile_core::settings::{self, LoadOutcome};
use dualfile_core::vfs::{format_ipc_path, Entry, EntryKind, VPath};
use dualfile_core::Error;
use dualfile_remote::manager::ConnectError;
use dualfile_remote::trust::TrustError;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

// ---------------------------------------------------------------- Errors

/// Error as the UI sees it: a kind (`CoreError.kind`) plus a message without internals.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiError {
    pub kind: String,
    pub message: String,
}

impl UiError {
    pub fn new(kind: &str, message: impl Into<String>) -> Self {
        Self {
            kind: kind.to_owned(),
            message: message.into(),
        }
    }

    pub fn internal() -> Self {
        Self::new("io", "Interner Fehler")
    }
}

/// Message text of a core error. The OS error text of `Error::Io` is left out on purpose
/// (it carries `errno` details); operation and path are enough for the user.
pub fn error_message(error: &Error) -> String {
    match error {
        Error::Io { op, path, .. } => format!("{op} fehlgeschlagen für {}", path.display()),
        // Complete sentences of the text diff; the generic "not supported" prefix would only
        // bury them.
        Error::Unsupported(message) if *message == MSG_BINARY || *message == MSG_TOO_LARGE => {
            (*message).to_owned()
        }
        // Server texts are not passed on (they could echo what was sent); the kind says what
        // went wrong, the text is fixed.
        Error::Remote(_) => MSG_REMOTE.to_owned(),
        Error::SizeMismatch(_) => MSG_SIZE_MISMATCH.to_owned(),
        other => other.to_string(),
    }
}

pub const MSG_REMOTE: &str = "Der Server hat den Vorgang nicht ausgeführt";
pub const MSG_SIZE_MISMATCH: &str = "Die Größe stimmt nach dem Übertragen nicht";

/// A failed connect as the UI sees it: fixed texts, never a server reply and never anything
/// that could contain a credential.
pub fn connect_error(error: &ConnectError) -> UiError {
    use dualfile_remote::error::TransportError as T;
    match error {
        ConnectError::Invalid(message) => UiError::new("invalidInput", *message),
        ConnectError::TooManyConnections => UiError::new(
            "tooManyConnections",
            "Es sind schon zu viele Verbindungen offen",
        ),
        ConnectError::Transport(transport) => match transport {
            T::Auth
            | T::Reply {
                code: 530 | 532, ..
            } => UiError::new("auth", "Die Anmeldung am Server ist fehlgeschlagen"),
            T::AuthFailed => UiError::new(
                "auth",
                "Die Anmeldung ist fehlgeschlagen: Schlüssel oder ssh-agent nötig",
            ),
            T::HostKeyRejected => UiError::new(
                "hostKeyRejected",
                "Der Schlüssel des Servers wurde von OpenSSH nicht akzeptiert",
            ),
            T::OpensshMissing => {
                UiError::new("opensshMissing", "OpenSSH (ssh) wurde nicht gefunden")
            }
            T::Unreachable => UiError::new("unreachable", "Der SSH-Server ist nicht erreichbar"),
            T::SubsystemMissing => UiError::new(
                "subsystemMissing",
                "Der Server bietet kein SFTP-Subsystem an",
            ),
            T::SmbSecurity => UiError::new(
                "smbSecurity",
                "SMB-Signatur oder Verschlüsselung fehlgeschlagen",
            ),
            T::SmbUnreachable => UiError::new("smbUnreachable", "SMB-Server nicht erreichbar"),
            T::SmbShareMissing => UiError::new("smbShareMissing", "SMB-Freigabe nicht gefunden"),
            T::SmbAuth => UiError::new("smbAuth", "SMB-Anmeldung abgelehnt"),
            T::SmbFailure => UiError::new("smbFailure", "SMB-Vorgang fehlgeschlagen"),
            T::Timeout => UiError::new("timeout", "Der Server hat nicht rechtzeitig geantwortet"),
            T::Trust(trust) => match trust {
                TrustError::PinMismatch { .. } => UiError::new(
                    "pinMismatch",
                    "Das Zertifikat oder der Schlüssel des Servers hat sich geändert",
                ),
                TrustError::AlgorithmNotAllowed(_) => UiError::new(
                    "algorithmNotAllowed",
                    "Der Server verwendet einen nicht erlaubten Schlüsselalgorithmus",
                ),
                TrustError::Expired => UiError::new(
                    "trust",
                    "Das Zertifikat des Servers ist abgelaufen oder noch nicht gültig",
                ),
                TrustError::InvalidChain => {
                    UiError::new("trust", "Die Zertifikatskette des Servers ist ungültig")
                }
                TrustError::NotConfirmed(_) => {
                    UiError::new("trust", "Das Zertifikat des Servers wurde nicht bestätigt")
                }
            },
            T::Rejected => UiError::new("invalidInput", "Unzulässiges Zeichen in einem Befehl"),
            T::Unsupported(what) => UiError::new("unsupported", *what),
            T::Connection(_) | T::Tls(_) | T::Protocol(_) | T::TooLarge(_) => UiError::new(
                "remote",
                "Die Verbindung zum Server ist fehlgeschlagen oder wurde unterbrochen",
            ),
            T::Reply { .. } | T::Status { .. } => UiError::new("remote", MSG_REMOTE),
        },
    }
}

pub fn error_kind(error: &Error) -> &'static str {
    match error {
        Error::NotFound(_) => "notFound",
        Error::Exists(_) => "exists",
        Error::PermissionDenied(_) => "permission",
        Error::NotADirectory(_) => "notDirectory",
        Error::InvalidName(_) => "invalidName",
        Error::InvalidPath(_) => "invalidPath",
        Error::InvalidInput(_) => "invalidInput",
        Error::DestinationInsideSource | Error::SameFile => "invalidTarget",
        Error::Disconnected => "disconnected",
        Error::Cancelled => "cancelled",
        Error::Unsupported(message)
            if *message == dualfile_core::archive::MSG_READ_ONLY
                || *message == dualfile_core::plugins::MSG_PLUGIN_READ_ONLY =>
        {
            "readOnly"
        }
        Error::Unsupported(_) | Error::NoReplaceUnsupported => "unsupported",
        // Damaged, encrypted, too large or too slow archives: the UI shows the message and
        // offers to leave the archive, which is different from a plain I/O failure.
        Error::Archive(_) => "archive",
        Error::Remote(code) if code == "smbSecurity" => "smbSecurity",
        Error::Remote(code) if code == "smbUnreachable" => "smbUnreachable",
        Error::Remote(code) if code == "smbShareMissing" => "smbShareMissing",
        Error::Remote(code) if code == "smbAuth" => "smbAuth",
        Error::Remote(code) if code == "smbFailure" => "smbFailure",
        Error::Remote(_) => "remote",
        Error::Plugin { kind, .. } => kind,
        Error::SizeMismatch(_) => "sizeMismatch",
        Error::VerifyFailed => "verifyFailed",
        Error::IsADirectory(_) => "isADirectory",
        Error::DirectoryNotEmpty(_) => "directoryNotEmpty",
        Error::ChangedSincePlan(_) => "changedSincePlan",
        Error::IncompletePlan => "incompletePlan",
        Error::RenameLeftover(_) => "renameLeftover",
        Error::Io { .. } => "io",
    }
}

/// Error for a failed `std::fs` call made by this crate itself (not by the core).
pub fn io_error(what: &str, path: &Path, error: &std::io::Error) -> UiError {
    match error.kind() {
        std::io::ErrorKind::NotFound => UiError::new(
            "notFound",
            format!("{what} wurde nicht gefunden: {}", path.display()),
        ),
        std::io::ErrorKind::PermissionDenied => UiError::new(
            "permission",
            format!("Zugriff verweigert: {}", path.display()),
        ),
        _ => UiError::new("io", format!("{what} nicht lesbar: {}", path.display())),
    }
}

impl From<Error> for UiError {
    fn from(error: Error) -> Self {
        Self::new(error_kind(&error), error_message(&error))
    }
}

// ---------------------------------------------------------------- Lister

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileInfoDto {
    pub size: u64,
    pub kind: &'static str,
    pub modified: Option<i64>,
    pub mime_guess: &'static str,
}

pub fn file_info_dto(info: &FileInfo) -> FileInfoDto {
    FileInfoDto {
        size: info.size,
        kind: match info.kind {
            FileKind::Text => "text",
            FileKind::Binary => "binary",
            FileKind::Image => "image",
            FileKind::Audio => "audio",
            FileKind::Video => "video",
        },
        modified: info.modified.map(millis),
        mime_guess: info.mime_guess,
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum DirectionDto {
    Forward,
    Backward,
}

impl From<DirectionDto> for Direction {
    fn from(direction: DirectionDto) -> Self {
        match direction {
            DirectionDto::Forward => Direction::Forward,
            DirectionDto::Backward => Direction::Backward,
        }
    }
}

/// `{ kind: "found", offset }`, `{ kind: "notFound" }` or `{ kind: "continue", next }`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SearchResultDto {
    Found { offset: u64 },
    NotFound,
    Continue { next: u64 },
}

impl From<SearchOutcome> for SearchResultDto {
    fn from(outcome: SearchOutcome) -> Self {
        match outcome {
            SearchOutcome::Found(offset) => Self::Found { offset },
            SearchOutcome::NotFound => Self::NotFound,
            SearchOutcome::Continue(next) => Self::Continue { next },
        }
    }
}

// ---------------------------------------------------------------- Entries and listings

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryDto {
    pub name: String,
    pub kind: &'static str,
    pub size: u64,
    pub modified: Option<i64>,
    pub mode: Option<u32>,
    pub hidden: bool,
    pub link_target: Option<String>,
    pub broken_link: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListingDto {
    pub path: String,
    pub entries: Vec<EntryDto>,
}

/// Milliseconds since 1970 (negative before), as the UI expects.
pub fn millis(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

pub fn kind_name(kind: EntryKind) -> &'static str {
    match kind {
        EntryKind::File => "file",
        EntryKind::Dir => "dir",
        EntryKind::Symlink => "symlink",
    }
}

pub fn entry_dto(entry: &Entry) -> EntryDto {
    EntryDto {
        name: entry.name.clone(),
        kind: kind_name(entry.kind),
        size: entry.size,
        modified: entry.modified.map(millis),
        mode: entry.mode,
        hidden: entry.hidden,
        link_target: entry.link_target.clone(),
        broken_link: entry.broken_link,
        error: entry.error.clone(),
    }
}

pub fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// A location as the UI knows it: a local path as is, an archive entry in the IPC format
/// `archive:<outer>!<inner>` (never just the inner path, which would look like a local one).
pub fn vpath_text(path: &VPath) -> String {
    format_ipc_path(path)
}

// ---------------------------------------------------------------- Jobs

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PathMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub path: String,
    pub message: String,
}

/// Language-neutral job title: the UI builds "Copy: x (+2)" from it in the interface language.
/// `name` of the job stays as the German fallback for older clients.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobTitleDto {
    pub kind: String,
    pub first: String,
    pub more: usize,
}

impl JobTitleDto {
    pub fn new(kind: &str, first: impl Into<String>, more: usize) -> Self {
        Self {
            kind: kind.to_owned(),
            first: first.into(),
            more,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStatusDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<JobTitleDto>,
    pub verified: bool,
    pub id: u64,
    pub name: String,
    pub state: &'static str,
    pub files_done: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub bytes_per_second: f64,
    pub eta_seconds: Option<u64>,
    pub queue_position: Option<usize>,
    pub lane: Option<String>,
    pub failed: Vec<PathMessage>,
    pub skipped: Vec<String>,
    pub warnings: Vec<PathMessage>,
}

pub fn state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Paused => "paused",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
    }
}

fn report_parts(report: &OpReport) -> (Vec<PathMessage>, Vec<String>, Vec<PathMessage>) {
    let failed = report
        .failed
        .iter()
        .map(|(path, error)| PathMessage {
            kind: Some(error_kind(error).into()),
            path: vpath_text(path),
            message: error_message(error),
        })
        .collect();
    let skipped = report.skipped.iter().map(vpath_text).collect();
    let warnings = report
        .warnings
        .iter()
        .map(|(path, message)| PathMessage {
            kind: None,
            path: vpath_text(path),
            message: message.clone(),
        })
        .collect();
    (failed, skipped, warnings)
}

/// UI status of a job. The core knows no job name, so the caller passes it in. The report
/// only exists once a job is finished; until then the lists are empty. Jobs that do not
/// report per-item progress (trash, delete) get their file count from the report.
pub fn job_status_dto(id: u64, name: &str, status: &JobStatus) -> JobStatusDto {
    let (failed, skipped, warnings) = status
        .report
        .as_deref()
        .map(report_parts)
        .unwrap_or_default();
    let reported_done = status
        .report
        .as_deref()
        .map_or(0, |report| report.done.len() as u64);
    JobStatusDto {
        title: None,
        verified: status.report.as_ref().is_some_and(|r| r.verified),
        id,
        name: name.to_owned(),
        state: state_name(status.state),
        eta_seconds: status.eta_seconds,
        queue_position: status.queue_position,
        lane: status.lane.as_deref().map(crate::lanes::opaque),
        files_done: status.files_done.max(reported_done),
        bytes_done: status.bytes_done,
        bytes_total: status.bytes_total,
        bytes_per_second: if status.bytes_per_second.is_finite() {
            status.bytes_per_second
        } else {
            0.0
        },
        failed,
        skipped,
        warnings,
    }
}

// ---------------------------------------------------------------- Packing

/// Archive formats the UI may ask for (`PackFormat` in `src/ipc/types.ts`). RAR is read-only.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum PackFormatDto {
    Zip,
    TarGz,
    TarBz2,
    TarXz,
    SevenZ,
}

impl PackFormatDto {
    pub fn to_core(self) -> Format {
        match self {
            Self::Zip => Format::Zip,
            Self::TarGz => Format::Tar(TarCompression::Gzip),
            Self::TarBz2 => Format::Tar(TarCompression::Bzip2),
            Self::TarXz => Format::Tar(TarCompression::Xz),
            Self::SevenZ => Format::SevenZ,
        }
    }
}

// ---------------------------------------------------------------- Conflicts

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictInfoDto {
    pub job_id: u64,
    pub source_path: String,
    pub destination_path: String,
    pub source: EntryDto,
    pub destination: EntryDto,
}

pub fn conflict_dto(job_id: u64, conflict: &Conflict) -> ConflictInfoDto {
    ConflictInfoDto {
        job_id,
        source_path: vpath_text(&conflict.source_path),
        destination_path: vpath_text(&conflict.destination_path),
        source: entry_dto(&conflict.source),
        destination: entry_dto(&conflict.destination),
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ResolutionDto {
    Overwrite,
    Skip,
    KeepBoth,
    Cancel,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConflictAnswerDto {
    pub resolution: ResolutionDto,
    pub apply_to_all: bool,
}

impl ConflictAnswerDto {
    /// "For all" maps to the core's `*All` decisions; cancelling is never "for all".
    pub fn to_core(self) -> ops::Resolution {
        use ops::Resolution as R;
        match (self.resolution, self.apply_to_all) {
            (ResolutionDto::Overwrite, false) => R::Overwrite,
            (ResolutionDto::Overwrite, true) => R::OverwriteAll,
            (ResolutionDto::Skip, false) => R::Skip,
            (ResolutionDto::Skip, true) => R::SkipAll,
            (ResolutionDto::KeepBoth, false) => R::KeepBoth,
            (ResolutionDto::KeepBoth, true) => R::KeepBothAll,
            (ResolutionDto::Cancel, _) => R::Cancel,
        }
    }
}

// ---------------------------------------------------------------- Delete plans

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletePlanDto {
    pub token: String,
    pub files: u64,
    pub dirs: u64,
    pub links: u64,
    pub bytes: u64,
    pub first_names: Vec<String>,
    pub incomplete: bool,
}

pub fn token_text(token: u128) -> String {
    format!("{token:032x}")
}

pub fn delete_plan_dto(plan: &DeletePlan) -> DeletePlanDto {
    DeletePlanDto {
        token: token_text(plan.token),
        files: plan.files,
        dirs: plan.dirs,
        links: plan.links,
        bytes: plan.bytes,
        first_names: plan.first_names.clone(),
        incomplete: plan.incomplete,
    }
}

/// What the UI sends back on confirmation. Only the token counts: the plan itself stays on
/// the Rust side and the numbers from the UI are never trusted.
#[derive(Clone, Debug, Deserialize)]
pub struct DeletePlanInput {
    pub token: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameDto {
    pub old_path: String,
    pub new_path: String,
}

/// One entry of a multi-rename: names inside the folder, never paths.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenameItemDto {
    pub old_name: String,
    pub new_name: String,
}

// ---------------------------------------------------------------- Settings

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkDto {
    pub name: String,
    pub path: String,
    pub color: String,
    pub order: u32,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TabDto {
    pub path: String,
    pub sort: String,
    pub filter: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowSessionDto {
    pub tabs: Vec<TabDto>,
    pub active_tab: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub windows: Vec<WindowSessionDto>,
    pub active_window: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsDto {
    pub version: u32,
    #[serde(default)]
    pub verify_policy: dualfile_core::ops::VerifyPolicy,
    pub theme: String,
    pub accent: String,
    #[serde(default)]
    pub accent_automatic: bool,
    #[serde(default = "default_show_parent_entry")]
    pub show_parent_entry: bool,
    #[serde(default = "default_terminal_height")]
    pub terminal_height: u32,
    #[serde(default = "default_terminal_shell")]
    pub terminal_shell: String,
    #[serde(default = "default_terminal_colors")]
    pub terminal_colors: String,
    #[serde(default = "default_terminal_bg")]
    pub terminal_bg: String,
    #[serde(default = "default_terminal_fg")]
    pub terminal_fg: String,
    #[serde(default = "default_show_parent_entry")]
    pub check_updates_on_start: bool,
    #[serde(default)]
    pub tray_enabled: bool,
    #[serde(default)]
    pub start_menu_prompted: bool,
    #[serde(default)]
    pub start_menu_prompt_count: u8,
    #[serde(default)]
    pub dismissed_update_version: String,
    #[serde(default)]
    pub last_update_check: Option<String>,
    #[serde(default)]
    pub last_seen_version: Option<String>,

    pub density: String,
    pub font_size: u8,
    pub language: String,
    pub bookmarks: Vec<BookmarkDto>,
    #[serde(default)]
    pub extension_column: bool,
    #[serde(default)]
    pub pane_color_left: String,
    #[serde(default)]
    pub pane_color_right: String,
    #[serde(default)]
    pub keybindings: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub column_widths: settings::ColumnWidths,
    pub session: SessionDto,
}

fn default_show_parent_entry() -> bool {
    true
}

fn default_terminal_height() -> u32 {
    260
}

fn default_terminal_shell() -> String {
    "login".into()
}

fn default_terminal_colors() -> String {
    "green".into()
}

fn default_terminal_bg() -> String {
    "#102030".into()
}

fn default_terminal_fg() -> String {
    "#f0e6d2".into()
}

impl From<&settings::Settings> for SettingsDto {
    fn from(s: &settings::Settings) -> Self {
        Self {
            version: s.version,
            verify_policy: s.verify_policy,
            theme: s.theme.clone(),
            accent: s.accent.clone(),
            accent_automatic: s.accent_automatic,
            show_parent_entry: s.show_parent_entry,
            terminal_height: s.terminal_height,
            terminal_shell: s.terminal_shell.clone(),
            terminal_colors: s.terminal_colors.clone(),
            terminal_bg: s.terminal_bg.clone(),
            terminal_fg: s.terminal_fg.clone(),
            check_updates_on_start: s.check_updates_on_start,
            tray_enabled: s.tray_enabled,
            start_menu_prompted: s.start_menu_prompted,
            start_menu_prompt_count: s.start_menu_prompt_count,
            dismissed_update_version: s.dismissed_update_version.clone(),
            last_update_check: s.last_update_check.clone(),
            last_seen_version: s.last_seen_version.clone(),

            density: s.density.clone(),
            font_size: s.font_size,
            language: s.language.clone(),
            bookmarks: s
                .bookmarks
                .iter()
                .map(|b| BookmarkDto {
                    name: b.name.clone(),
                    path: b.path.clone(),
                    color: b.color.clone(),
                    order: b.order,
                })
                .collect(),
            extension_column: s.extension_column,
            pane_color_left: s.pane_color_left.clone(),
            pane_color_right: s.pane_color_right.clone(),
            keybindings: s.keybindings.clone(),
            column_widths: s.column_widths.clone(),
            session: SessionDto {
                windows: s
                    .session
                    .windows
                    .iter()
                    .map(|w| WindowSessionDto {
                        tabs: w
                            .tabs
                            .iter()
                            .map(|t| TabDto {
                                path: t.path.clone(),
                                sort: t.sort.clone(),
                                filter: t.filter.clone(),
                            })
                            .collect(),
                        active_tab: w.active_tab,
                    })
                    .collect(),
                active_window: s.session.active_window,
            },
        }
    }
}

impl From<&SettingsDto> for settings::Settings {
    fn from(s: &SettingsDto) -> Self {
        Self {
            version: s.version,
            verify_policy: s.verify_policy,
            theme: s.theme.clone(),
            accent: s.accent.clone(),
            accent_automatic: s.accent_automatic,
            show_parent_entry: s.show_parent_entry,
            terminal_height: s.terminal_height,
            terminal_shell: s.terminal_shell.clone(),
            terminal_colors: s.terminal_colors.clone(),
            terminal_bg: s.terminal_bg.clone(),
            terminal_fg: s.terminal_fg.clone(),
            check_updates_on_start: s.check_updates_on_start,
            tray_enabled: s.tray_enabled,
            start_menu_prompted: s.start_menu_prompted,
            start_menu_prompt_count: s.start_menu_prompt_count,
            dismissed_update_version: s.dismissed_update_version.clone(),
            last_update_check: s.last_update_check.clone(),
            last_seen_version: s.last_seen_version.clone(),

            density: s.density.clone(),
            font_size: s.font_size,
            language: s.language.clone(),
            bookmarks: s
                .bookmarks
                .iter()
                .map(|b| settings::Bookmark {
                    name: b.name.clone(),
                    path: b.path.clone(),
                    color: b.color.clone(),
                    order: b.order,
                })
                .collect(),
            extension_column: s.extension_column,
            pane_color_left: s.pane_color_left.clone(),
            pane_color_right: s.pane_color_right.clone(),
            keybindings: s.keybindings.clone(),
            column_widths: s.column_widths.clone(),
            session: settings::Session {
                windows: s
                    .session
                    .windows
                    .iter()
                    .map(|w| settings::WindowSession {
                        tabs: w
                            .tabs
                            .iter()
                            .map(|t| settings::Tab {
                                path: t.path.clone(),
                                sort: t.sort.clone(),
                                filter: t.filter.clone(),
                            })
                            .collect(),
                        active_tab: w.active_tab,
                    })
                    .collect(),
                active_window: s.session.active_window,
            },
            // Profiles are not part of the UI's settings; `AppState::save_settings` keeps
            // the stored ones.
            connections: Vec::new(),
            plugins: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LoadOutcomeDto {
    Fresh,
    Loaded,
    Repaired { messages: Vec<String> },
    RecoveredFromCorrupt,
    NewerVersion { version: u32 },
    Migrated { from: u32, to: u32 },
}

impl From<LoadOutcome> for LoadOutcomeDto {
    fn from(outcome: LoadOutcome) -> Self {
        match outcome {
            LoadOutcome::Fresh => Self::Fresh,
            LoadOutcome::Loaded => Self::Loaded,
            LoadOutcome::Repaired(messages) => Self::Repaired { messages },
            LoadOutcome::RecoveredFromCorrupt => Self::RecoveredFromCorrupt,
            LoadOutcome::NewerVersion(version) => Self::NewerVersion { version },
            LoadOutcome::Migrated { from, to } => Self::Migrated { from, to },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedSettingsDto {
    pub settings: SettingsDto,
    pub outcome: LoadOutcomeDto,
}

// ---------------------------------------------------------------- Events

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemErrorInfoDto {
    pub job_id: u64,
    pub name: String,
    pub kind: &'static str,
    pub message: &'static str,
}

/// Payload of the `core-event` event; mirrors `CoreEvent` in `src/ipc/types.ts`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CoreEventDto {
    CloseRequested {
        count: usize,
    },
    CloseSaveRequested,
    ItemError {
        error: ItemErrorInfoDto,
    },
    UndoChanged,
    Job {
        status: JobStatusDto,
    },
    Conflict {
        conflict: ConflictInfoDto,
    },
    DirChanged {
        path: String,
    },
    /// A profile was saved, deleted or imported (possibly from the editor window).
    ConnectionsChanged,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoEntryDto {
    pub id: u64,
    pub time: u64,
    pub label: String,
    pub kind: &'static str,
    pub first: String,
    pub more: usize,
    pub undone: bool,
}
impl From<dualfile_core::undo::UndoEntry> for UndoEntryDto {
    fn from(e: dualfile_core::undo::UndoEntry) -> Self {
        use dualfile_core::undo::UndoKind;
        let kind = match e.kind {
            UndoKind::Move => "move",
            UndoKind::Rename => "rename",
            UndoKind::Trash => "trash",
            UndoKind::Copy => "copy",
            UndoKind::MakeDir => "mkdir",
        };
        Self {
            id: e.id,
            time: e
                .time
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
            label: format!("undoLabel.{kind}"),
            kind,
            first: e.first,
            more: e.more,
            undone: e.undone,
        }
    }
}
pub fn undo_error(error: dualfile_core::undo::UndoError) -> &'static str {
    use dualfile_core::undo::UndoError::*;
    match error {
        Changed => "changed",
        Conflict => "conflict",
        AlreadyUndone => "alreadyUndone",
        NotFound => "notFound",
        Unsupported => "unsupported",
        NotLocal => "notLocal",
        Io => "io",
    }
}
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoResultDto {
    pub job_id: u64,
    pub ok: bool,
    pub done: usize,
    pub remaining: usize,
    pub cancelled: bool,
    pub errors: Vec<&'static str>,
}
impl UndoResultDto {
    pub fn from_report(
        result: Result<dualfile_core::undo::UndoReport, dualfile_core::undo::UndoError>,
    ) -> Self {
        match result {
            Ok(r) => Self {
                job_id: 0,
                ok: r.remaining.is_empty() && !r.cancelled,
                done: r.done.len(),
                remaining: r.remaining.len(),
                cancelled: r.cancelled,
                errors: r.failed.iter().map(|(_, e)| undo_error(*e)).collect(),
            },
            Err(e) => Self {
                job_id: 0,
                ok: false,
                done: 0,
                remaining: 0,
                cancelled: false,
                errors: vec![undo_error(e)],
            },
        }
    }
}

/// Stable scan issue codes; presentation is localized by the UI.
pub fn scan_issue(issue: dualfile_core::usage::ScanIssue) -> &'static str {
    use dualfile_core::usage::ScanIssue;
    match issue {
        ScanIssue::Io(_) => "io",
        ScanIssue::DepthLimit => "depthLimit",
        ScanIssue::EntryLimit => "entryLimit",
        ScanIssue::FileSystemBoundary => "fileSystemBoundary",
        ScanIssue::Changed => "changed",
    }
}

/// Similar-image decoder failures have fixed codes, without decoder-provided text.
pub fn similar_issue(issue: dualfile_core::usage::ScanIssue) -> &'static str {
    match issue {
        dualfile_core::usage::ScanIssue::Io(std::io::ErrorKind::InvalidData) => "invalidImage",
        dualfile_core::usage::ScanIssue::Io(std::io::ErrorKind::FileTooLarge) => "imageTooLarge",
        other => scan_issue(other),
    }
}

pub fn system_mount_error(error: dualfile_remote::system_mount::MountError) -> UiError {
    use dualfile_remote::system_mount::MountError;
    match error {
        MountError::Timeout(detail) => UiError::new("systemMountTimeout", detail),
        MountError::Invalid => UiError::new("invalidInput", "Ungültige System-Einbindung"),
        MountError::Failed(detail) => UiError::new("systemMountFailed", detail),
        MountError::Unavailable => UiError::new("systemMount", "Für diesen Netzwerkort wird GVfs (gio) oder KIO-FUSE benötigt. Bitte im System-Dateimanager verbinden und erneut versuchen."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dualfile_core::vfs::VPath;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;

    fn entry(name: &str, kind: EntryKind) -> Entry {
        Entry {
            name: name.into(),
            kind,
            size: 12,
            modified: Some(UNIX_EPOCH + Duration::from_millis(1_500)),
            mode: Some(0o644),
            hidden: name.starts_with('.'),
            link_target: None,
            broken_link: false,
            error: None,
        }
    }

    #[test]
    fn verification_error_and_private_lane() {
        let error = UiError::from(Error::VerifyFailed);
        assert_eq!(error.kind, "verifyFailed");
        assert_eq!(
            error.message,
            "Die Prüfsumme stimmt nach dem Übertragen nicht überein"
        );
        let status = JobStatus {
            state: JobState::Paused,
            files_done: 0,
            bytes_done: 0,
            bytes_total: 10,
            bytes_per_second: 0.0,
            report: None,
            eta_seconds: Some(3),
            queue_position: Some(2),
            lane: Some("conn:secret-host".into()),
        };
        let dto = job_status_dto(1, "Copy", &status);
        let json = serde_json::to_value(dto).unwrap();
        assert_eq!(json["state"], "paused");
        assert_eq!(json["etaSeconds"], 3);
        assert_eq!(json["queuePosition"], 2);
        assert_eq!(json["lane"].as_str().unwrap().len(), 16);
        assert!(!json.to_string().contains("secret"));
    }

    #[test]
    fn entry_serializes_with_camel_case_contract_fields() {
        let mut e = entry(".x", EntryKind::Symlink);
        e.link_target = Some("../y".into());
        e.broken_link = true;
        let value = serde_json::to_value(entry_dto(&e)).unwrap();
        assert_eq!(
            value,
            json!({
                "name": ".x", "kind": "symlink", "size": 12, "modified": 1500, "mode": 420,
                "hidden": true, "linkTarget": "../y", "brokenLink": true, "error": null
            })
        );
    }

    #[test]
    fn modified_before_epoch_is_negative_and_unknown_is_null() {
        assert_eq!(millis(UNIX_EPOCH - Duration::from_millis(7)), -7);
        let mut e = entry("a", EntryKind::File);
        e.modified = None;
        assert_eq!(entry_dto(&e).modified, None);
    }

    #[test]
    fn error_kinds_match_the_ui_error_table() {
        let p = || PathBuf::from("/x");
        let cases: Vec<(Error, &str)> = vec![
            (Error::NotFound(p()), "notFound"),
            (Error::Exists(p()), "exists"),
            (Error::PermissionDenied(p()), "permission"),
            (Error::NotADirectory(p()), "notDirectory"),
            (Error::InvalidName("a/b".into()), "invalidName"),
            (Error::InvalidPath("x".into()), "invalidPath"),
            (Error::DestinationInsideSource, "invalidTarget"),
            (Error::SameFile, "invalidTarget"),
            (Error::Disconnected, "disconnected"),
            (Error::SizeMismatch(p()), "sizeMismatch"),
            (Error::Remote("x".into()), "remote"),
            (Error::Cancelled, "cancelled"),
            (Error::IsADirectory(p()), "isADirectory"),
            (Error::DirectoryNotEmpty(p()), "directoryNotEmpty"),
            (Error::ChangedSincePlan(p()), "changedSincePlan"),
            (Error::IncompletePlan, "incompletePlan"),
            (Error::RenameLeftover(p()), "renameLeftover"),
            (Error::Archive("Archiv zu groß für die Anzeige"), "archive"),
            (
                Error::Unsupported(dualfile_core::archive::MSG_READ_ONLY),
                "readOnly",
            ),
            (Error::Unsupported("anderes"), "unsupported"),
            (Error::Unsupported(MSG_BINARY), "unsupported"),
            (Error::Unsupported(MSG_TOO_LARGE), "unsupported"),
        ];
        for (error, kind) in cases {
            assert_eq!(UiError::from(error).kind, kind);
        }
        // The two text-diff refusals keep their sentence as it is.
        assert_eq!(
            UiError::from(Error::Unsupported(MSG_BINARY)).message,
            MSG_BINARY
        );
        assert_eq!(
            UiError::from(Error::Unsupported(MSG_TOO_LARGE)).message,
            MSG_TOO_LARGE
        );
    }

    #[test]
    fn server_errors_have_fixed_texts_without_server_replies() {
        // a reply text that echoes a password must not come out
        let remote = UiError::from(Error::Remote("Login incorrect for u with hunter2".into()));
        assert_eq!(remote.kind, "remote");
        assert_eq!(remote.message, MSG_REMOTE);
        assert!(!remote.message.contains("hunter2"));
        let size = UiError::from(Error::SizeMismatch(PathBuf::from("/a")));
        assert_eq!(size.message, MSG_SIZE_MISMATCH);
    }

    #[test]
    fn connect_errors_map_onto_ui_kinds_with_fixed_texts() {
        use dualfile_remote::error::TransportError as T;
        let kind = |e: ConnectError| connect_error(&e);
        let t = |e: T| kind(ConnectError::Transport(e));
        assert_eq!(t(T::Auth).kind, "auth");
        assert_eq!(t(T::reply(530, "Login incorrect hunter2")).kind, "auth");
        assert_eq!(t(T::Timeout).kind, "timeout");
        assert_eq!(t(T::Trust(TrustError::Expired)).kind, "trust");
        assert_eq!(t(T::Trust(TrustError::InvalidChain)).kind, "trust");
        assert_eq!(
            t(T::Trust(TrustError::AlgorithmNotAllowed("ssh-dss".into()))).kind,
            "algorithmNotAllowed"
        );
        assert_eq!(
            t(T::Trust(TrustError::PinMismatch {
                expected: "a".into(),
                actual: "b".into()
            }))
            .kind,
            "pinMismatch"
        );
        assert_eq!(
            t(T::Connection("host 10.0.0.1 hunter2".into())).kind,
            "remote"
        );
        let reply = t(T::reply(550, "echo hunter2"));
        assert_eq!(reply.kind, "remote");
        assert!(!reply.message.contains("hunter2"));
        let algorithm = t(T::Trust(TrustError::AlgorithmNotAllowed("ssh-dss".into())));
        assert!(!algorithm.message.contains("ssh-dss"));
        assert_eq!(
            kind(ConnectError::TooManyConnections).kind,
            "tooManyConnections"
        );
        assert_eq!(kind(ConnectError::Invalid("x")).kind, "invalidInput");
    }

    #[test]
    fn lister_dtos_have_the_shape_of_the_ui_contract() {
        let value = |r: SearchOutcome| serde_json::to_value(SearchResultDto::from(r)).unwrap();
        assert_eq!(
            value(SearchOutcome::Found(5)),
            serde_json::json!({"kind": "found", "offset": 5})
        );
        assert_eq!(
            value(SearchOutcome::NotFound),
            serde_json::json!({"kind": "notFound"})
        );
        assert_eq!(
            value(SearchOutcome::Continue(9)),
            serde_json::json!({"kind": "continue", "next": 9})
        );
        let dir: DirectionDto = serde_json::from_str("\"backward\"").unwrap();
        assert_eq!(Direction::from(dir), Direction::Backward);
        assert!(serde_json::from_str::<DirectionDto>("\"sideways\"").is_err());
        let info = FileInfo {
            size: 3,
            kind: FileKind::Image,
            modified: None,
            mime_guess: "image/png",
        };
        assert_eq!(
            serde_json::to_value(file_info_dto(&info)).unwrap(),
            serde_json::json!({"size": 3, "kind": "image", "modified": null, "mimeGuess": "image/png"})
        );
        assert_eq!(UiError::from(Error::InvalidInput("x")).kind, "invalidInput");
    }

    #[test]
    fn io_error_message_hides_the_os_error_text() {
        let error = Error::Io {
            op: "Lesen",
            path: PathBuf::from("/tmp/a"),
            source: std::io::Error::from_raw_os_error(5),
        };
        let ui = UiError::from(error);
        assert_eq!(ui.kind, "io");
        assert_eq!(ui.message, "Lesen fehlgeschlagen für /tmp/a");
        assert!(!ui.message.contains("os error"));
    }

    #[test]
    fn job_status_uses_report_for_lists_and_file_count() {
        let mut report = OpReport::default();
        report.done.push(VPath::local("/a").unwrap());
        report.done.push(VPath::local("/b").unwrap());
        report
            .failed
            .push((VPath::local("/c").unwrap(), Error::NotFound("/c".into())));
        report.skipped.push(VPath::local("/d").unwrap());
        report
            .warnings
            .push((VPath::local("/e").unwrap(), "Hinweis".into()));
        let status = JobStatus {
            eta_seconds: Some(3),
            queue_position: None,
            lane: Some("private/path".into()),
            state: JobState::Done,
            files_done: 0,
            bytes_done: 5,
            bytes_total: 9,
            bytes_per_second: f64::NAN,
            report: Some(Arc::new(report)),
        };
        let value = serde_json::to_value(job_status_dto(7, "Papierkorb: a (+1)", &status)).unwrap();
        assert_eq!(value["id"], 7);
        assert_eq!(value["name"], "Papierkorb: a (+1)");
        assert_eq!(value["state"], "done");
        assert_eq!(value["filesDone"], 2);
        assert_eq!(value["bytesTotal"], 9);
        assert_eq!(value["bytesPerSecond"], 0.0);
        assert_eq!(value["failed"][0]["path"], "/c");
        assert!(value["failed"][0]["message"]
            .as_str()
            .unwrap()
            .contains("/c"));
        assert_eq!(value["skipped"], json!(["/d"]));
        assert_eq!(value["warnings"][0]["message"], "Hinweis");
    }

    #[test]
    fn job_without_report_has_empty_lists() {
        let status = JobStatus {
            eta_seconds: Some(3),
            queue_position: None,
            lane: Some("private/path".into()),
            state: JobState::Running,
            files_done: 3,
            bytes_done: 1,
            bytes_total: 2,
            bytes_per_second: 10.0,
            report: None,
        };
        let dto = job_status_dto(1, "Kopieren: x", &status);
        assert_eq!(dto.state, "running");
        assert_eq!(dto.files_done, 3);
        assert!(dto.failed.is_empty() && dto.skipped.is_empty() && dto.warnings.is_empty());
    }

    #[test]
    fn every_job_state_has_the_ui_name() {
        let names: Vec<_> = [
            JobState::Queued,
            JobState::Running,
            JobState::Paused,
            JobState::Done,
            JobState::Failed,
            JobState::Cancelled,
        ]
        .into_iter()
        .map(state_name)
        .collect();
        assert_eq!(
            names,
            ["queued", "running", "paused", "done", "failed", "cancelled"]
        );
    }

    #[test]
    fn conflict_answers_map_to_core_resolutions() {
        use ops::Resolution as R;
        let answer = |json: serde_json::Value| -> R {
            serde_json::from_value::<ConflictAnswerDto>(json)
                .unwrap()
                .to_core()
        };
        assert_eq!(
            answer(json!({"resolution": "overwrite", "applyToAll": false})),
            R::Overwrite
        );
        assert_eq!(
            answer(json!({"resolution": "overwrite", "applyToAll": true})),
            R::OverwriteAll
        );
        assert_eq!(
            answer(json!({"resolution": "skip", "applyToAll": true})),
            R::SkipAll
        );
        assert_eq!(
            answer(json!({"resolution": "keepBoth", "applyToAll": false})),
            R::KeepBoth
        );
        assert_eq!(
            answer(json!({"resolution": "keepBoth", "applyToAll": true})),
            R::KeepBothAll
        );
        assert_eq!(
            answer(json!({"resolution": "cancel", "applyToAll": true})),
            R::Cancel
        );
        assert!(serde_json::from_value::<ConflictAnswerDto>(
            json!({"resolution": "delete", "applyToAll": false})
        )
        .is_err());
    }

    #[test]
    fn conflict_event_has_the_contract_shape() {
        let conflict = Conflict {
            source: entry("a", EntryKind::File),
            destination: entry("a", EntryKind::File),
            source_path: VPath::local("/s/a").unwrap(),
            destination_path: VPath::local("/d/a").unwrap(),
        };
        let event = CoreEventDto::Conflict {
            conflict: conflict_dto(4, &conflict),
        };
        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["type"], "conflict");
        assert_eq!(value["conflict"]["jobId"], 4);
        assert_eq!(value["conflict"]["sourcePath"], "/s/a");
        assert_eq!(value["conflict"]["destinationPath"], "/d/a");
        assert_eq!(value["conflict"]["destination"]["name"], "a");
    }

    #[test]
    fn dir_changed_and_job_events_use_the_type_tag() {
        let value = serde_json::to_value(CoreEventDto::DirChanged { path: "/x".into() }).unwrap();
        assert_eq!(value, json!({"type": "dirChanged", "path": "/x"}));
        let status = JobStatus {
            eta_seconds: Some(3),
            queue_position: None,
            lane: Some("private/path".into()),
            state: JobState::Queued,
            files_done: 0,
            bytes_done: 0,
            bytes_total: 0,
            bytes_per_second: 0.0,
            report: None,
        };
        let value = serde_json::to_value(CoreEventDto::Job {
            status: job_status_dto(1, "n", &status),
        })
        .unwrap();
        assert_eq!(value["type"], "job");
        assert_eq!(value["status"]["state"], "queued");
    }

    #[test]
    fn delete_plan_token_is_fixed_width_hex() {
        assert_eq!(token_text(255), format!("{}ff", "0".repeat(30)));
        let input: DeletePlanInput = serde_json::from_value(
            json!({"token": "ab", "files": 1, "dirs": 0, "links": 0, "bytes": 1, "firstNames": [], "incomplete": false}),
        )
        .unwrap();
        assert_eq!(input.token, "ab");
    }

    #[test]
    fn settings_round_trip_through_the_ui_shape() {
        let mut core = settings::Settings {
            font_size: 15,
            show_parent_entry: false,
            terminal_height: 420,
            terminal_shell: "login".into(),
            terminal_colors: "amber".into(),
            ..settings::Settings::default()
        };
        core.bookmarks.push(settings::Bookmark {
            name: "Doku".into(),
            path: "/home/x/Doku".into(),
            color: "#aabbcc".into(),
            order: 2,
        });
        core.session.windows = vec![settings::WindowSession {
            tabs: vec![settings::Tab {
                path: "/home/x".into(),
                sort: "size-desc".into(),
                filter: "a".into(),
            }],
            active_tab: 0,
        }];
        let dto = SettingsDto::from(&core);
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["fontSize"], 15);
        assert_eq!(value["showParentEntry"], false);
        assert_eq!(value["terminalHeight"], 420);
        assert_eq!(value["terminalShell"], "login");
        assert_eq!(value["terminalColors"], "amber");
        let mut old = value.clone();
        old.as_object_mut().unwrap().remove("terminalColors");
        let old: SettingsDto = serde_json::from_value(old).unwrap();
        assert_eq!(
            (old.terminal_colors.as_str(), old.terminal_bg.as_str()),
            ("green", "#102030")
        );
        let mut legacy = value.clone();
        legacy.as_object_mut().unwrap().remove("showParentEntry");
        assert!(
            serde_json::from_value::<SettingsDto>(legacy)
                .unwrap()
                .show_parent_entry
        );
        assert_eq!(value["session"]["activeWindow"], 0);
        assert_eq!(value["session"]["windows"][0]["activeTab"], 0);
        assert_eq!(value["bookmarks"][0]["color"], "#aabbcc");
        let back: SettingsDto = serde_json::from_value(value).unwrap();
        assert_eq!(settings::Settings::from(&back), core);
    }

    #[test]
    fn load_outcomes_use_the_kind_tag() {
        let value = |o: LoadOutcome| serde_json::to_value(LoadOutcomeDto::from(o)).unwrap();
        assert_eq!(value(LoadOutcome::Fresh), json!({"kind": "fresh"}));
        assert_eq!(value(LoadOutcome::Loaded), json!({"kind": "loaded"}));
        assert_eq!(
            value(LoadOutcome::Repaired(vec!["theme".into()])),
            json!({"kind": "repaired", "messages": ["theme"]})
        );
        assert_eq!(
            value(LoadOutcome::RecoveredFromCorrupt),
            json!({"kind": "recoveredFromCorrupt"})
        );
        assert_eq!(
            value(LoadOutcome::NewerVersion(9)),
            json!({"kind": "newerVersion", "version": 9})
        );
        assert_eq!(
            value(LoadOutcome::Migrated { from: 0, to: 1 }),
            json!({"kind": "migrated", "from": 0, "to": 1})
        );
    }
}

#[cfg(test)]
mod undo_dto_tests {
    use super::*;
    use dualfile_core::undo::UndoError;

    #[test]
    fn undo_errors_are_fixed_codes_without_path_or_system_text() {
        for (error, code) in [
            (UndoError::Changed, "changed"),
            (UndoError::Conflict, "conflict"),
            (UndoError::AlreadyUndone, "alreadyUndone"),
            (UndoError::NotFound, "notFound"),
            (UndoError::Unsupported, "unsupported"),
            (UndoError::NotLocal, "notLocal"),
            (UndoError::Io, "io"),
        ] {
            let dto = UndoResultDto::from_report(Err(error));
            assert_eq!(dto.errors, vec![code]);
            let json = serde_json::to_value(dto).unwrap();
            assert_eq!(json["errors"], serde_json::json!([code]));
            assert!(json.get("path").is_none());
            assert!(json.get("message").is_none());
        }
    }
}

#[cfg(test)]
mod scan_tests {
    #[test]
    fn scan_errors_have_fixed_codes() {
        use dualfile_core::usage::ScanIssue;
        for (issue, expected) in [
            (ScanIssue::Io(std::io::ErrorKind::PermissionDenied), "io"),
            (ScanIssue::Changed, "changed"),
            (ScanIssue::DepthLimit, "depthLimit"),
            (ScanIssue::EntryLimit, "entryLimit"),
            (ScanIssue::FileSystemBoundary, "fileSystemBoundary"),
        ] {
            assert_eq!(super::scan_issue(issue), expected);
        }
    }
}

#[cfg(test)]
mod similar_tests {
    #[test]
    fn decoder_errors_are_stable() {
        use dualfile_core::usage::ScanIssue;
        assert_eq!(
            super::similar_issue(ScanIssue::Io(std::io::ErrorKind::InvalidData)),
            "invalidImage"
        );
        assert_eq!(
            super::similar_issue(ScanIssue::Io(std::io::ErrorKind::FileTooLarge)),
            "imageTooLarge"
        );
        assert_eq!(super::similar_issue(ScanIssue::Changed), "changed");
    }
}

#[cfg(test)]
mod system_mount_tests {
    use super::system_mount_error;
    use dualfile_remote::system_mount::MountError;

    #[test]
    fn missing_helper_failure_and_timeout_have_distinct_codes() {
        assert_eq!(
            system_mount_error(MountError::Unavailable).kind,
            "systemMount"
        );
        let error = system_mount_error(MountError::Failed("Host unreachable".into()));
        assert_eq!(error.kind, "systemMountFailed");
        assert_eq!(error.message, "Host unreachable");
        assert_eq!(
            system_mount_error(MountError::Timeout(String::new())).kind,
            "systemMountTimeout"
        );
    }
}

#[cfg(test)]
mod smb_tests {
    use super::{connect_error, error_kind};
    use dualfile_core::Error;
    use dualfile_remote::{error::TransportError, manager::ConnectError};
    #[test]
    fn smb_failure_kinds_are_fixed_and_localizable() {
        for (error, key) in [
            (TransportError::SmbAuth, "smbAuth"),
            (TransportError::SmbSecurity, "smbSecurity"),
            (TransportError::SmbUnreachable, "smbUnreachable"),
            (TransportError::SmbShareMissing, "smbShareMissing"),
        ] {
            assert_eq!(error_kind(&Error::Remote(error.to_string())), key);
            assert_eq!(connect_error(&ConnectError::Transport(error)).kind, key);
        }
    }
}
