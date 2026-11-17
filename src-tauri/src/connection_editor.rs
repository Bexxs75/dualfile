//! The connection-profile editor as its own OS window. The window loads the same web app
//! (`index.html?view=connection-editor&...`); the appearance travels in the URL because the
//! editor never loads (and so never risks rewriting) the settings file.

use crate::convert::UiError;
use serde::Deserialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::{Manager, Url, WebviewUrl, WebviewWindowBuilder};

pub(crate) const EDITOR_LABEL: &str = "connection-editor";
const MAX_TITLE_CHARS: usize = 120;
const MAX_ID_BYTES: usize = 128;
const THEMES: [&str; 3] = ["schiefer", "papier", "werkstatt"];
const DENSITIES: [&str; 3] = ["compact", "normal", "comfortable"];

/// Held across "does it exist?" and "build it", so two quick clicks cannot both create it.
static OPEN_LOCK: Mutex<()> = Mutex::new(());
/// Set once the main window is going away; a window built after that would keep the process alive.
static MAIN_SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

pub(crate) fn mark_main_closing() {
    MAIN_SHUTTING_DOWN.store(true, Ordering::SeqCst);
}

/// Look and language of the main window at the moment the editor opens.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appearance {
    pub theme: String,
    pub accent: String,
    pub accent_automatic: bool,
    pub density: String,
    pub font_size: f64,
    pub lang: String,
}

fn invalid(message: &str) -> UiError {
    UiError::new("invalidInput", message)
}

pub(crate) fn only_label(label: &str, wanted: &str) -> Result<(), UiError> {
    if label == wanted {
        Ok(())
    } else {
        Err(UiError::new(
            "unsupported",
            "Verbindungseditor nicht verfügbar",
        ))
    }
}

/// Window title: control characters dropped, length capped.
pub(crate) fn sanitize_title(title: &str) -> String {
    let clean: String = title
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_TITLE_CHARS)
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        "Dualfile".to_owned()
    } else {
        clean.to_owned()
    }
}

fn is_hex_color(text: &str) -> bool {
    text.len() == 7 && text.starts_with('#') && text[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// Path and query of the editor page. Every value is validated here: the query is read back
/// by the page, so nothing unchecked may end up in it.
pub(crate) fn editor_query(
    profile_id: Option<&str>,
    appearance: &Appearance,
) -> Result<String, UiError> {
    if let Some(id) = profile_id {
        if id.is_empty() || id.len() > MAX_ID_BYTES || id.chars().any(|c| c.is_control()) {
            return Err(invalid("Ungültige Profilkennung"));
        }
    }
    let mut url = Url::parse("http://localhost/").map_err(|_| UiError::internal())?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("view", EDITOR_LABEL);
        if let Some(id) = profile_id {
            query.append_pair("profile", id);
        }
    }
    append_appearance(&mut url, appearance)?;
    Ok(format!("index.html?{}", url.query().unwrap_or_default()))
}

pub(crate) fn append_appearance(url: &mut Url, appearance: &Appearance) -> Result<(), UiError> {
    if !THEMES.contains(&appearance.theme.as_str())
        || !DENSITIES.contains(&appearance.density.as_str())
        || !["de", "en"].contains(&appearance.lang.as_str())
        || !is_hex_color(&appearance.accent)
        || !appearance.font_size.is_finite()
        || !(1.0..=200.0).contains(&appearance.font_size)
    {
        return Err(invalid("Ungültige Darstellung"));
    }
    let mut query = url.query_pairs_mut();
    query
        .append_pair("theme", &appearance.theme)
        .append_pair("accent", &appearance.accent)
        .append_pair(
            "accentAutomatic",
            if appearance.accent_automatic {
                "1"
            } else {
                "0"
            },
        )
        .append_pair("density", &appearance.density)
        .append_pair("fontSize", &appearance.font_size.to_string())
        .append_pair("lang", &appearance.lang);
    Ok(())
}

pub(crate) fn focus_existing(app: &tauri::AppHandle, label: &str) -> bool {
    match app.get_webview_window(label) {
        Some(existing) => {
            crate::system::bring_to_front(&existing);
            true
        }
        None => false,
    }
}

/// Opens the editor window. `false` = it was already open (now in front); the main window
/// then tells the user instead of silently switching profiles under their hands.
#[tauri::command]
pub async fn connection_editor_open(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    profile_id: Option<String>,
    title: String,
    appearance: Appearance,
) -> Result<bool, UiError> {
    only_label(window.label(), "main")?;
    let page = editor_query(profile_id.as_deref(), &appearance)?;
    open_child(
        &app,
        &window,
        EDITOR_LABEL,
        page,
        title,
        (520.0, 640.0),
        (400.0, 320.0),
    )
}

pub(crate) fn open_child(
    app: &tauri::AppHandle,
    window: &tauri::WebviewWindow,
    label: &str,
    page: String,
    title: String,
    size: (f64, f64),
    min: (f64, f64),
) -> Result<bool, UiError> {
    let _open = OPEN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if MAIN_SHUTTING_DOWN.load(Ordering::SeqCst) {
        return Ok(false);
    }
    if focus_existing(app, label) {
        return Ok(false);
    }
    let builder = WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(sanitize_title(&title))
        .inner_size(size.0, size.1)
        .min_inner_size(min.0, min.1)
        .resizable(true)
        .focused(true);
    // Transient for the main window: stays above it and follows it when minimized, without
    // being modal. No position is set (Wayland compositors place windows themselves).
    #[cfg(target_os = "linux")]
    let builder = builder
        .transient_for(window)
        .map_err(|_| UiError::new("io", "Fenster konnte nicht geöffnet werden"))?;
    match builder.build() {
        Ok(created) => {
            // The main window may have been destroyed while this one was being built.
            if MAIN_SHUTTING_DOWN.load(Ordering::SeqCst) {
                let _ = created.destroy();
                return Ok(false);
            }
            Ok(true)
        }
        Err(
            tauri::Error::WebviewLabelAlreadyExists(_) | tauri::Error::WindowLabelAlreadyExists(_),
        ) => {
            focus_existing(app, label);
            Ok(false)
        }
        Err(_) => Err(UiError::new("io", "Fenster konnte nicht geöffnet werden")),
    }
}

/// Closes the calling window, but only if it is the editor (no blanket close permission).
#[tauri::command]
pub async fn connection_editor_close(window: tauri::WebviewWindow) -> Result<(), UiError> {
    only_label(window.label(), EDITOR_LABEL)?;
    window
        .close()
        .map_err(|_| UiError::new("io", "Fenster konnte nicht geschlossen werden"))
}

/// Destroys the editor together with the main window (it would keep the process alive).
pub(crate) fn destroy_children(app: &tauri::AppHandle) {
    for label in [EDITOR_LABEL, crate::settings_window::SETTINGS_LABEL] {
        if let Some(child) = app.get_webview_window(label) {
            let _ = child.destroy();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn appearance() -> Appearance {
        Appearance {
            theme: "papier".into(),
            accent: "#2f6f6a".into(),
            accent_automatic: false,
            density: "normal".into(),
            font_size: 13.0,
            lang: "de".into(),
        }
    }

    #[test]
    fn only_the_named_window_passes() {
        assert!(only_label("main", "main").is_ok());
        assert!(only_label(EDITOR_LABEL, EDITOR_LABEL).is_ok());
        for label in ["", "MAIN", EDITOR_LABEL, "preview"] {
            assert!(only_label(label, "main").is_err());
        }
        assert!(only_label("main", EDITOR_LABEL).is_err());
        assert!(only_label("", EDITOR_LABEL).is_err());
    }

    #[test]
    fn query_encodes_the_profile_id() {
        let page = editor_query(Some("a b&c=d#e?f/é"), &appearance()).unwrap();
        assert!(page.starts_with("index.html?view=connection-editor&profile="));
        let query = page.trim_start_matches("index.html?");
        // Only the separators of the query itself may stay raw.
        let profile = query
            .split('&')
            .find_map(|p| p.strip_prefix("profile="))
            .unwrap();
        assert!(!profile.contains(['&', '#', '?', '=', '/', ' ']));
        let parsed = Url::parse(&format!("http://localhost/?{query}")).unwrap();
        let back: Vec<_> = parsed.query_pairs().collect();
        assert!(back
            .iter()
            .any(|(k, v)| k == "profile" && v == "a b&c=d#e?f/é"));
    }

    #[test]
    fn query_without_profile_is_a_new_profile() {
        let page = editor_query(None, &appearance()).unwrap();
        assert!(!page.contains("profile="));
        assert!(page.contains("theme=papier"));
        assert!(page.contains("accent=%232f6f6a"));
        assert!(page.contains("accentAutomatic=0"));
        assert!(page.contains("fontSize=13"));
        assert!(page.contains("lang=de"));
    }

    #[test]
    fn query_rejects_bad_input() {
        let ok = appearance();
        assert!(editor_query(Some(""), &ok).is_err());
        assert!(editor_query(Some(&"x".repeat(MAX_ID_BYTES + 1)), &ok).is_err());
        assert!(editor_query(Some("a\nb"), &ok).is_err());
        for bad in [
            Appearance {
                theme: "x".into(),
                ..ok.clone()
            },
            Appearance {
                density: "x".into(),
                ..ok.clone()
            },
            Appearance {
                lang: "fr".into(),
                ..ok.clone()
            },
            Appearance {
                accent: "red".into(),
                ..ok.clone()
            },
            Appearance {
                accent: "#12345g".into(),
                ..ok.clone()
            },
            Appearance {
                font_size: f64::NAN,
                ..ok.clone()
            },
            Appearance {
                font_size: f64::INFINITY,
                ..ok.clone()
            },
            Appearance {
                font_size: 0.0,
                ..ok.clone()
            },
        ] {
            assert!(editor_query(None, &bad).is_err());
        }
    }

    #[test]
    fn title_is_cleaned_and_capped() {
        assert_eq!(sanitize_title("Profil\n\u{7}x"), "Profilx");
        assert_eq!(sanitize_title("  \n "), "Dualfile");
        assert_eq!(
            sanitize_title(&"ä".repeat(500)).chars().count(),
            MAX_TITLE_CHARS
        );
    }

    #[test]
    fn editor_capability_is_narrow() {
        let text = include_str!("../capabilities/connection-editor.json");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["windows"], serde_json::json!([EDITOR_LABEL]));
        let permissions: Vec<&str> = json["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap())
            .collect();
        assert_eq!(
            permissions,
            [
                "allow-connections-list",
                "allow-connection-save",
                "allow-ssh-hosts-list",
                "allow-ssh-host-resolve",
                "allow-connection-editor-close",
            ]
        );
    }

    #[test]
    fn only_main_may_open_the_editor() {
        let text = include_str!("../capabilities/default.json");
        let json: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(json["windows"], serde_json::json!(["main"]));
        let all = json["permissions"].as_array().unwrap();
        assert!(all.iter().any(|p| p == "allow-connection-editor-open"));
        assert!(!all.iter().any(|p| p == "allow-connection-editor-close"));
    }
}
