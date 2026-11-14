use gtk::prelude::*;
use gtk::{Application, ApplicationWindow, Box as UiBox, Button, Entry, Label, Orientation, ScrolledWindow};
use std::{cell::RefCell, fs, io::Write, path::{Path, PathBuf}, process::{Command, Stdio}, rc::Rc, time::{Duration, SystemTime, UNIX_EPOCH}};

#[derive(Clone)]
struct Widgets { rows: UiBox, tabs: UiBox, address: Entry, search: Entry, count: Label }
#[derive(Clone)]
struct Pane { path: PathBuf, tabs: Vec<PathBuf>, current_tab: usize, selected: Vec<PathBuf>, hidden: bool }
struct State { panes: [Pane; 2], widgets: [Widgets; 2], active: usize, message: Label, window: ApplicationWindow, mounts: Rc<RefCell<Vec<PathBuf>>> }
type Shared = Rc<RefCell<State>>;

fn main() {
    let shot = std::env::args().collect::<Vec<_>>().windows(2)
        .find(|w| w[0] == "--screenshot").map(|w| PathBuf::from(&w[1]));
    // A display server is unavailable in some build checks. Cairo can still
    // render the same initial file-manager view to a PNG in that case.
    if let Some(path) = shot.as_ref() {
        if gtk::init().is_err() {
            if let Err(error) = save_headless_screenshot(path) {
                eprintln!("Could not save screenshot: {error}");
                std::process::exit(1);
            }
            return;
        }
    }
    // A screenshot request must activate this process even when the file
    // manager is already open; otherwise GTK forwards it to that instance.
    let flags = if shot.is_some() {
        gtk::gio::ApplicationFlags::NON_UNIQUE
    } else {
        gtk::gio::ApplicationFlags::FLAGS_NONE
    };
    let app = Application::new(Some("org.example.dualfile"), flags);
    app.connect_activate(move |app| build(app, shot.clone()));
    app.run_with_args(&["dualfile"]);
}

fn build(app: &Application, shot: Option<PathBuf>) {
    let css = gtk::CssProvider::new();
    css.load_from_data(include_bytes!("style.css")).unwrap();
    gtk::StyleContext::add_provider_for_screen(&gdk::Screen::default().unwrap(), &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
    let window = ApplicationWindow::new(app);
    window.set_title("Dualfile — Dateimanager");
    window.set_default_size(1360, 800);
    if shot.is_some() { window.set_accept_focus(false); window.set_focus_on_map(false); }
    let outer = UiBox::new(Orientation::Vertical, 0); outer.style_context().add_class("app"); window.add(&outer);
    let header = UiBox::new(Orientation::Horizontal, 12); header.style_context().add_class("header");
    let brand = Label::new(Some("DUALFILE")); brand.style_context().add_class("brand"); header.pack_start(&brand, false, false, 0);
    let subtitle = Label::new(Some("Dateien im Blick. Wege kurz.")); subtitle.style_context().add_class("sub"); header.pack_start(&subtitle, false, false, 0);
    let gap = Label::new(None); header.pack_start(&gap, true, true, 0);
    let hint = Label::new(Some("Zwei Orte · ein Arbeitsfluss")); hint.style_context().add_class("hint"); header.pack_end(&hint, false, false, 0);
    outer.pack_start(&header, false, false, 0);
    let body = UiBox::new(Orientation::Horizontal, 0); outer.pack_start(&body, true, true, 0);
    let side = UiBox::new(Orientation::Vertical, 8); side.set_size_request(200, -1); side.style_context().add_class("sidebar");
    let places = Label::new(Some("ORTE")); places.set_xalign(0.0); places.style_context().add_class("sidehead"); side.pack_start(&places, false, false, 0);
    body.pack_start(&side, false, false, 0);
    let main = UiBox::new(Orientation::Vertical, 0); body.pack_start(&main, true, true, 0);
    let toolbar = UiBox::new(Orientation::Horizontal, 7); toolbar.style_context().add_class("toolbar"); main.pack_start(&toolbar, false, false, 0);
    let panes_box = UiBox::new(Orientation::Horizontal, 12); panes_box.style_context().add_class("panes"); main.pack_start(&panes_box, true, true, 0);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let downloads = home.join("Downloads");
    let right = if downloads.is_dir() { downloads } else { home.clone() };
    let mut widgets = Vec::new();
    for i in 0..2 { let (panel, w) = pane_ui(i); panes_box.pack_start(&panel, true, true, 0); widgets.push(w); }
    let footer = UiBox::new(Orientation::Horizontal, 10); footer.style_context().add_class("footer");
    let message = Label::new(Some("Bereit. Wähle Dateien aus und nutze die Aktionen oben.")); message.set_xalign(0.0); footer.pack_start(&message, true, true, 0);
    let keys = Label::new(Some("F3 Anzeigen   F4 Bearbeiten   F5 Kopieren   F6 Verschieben   F7 Ordner   F8 Papierkorb")); keys.style_context().add_class("keys"); footer.pack_end(&keys, false, false, 0);
    outer.pack_end(&footer, false, false, 0);
    let mounts = Rc::new(RefCell::new(Vec::<PathBuf>::new()));
    let state = Rc::new(RefCell::new(State {
        panes: [Pane { path: home.clone(), tabs: vec![home.clone()], current_tab: 0, selected: vec![], hidden: false },
                Pane { path: right.clone(), tabs: vec![right], current_tab: 0, selected: vec![], hidden: false }],
        widgets: [widgets[0].clone(), widgets[1].clone()], active: 0, message, window: window.clone(), mounts: mounts.clone()
    }));
    app.connect_shutdown(move |_| {
        for path in mounts.borrow().iter() {
            if Command::new("fusermount3").arg("-u").arg(path).status().map(|s| s.success()).unwrap_or(false) {
                let _ = fs::remove_dir(path);
            }
        }
    });
    for (title, path) in [
        ("Persönlicher Ordner", home.clone()), ("Downloads", home.join("Downloads")),
        ("Dokumente", home.join("Dokumente")), ("Bilder", home.join("Bilder")),
        ("Schreibtisch", home.join("Schreibtisch")), ("Dateisystem", PathBuf::from("/"))
    ] {
        if !path.is_dir() { continue; }
        let b = Button::with_label(title); b.style_context().add_class("place"); side.pack_start(&b, false, false, 0);
        let s = state.clone(); b.connect_clicked(move |_| { let i = s.borrow().active; navigate(&s, i, path.clone()); });
    }
    let network = Label::new(Some("NETZWERK")); network.set_xalign(0.0);
    network.style_context().add_class("sidehead"); network.style_context().add_class("network-head");
    side.pack_start(&network, false, false, 0);
    for (title, protocol) in [("SSH-Terminal", "ssh"), ("SFTP verbinden", "sftp"), ("SMB verbinden", "smb")] {
        let button = Button::with_label(title); button.style_context().add_class("place");
        side.pack_start(&button, false, false, 0);
        let s = state.clone(); button.connect_clicked(move |_| connect_network(&s, protocol));
    }
    let side_note = Label::new(Some("Tipp: Mit Strg mehrere\nDateien auswählen.")); side_note.set_xalign(0.0);
    side_note.style_context().add_class("sidenote"); side.pack_end(&side_note, false, false, 0);
    let hidden = Button::with_label("Versteckte Dateien zeigen");
    hidden.style_context().add_class("place"); side.pack_end(&hidden, false, false, 0);
    let s = state.clone();
    hidden.connect_clicked(move |b| {
        let (i, visible) = { let mut st = s.borrow_mut(); let i = st.active; st.panes[i].hidden = !st.panes[i].hidden; (i, st.panes[i].hidden) };
        b.set_label(if visible { "Versteckte ausblenden" } else { "Versteckte Dateien zeigen" });
        render(&s, i);
    });
    for (label, action) in [
        ("↑  Übergeordnet", "up"), ("⌂  Start", "home"), ("⟳  Aktualisieren", "refresh"),
        ("⧉  Kopieren", "copy"), ("↪  Verschieben", "move"), ("✎  Umbenennen", "rename"),
        ("＋  Ordner", "mkdir"), ("▤  Anzeigen", "view"), ("✎  Bearbeiten", "edit"), ("⌫  Papierkorb", "trash")
    ] {
        let b = Button::with_label(label);
        b.style_context().add_class(if action == "copy" || action == "move" { "primary" } else { "action" });
        toolbar.pack_start(&b, false, false, 0);
        let s = state.clone(); b.connect_clicked(move |_| perform(&s, action));
    }
    for i in 0..2 {
        let s = state.clone(); let entry = s.borrow().widgets[i].address.clone();
        entry.connect_activate(move |e| navigate(&s, i, PathBuf::from(e.text().as_str())));
        let s = state.clone(); let search = s.borrow().widgets[i].search.clone();
        search.connect_changed(move |_| render(&s, i));
    }
    let s = state.clone();
    window.connect_key_press_event(move |_, e| {
        let k = e.keyval();
        let a = if k == gdk::keys::constants::F3 { Some("view") }
            else if k == gdk::keys::constants::F4 { Some("edit") }
            else if k == gdk::keys::constants::F5 { Some("copy") }
            else if k == gdk::keys::constants::F6 { Some("move") }
            else if k == gdk::keys::constants::F7 { Some("mkdir") }
            else if k == gdk::keys::constants::F8 { Some("trash") }
            else if k == gdk::keys::constants::F2 { Some("rename") } else { None };
        if let Some(a) = a { perform(&s, a); glib::Propagation::Stop } else { glib::Propagation::Proceed }
    });
    render(&state, 0); render(&state, 1); window.show_all();
    if let Some(path) = shot {
        let frames = Rc::new(RefCell::new(0));
        glib::timeout_add_local(Duration::from_millis(16), move || {
            *frames.borrow_mut() += 1; window.queue_draw();
            if *frames.borrow() < 120 { return glib::ControlFlow::Continue; }
            let width = window.allocated_width().max(1);
            let height = window.allocated_height().max(1);
            let result = (|| -> Result<(), Box<dyn std::error::Error>> {
                let surface = gtk::cairo::ImageSurface::create(gtk::cairo::Format::ARgb32, width, height)?;
                let context = gtk::cairo::Context::new(&surface)?;
                window.draw(&context);
                surface.flush();
                write_png(&surface, &path)
            })();
            if let Err(error) = result {
                eprintln!("Could not save screenshot: {error}");
                std::process::exit(1);
            }
            window.close(); glib::ControlFlow::Break
        });
    }
}

fn write_png(surface: &gtk::cairo::ImageSurface, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() { fs::create_dir_all(parent)?; }
    let mut file = fs::File::create(path)?;
    surface.write_to_png(&mut file)?;
    Ok(())
}

fn save_headless_screenshot(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use gtk::cairo::{Context, FontSlant, FontWeight, Format, ImageSurface};

    let surface = ImageSurface::create(Format::ARgb32, 1360, 800)?;
    let cr = Context::new(&surface)?;
    let fill = |cr: &Context, x: f64, y: f64, w: f64, h: f64, color: (f64, f64, f64)| {
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.rectangle(x, y, w, h);
        let _ = cr.fill();
    };
    let label = |cr: &Context, x: f64, y: f64, text: &str, size: f64, bold: bool, color: (f64, f64, f64)| {
        cr.select_font_face("sans", FontSlant::Normal, if bold { FontWeight::Bold } else { FontWeight::Normal });
        cr.set_font_size(size);
        cr.set_source_rgb(color.0, color.1, color.2);
        cr.move_to(x, y);
        let _ = cr.show_text(text);
    };
    let dark = (0.067, 0.176, 0.227);
    let ink = (0.10, 0.20, 0.25);
    let muted = (0.39, 0.51, 0.55);
    let blue = (0.08, 0.44, 0.53);
    fill(&cr, 0.0, 0.0, 1360.0, 800.0, (0.929, 0.949, 0.961));
    fill(&cr, 0.0, 0.0, 1360.0, 58.0, dark);
    label(&cr, 22.0, 36.0, "DUALFILE", 17.0, true, (0.96, 0.98, 0.98));
    label(&cr, 150.0, 35.0, "Dateien im Blick. Wege kurz.", 12.0, false, (0.67, 0.78, 0.82));
    label(&cr, 1165.0, 35.0, "Zwei Orte · ein Arbeitsfluss", 12.0, false, (0.56, 0.70, 0.74));
    fill(&cr, 0.0, 58.0, 200.0, 707.0, (0.89, 0.93, 0.94));
    label(&cr, 22.0, 95.0, "ORTE", 11.0, true, muted);
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"));
    let mut y = 132.0;
    for (name, dir) in [
        ("Persönlicher Ordner", home.clone()), ("Downloads", home.join("Downloads")),
        ("Dokumente", home.join("Dokumente")), ("Bilder", home.join("Bilder")),
        ("Schreibtisch", home.join("Schreibtisch")), ("Dateisystem", PathBuf::from("/")),
    ] {
        if dir.is_dir() { label(&cr, 23.0, y, name, 13.0, false, ink); y += 43.0; }
    }
    y += 14.0;
    label(&cr, 22.0, y, "NETZWERK", 11.0, true, muted);
    for name in ["SSH-Terminal", "SFTP verbinden", "SMB verbinden"] {
        y += 42.0;
        label(&cr, 23.0, y, name, 13.0, false, ink);
    }
    label(&cr, 22.0, 692.0, "Versteckte Dateien zeigen", 12.0, false, ink);
    label(&cr, 22.0, 727.0, "Tipp: Mit Strg mehrere", 11.0, false, muted);
    label(&cr, 22.0, 743.0, "Dateien auswählen.", 11.0, false, muted);

    fill(&cr, 200.0, 58.0, 1160.0, 58.0, (0.97, 0.98, 0.99));
    let actions = ["↑  Übergeordnet", "⌂  Start", "⟳  Aktualisieren", "⧉  Kopieren", "↪  Verschieben", "✎  Umbenennen", "＋  Ordner", "▤  Anzeigen", "✎  Bearbeiten", "⌫  Papierkorb"];
    let mut x = 215.0;
    for action in actions {
        let width = (action.chars().count() as f64 * 6.7 + 22.0).max(76.0);
        let primary = action.contains("Kopieren") || action.contains("Verschieben");
        fill(&cr, x, 70.0, width, 34.0, if primary { (0.09, 0.43, 0.51) } else { (1.0, 1.0, 1.0) });
        label(&cr, x + 8.0, 92.0, action, 11.0, primary, if primary { (1.0, 1.0, 1.0) } else { ink });
        x += width + 7.0;
        if x > 1330.0 { break; }
    }

    let right = if home.join("Downloads").is_dir() { home.join("Downloads") } else { home.clone() };
    for (index, dir) in [home, right].iter().enumerate() {
        let left = 217.0 + index as f64 * 572.0;
        fill(&cr, left, 133.0, 555.0, 611.0, (1.0, 1.0, 1.0));
        fill(&cr, left, 133.0, 555.0, 46.0, (0.96, 0.98, 0.98));
        label(&cr, left + 16.0, 161.0, if index == 0 { "LINKS" } else { "RECHTS" }, 11.0, true, blue);
        let mut entries = fs::read_dir(dir).map(|it| it.filter_map(Result::ok).filter(|e| !e.file_name().to_string_lossy().starts_with('.')).collect::<Vec<_>>()).unwrap_or_default();
        entries.sort_by(|a, b| {
            let ad = a.file_type().map(|f| f.is_dir()).unwrap_or(false);
            let bd = b.file_type().map(|f| f.is_dir()).unwrap_or(false);
            bd.cmp(&ad).then_with(|| a.file_name().to_string_lossy().to_lowercase().cmp(&b.file_name().to_string_lossy().to_lowercase()))
        });
        label(&cr, left + 458.0, 161.0, &format!("{} Einträge", entries.len()), 11.0, false, muted);
        fill(&cr, left, 179.0, 555.0, 39.0, (0.93, 0.96, 0.97));
        let tab = dir.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
        label(&cr, left + 18.0, 203.0, &tab, 12.0, true, blue);
        label(&cr, left + 170.0, 203.0, "+", 16.0, true, muted);
        fill(&cr, left + 14.0, 231.0, 527.0, 37.0, (0.98, 0.99, 1.0));
        label(&cr, left + 25.0, 255.0, &dir.to_string_lossy(), 12.0, false, ink);
        fill(&cr, left + 14.0, 277.0, 527.0, 34.0, (0.97, 0.98, 0.99));
        label(&cr, left + 25.0, 299.0, "In diesem Ordner filtern …", 12.0, false, muted);
        fill(&cr, left, 324.0, 555.0, 35.0, (0.96, 0.98, 0.98));
        label(&cr, left + 18.0, 346.0, "NAME", 10.0, true, muted);
        label(&cr, left + 488.0, 346.0, "GRÖSSE", 10.0, true, muted);
        for (n, entry) in entries.iter().take(10).enumerate() {
            let yy = 359.0 + n as f64 * 36.0;
            let is_dir = entry.file_type().map(|f| f.is_dir()).unwrap_or(false);
            fill(&cr, left, yy + 35.0, 555.0, 1.0, (0.93, 0.95, 0.96));
            label(&cr, left + 17.0, yy + 24.0, if is_dir { "▣" } else { "▤" }, 15.0, false, blue);
            let name = entry.file_name().to_string_lossy().to_string();
            let short = name.chars().take(43).collect::<String>();
            label(&cr, left + 43.0, yy + 23.0, &short, 12.0, false, ink);
            if is_dir { label(&cr, left + 476.0, yy + 23.0, "Ordner", 11.0, false, muted); }
        }
    }
    fill(&cr, 0.0, 765.0, 1360.0, 35.0, dark);
    label(&cr, 18.0, 787.0, "Bereit. Wähle Dateien aus und nutze die Aktionen oben.", 12.0, false, (0.84, 0.91, 0.93));
    label(&cr, 719.0, 787.0, "F3 Anzeigen   F4 Bearbeiten   F5 Kopieren   F6 Verschieben   F7 Ordner   F8 Papierkorb", 11.0, false, (0.57, 0.73, 0.77));
    surface.flush();
    write_png(&surface, path)
}

fn pane_ui(i: usize) -> (UiBox, Widgets) {
    let panel = UiBox::new(Orientation::Vertical, 0); panel.style_context().add_class("pane");
    let top = UiBox::new(Orientation::Horizontal, 8); top.style_context().add_class("panetop");
    let title = Label::new(Some(if i == 0 { "LINKS" } else { "RECHTS" })); title.style_context().add_class("panetitle"); top.pack_start(&title, false, false, 0);
    let count = Label::new(None); count.style_context().add_class("count"); top.pack_end(&count, false, false, 0);
    panel.pack_start(&top, false, false, 0);
    let tabs = UiBox::new(Orientation::Horizontal, 4); tabs.style_context().add_class("tabs");
    panel.pack_start(&tabs, false, false, 0);
    let address = Entry::new(); address.set_placeholder_text(Some("Pfad eingeben …")); address.style_context().add_class("address"); panel.pack_start(&address, false, false, 0);
    let search = Entry::new(); search.set_placeholder_text(Some("In diesem Ordner filtern …")); search.style_context().add_class("search"); panel.pack_start(&search, false, false, 0);
    let columns = UiBox::new(Orientation::Horizontal, 0); columns.style_context().add_class("columns");
    let name = Label::new(Some("NAME")); name.set_xalign(0.0); columns.pack_start(&name, true, true, 0);
    columns.pack_end(&Label::new(Some("GRÖSSE")), false, false, 0); panel.pack_start(&columns, false, false, 0);
    let scroll = ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_policy(gtk::PolicyType::Automatic, gtk::PolicyType::Automatic);
    let rows = UiBox::new(Orientation::Vertical, 0); scroll.add(&rows); panel.pack_start(&scroll, true, true, 0);
    (panel, Widgets { rows, tabs, address, search, count })
}

fn render(shared: &Shared, i: usize) {
    let (pane, w) = { let s = shared.borrow(); (s.panes[i].clone(), s.widgets[i].clone()) };
    for child in w.tabs.children() { w.tabs.remove(&child); }
    for (tab_index, tab_path) in pane.tabs.iter().enumerate() {
        let title = tab_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "/".into());
        let group = UiBox::new(Orientation::Horizontal, 0); group.style_context().add_class("tabgroup");
        let b = Button::with_label(&title); b.style_context().add_class("tab");
        if tab_index == pane.current_tab { b.style_context().add_class("current"); }
        group.pack_start(&b, false, false, 0);
        let s = shared.clone();
        b.connect_clicked(move |_| {
            let path = {
                let mut st = s.borrow_mut(); st.panes[i].current_tab = tab_index;
                st.panes[i].selected.clear(); st.active = i; st.panes[i].tabs[tab_index].clone()
            };
            navigate(&s, i, path);
        });
        if pane.tabs.len() > 1 {
            let close = Button::with_label("×"); close.style_context().add_class("tab-close");
            group.pack_start(&close, false, false, 0);
            let s = shared.clone();
            close.connect_clicked(move |_| {
                let path = {
                    let mut st = s.borrow_mut(); let p = &mut st.panes[i];
                    p.tabs.remove(tab_index);
                    if p.current_tab >= p.tabs.len() { p.current_tab = p.tabs.len() - 1; }
                    else if tab_index < p.current_tab { p.current_tab -= 1; }
                    p.tabs[p.current_tab].clone()
                };
                navigate(&s, i, path);
            });
        }
        w.tabs.pack_start(&group, false, false, 0);
    }
    let add = Button::with_label("+"); add.style_context().add_class("tab-add"); w.tabs.pack_start(&add, false, false, 0);
    let s = shared.clone();
    add.connect_clicked(move |_| {
        { let mut st = s.borrow_mut(); let path = st.panes[i].path.clone();
          st.panes[i].tabs.push(path); st.panes[i].current_tab = st.panes[i].tabs.len() - 1; st.active = i; }
        render(&s, i);
    });
    w.tabs.show_all();
    w.address.set_text(&pane.path.to_string_lossy());
    for child in w.rows.children() { w.rows.remove(&child); }
    let query = w.search.text().to_lowercase();
    let mut entries = match fs::read_dir(&pane.path) {
        Ok(list) => list.filter_map(Result::ok).filter(|e| {
            let n = e.file_name().to_string_lossy().to_string();
            (pane.hidden || !n.starts_with('.')) && n.to_lowercase().contains(query.as_str())
        }).collect::<Vec<_>>(),
        Err(e) => {
            let label = Label::new(Some(&format!("Ordner kann nicht geöffnet werden: {e}")));
            label.style_context().add_class("empty"); w.rows.pack_start(&label, false, false, 0);
            w.rows.show_all(); return;
        }
    };
    entries.sort_by(|a, b| {
        let ad = a.file_type().map(|f| f.is_dir()).unwrap_or(false);
        let bd = b.file_type().map(|f| f.is_dir()).unwrap_or(false);
        bd.cmp(&ad).then_with(|| a.file_name().to_string_lossy().to_lowercase().cmp(&b.file_name().to_string_lossy().to_lowercase()))
    });
    w.count.set_text(&format!("{} Einträge", entries.len()));
    if entries.is_empty() {
        let label = Label::new(Some(if query.is_empty() { "Dieser Ordner ist leer." } else { "Keine passenden Dateien." }));
        label.style_context().add_class("empty"); w.rows.pack_start(&label, false, false, 0);
    }
    for entry in entries {
        let path = entry.path();
        let dir = entry.file_type().map(|f| f.is_dir()).unwrap_or(false);
        let row = Button::new(); row.style_context().add_class("file");
        if pane.selected.contains(&path) { row.style_context().add_class("selected"); }
        let h = UiBox::new(Orientation::Horizontal, 9);
        h.pack_start(&gtk::Image::from_icon_name(Some(if dir { "folder-symbolic" } else { "text-x-generic-symbolic" }), gtk::IconSize::Menu), false, false, 0);
        let name = Label::new(Some(&entry.file_name().to_string_lossy()));
        name.set_xalign(0.0); name.set_ellipsize(pango::EllipsizeMode::End); h.pack_start(&name, true, true, 0);
        let size = if dir { "Ordner".to_string() } else { entry.metadata().map(|m| file_size(m.len())).unwrap_or_default() };
        let meta = Label::new(Some(&size)); meta.style_context().add_class("meta"); h.pack_end(&meta, false, false, 0);
        row.add(&h); w.rows.pack_start(&row, false, false, 0);
        let s = shared.clone(); let p = path.clone();
        row.connect_clicked(move |button| {
            let selected = {
                let mut st = s.borrow_mut(); let v = &mut st.panes[i].selected;
                if v.contains(&p) { v.retain(|x| x != &p); } else { v.push(p.clone()); }
                let selected = v.contains(&p); st.active = i; selected
            };
            if selected { button.style_context().add_class("selected"); }
            else { button.style_context().remove_class("selected"); }
        });
        let s = shared.clone(); let p = path.clone();
        row.connect_button_press_event(move |_, e| {
            if e.event_type() == gdk::EventType::DoubleButtonPress {
                if p.is_dir() { navigate(&s, i, p.clone()); } else { open_file(&s, &p); }
                glib::Propagation::Stop
            } else { glib::Propagation::Proceed }
        });
    }
    w.rows.show_all();
}

fn navigate(shared: &Shared, i: usize, path: PathBuf) {
    let path = if path.is_absolute() { path } else { shared.borrow().panes[i].path.join(path) };
    if !path.is_dir() { notify(shared, "Dieser Ordner ist nicht erreichbar."); return; }
    { let mut s = shared.borrow_mut(); s.panes[i].path = path.clone(); s.panes[i].selected.clear(); s.active = i;
      let tab = s.panes[i].current_tab; s.panes[i].tabs[tab] = path; }
    render(shared, i);
}

fn notify(shared: &Shared, text: &str) { shared.borrow().message.set_text(text); }
fn valid_name(name: &str) -> bool { !name.trim().is_empty() && name != "." && name != ".." && !name.contains('/') }

struct Connection { host: String, user: String, password: String, port: u16, location: String }

fn connection_dialog(parent: &ApplicationWindow, protocol: &str) -> Option<Connection> {
    let title = match protocol { "ssh" => "SSH-Terminal", "sftp" => "SFTP verbinden", _ => "SMB verbinden" };
    let dialog = gtk::Dialog::with_buttons(Some(title), Some(parent), gtk::DialogFlags::MODAL,
        &[("Abbrechen", gtk::ResponseType::Cancel), ("Verbinden", gtk::ResponseType::Ok)]);
    dialog.set_default_size(420, 280);
    let content = dialog.content_area();
    let hint = Label::new(Some(match protocol {
        "ssh" => "SSH öffnet eine Terminalsitzung. Anmeldung erfolgt dort.",
        "sftp" => "Server-Schlüssel zuerst per SSH bestätigen; Passwort oder SSH-Agent nutzen.",
        _ => "Freigabe auf dem SMB-Server eingeben. Passwort bleibt nur für diese Verbindung gespeichert."
    }));
    hint.set_xalign(0.0); hint.set_line_wrap(true); content.pack_start(&hint, false, false, 8);
    let fields = UiBox::new(Orientation::Vertical, 5); content.pack_start(&fields, false, false, 8);
    let add_field = |label: &str, initial: &str, secret: bool| {
        let row = UiBox::new(Orientation::Horizontal, 8);
        let caption = Label::new(Some(label)); caption.set_xalign(0.0); caption.set_size_request(105, -1);
        let entry = Entry::new(); entry.set_text(initial); entry.set_hexpand(true);
        if secret { entry.set_visibility(false); }
        row.pack_start(&caption, false, false, 0); row.pack_start(&entry, true, true, 0);
        fields.pack_start(&row, false, false, 0); entry
    };
    let host = add_field("Server", "", false);
    let user = add_field("Benutzer", &std::env::var("USER").unwrap_or_default(), false);
    let port = add_field("Port", if protocol == "smb" { "445" } else { "22" }, false);
    let location = if protocol == "smb" { Some(add_field("Freigabe", "", false)) }
        else if protocol == "sftp" { Some(add_field("Startordner", "", false)) }
        else { None };
    let password = if protocol == "ssh" { None } else { Some(add_field("Passwort", "", true)) };
    dialog.show_all();
    let result = if dialog.run() == gtk::ResponseType::Ok {
        Some(Connection { host: host.text().trim().to_string(), user: user.text().trim().to_string(),
            password: password.map(|e| e.text().to_string()).unwrap_or_default(),
            port: port.text().parse().unwrap_or(0),
            location: location.map(|e| e.text().trim().to_string()).unwrap_or_default() })
    } else { None };
    dialog.close(); result
}

fn valid_host(host: &str) -> bool {
    !host.is_empty() && !host.starts_with('-') && host.chars().all(|c| c.is_ascii_alphanumeric() || ".:-_[]".contains(c))
}

fn obscure_password(password: &str) -> Result<String, String> {
    let mut child = Command::new("rclone").args(["obscure", "-"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
        .spawn().map_err(|_| "rclone ist nicht installiert.".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(password.as_bytes()).and_then(|_| stdin.write_all(b"\n"))
            .map_err(|e| format!("Passwort konnte nicht übergeben werden: {e}"))?;
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if !output.status.success() { return Err("Passwort konnte nicht vorbereitet werden.".into()); }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn open_ssh(connection: &Connection) -> Result<(), String> {
    let target = if connection.user.is_empty() { connection.host.clone() }
        else { format!("{}@{}", connection.user, connection.host) };
    for terminal in ["konsole", "alacritty", "xterm"] {
        if Command::new(terminal).arg("-e").arg("ssh").arg("-p")
            .arg(connection.port.to_string()).arg(&target).spawn().is_ok() { return Ok(()); }
    }
    Err("Kein Terminal gefunden (konsole, alacritty oder xterm).".into())
}

fn mount_remote(protocol: &str, connection: &Connection) -> Result<PathBuf, String> {
    let known_hosts = std::env::var_os("HOME").map(PathBuf::from)
        .unwrap_or_default().join(".ssh/known_hosts");
    if protocol == "sftp" && !known_hosts.is_file() {
        return Err("Server-Schlüssel fehlt. Bitte zuerst per SSH verbinden und bestätigen.".into());
    }
    let obscured = if connection.password.is_empty() { None } else { Some(obscure_password(&connection.password)?) };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos();
    let mountpoint = std::env::temp_dir().join(format!("dualfile-{}-{stamp}", std::process::id()));
    fs::create_dir(&mountpoint).map_err(|e| format!("Einhängeordner konnte nicht angelegt werden: {e}"))?;
    let remote = "dualfile_remote";
    let prefix = "RCLONE_CONFIG_DUALFILE_REMOTE_";
    let mut command = Command::new("rclone");
    command.args(["--config", "/dev/null", "mount", &format!("{remote}:{}", connection.location),
        mountpoint.to_string_lossy().as_ref(), "--daemon", "--daemon-wait", "15s",
        "--vfs-cache-mode", "writes"]);
    command.env(format!("{prefix}TYPE"), protocol)
        .env(format!("{prefix}HOST"), &connection.host)
        .env(format!("{prefix}USER"), &connection.user)
        .env(format!("{prefix}PORT"), connection.port.to_string());
    if protocol == "sftp" {
        command.env(format!("{prefix}KNOWN_HOSTS_FILE"), known_hosts);
        if connection.password.is_empty() { command.env(format!("{prefix}KEY_USE_AGENT"), "true"); }
    }
    if let Some(password) = obscured {
        command.env(format!("{prefix}PASS"), password);
    }
    let output = command.output().map_err(|_| {
        let _ = fs::remove_dir(&mountpoint); "rclone ist nicht installiert.".to_string()
    })?;
    if !output.status.success() {
        let _ = fs::remove_dir(&mountpoint);
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Verbindung fehlgeschlagen: {}", detail.trim().lines().last().unwrap_or("Unbekannter Fehler")));
    }
    Ok(mountpoint)
}

fn connect_network(shared: &Shared, protocol: &str) {
    let window = shared.borrow().window.clone();
    let Some(connection) = connection_dialog(&window, protocol) else { return };
    if !valid_host(&connection.host) || connection.port == 0 || connection.user.starts_with('-')
        || connection.user.chars().any(char::is_whitespace) {
        notify(shared, "Bitte Server, Benutzer und Port prüfen."); return;
    }
    if protocol == "smb" && (!valid_name(&connection.location) || connection.location.contains('\\')) {
        notify(shared, "Bitte einen gültigen Freigabenamen eingeben."); return;
    }
    if protocol == "sftp" && connection.location.contains("..") {
        notify(shared, "Bitte einen Startordner ohne .. eingeben."); return;
    }
    if protocol == "ssh" {
        match open_ssh(&connection) { Ok(()) => notify(shared, "SSH-Terminal geöffnet."), Err(e) => notify(shared, &e) }
        return;
    }
    notify(shared, "Verbindung wird aufgebaut …");
    match mount_remote(protocol, &connection) {
        Ok(path) => {
            let i = shared.borrow().active;
            shared.borrow().mounts.borrow_mut().push(path.clone());
            navigate(shared, i, path);
            notify(shared, if protocol == "sftp" { "SFTP verbunden." } else { "SMB verbunden." });
        },
        Err(e) => notify(shared, &e),
    }
}

fn perform(shared: &Shared, action: &str) {
    let (i, path, chosen, dest, win) = {
        let s = shared.borrow(); let i = s.active;
        (i, s.panes[i].path.clone(), s.panes[i].selected.clone(), s.panes[1-i].path.clone(), s.window.clone())
    };
    match action {
        "up" => { if let Some(parent) = path.parent() { navigate(shared, i, parent.to_path_buf()); } },
        "home" => { if let Some(h) = std::env::var_os("HOME") { navigate(shared, i, PathBuf::from(h)); } },
        "refresh" => { render(shared, 0); render(shared, 1); notify(shared, "Ansicht aktualisiert."); },
        "mkdir" => if let Some(name) = prompt(&win, "Neuer Ordner", "Name des neuen Ordners", "Neuer Ordner") {
            if !valid_name(&name) { notify(shared, "Bitte einen Namen ohne / eingeben."); return; }
            match fs::create_dir(path.join(name)) { Ok(_) => notify(shared, "Ordner angelegt."), Err(e) => notify(shared, &format!("Ordner konnte nicht angelegt werden: {e}")) }
            render(shared, i);
        },
        "rename" => {
            if chosen.len() != 1 { notify(shared, "Zum Umbenennen genau eine Datei auswählen."); return; }
            let source = &chosen[0]; let old = source.file_name().unwrap_or_default().to_string_lossy().to_string();
            if let Some(name) = prompt(&win, "Umbenennen", "Neuer Name", &old) {
                if !valid_name(&name) { notify(shared, "Bitte einen Namen ohne / eingeben."); return; }
                let target = source.with_file_name(name);
                if target.exists() { notify(shared, "Der Name ist bereits vergeben."); return; }
                match fs::rename(source, target) { Ok(_) => notify(shared, "Umbenannt."), Err(e) => notify(shared, &format!("Umbenennen fehlgeschlagen: {e}")) }
                shared.borrow_mut().panes[i].selected.clear(); render(shared, i);
            }
        },
        "copy" | "move" => {
            if chosen.is_empty() { notify(shared, "Zuerst Dateien auswählen."); return; }
            let mut done = 0; let mut error = None;
            for source in &chosen {
                if source.parent() == Some(dest.as_path()) { continue; }
                let target = unique_target(&dest.join(source.file_name().unwrap_or_default()));
                let result = if action == "copy" { copy_path(source, &target) }
                    else { fs::rename(source, &target).or_else(|_| { copy_path(source, &target)?; remove_path(source) }) };
                match result { Ok(_) => done += 1, Err(e) => { error = Some(e.to_string()); break; } }
            }
            shared.borrow_mut().panes[i].selected.clear(); render(shared, 0); render(shared, 1);
            notify(shared, &match error { Some(e) => format!("{done} erledigt. Fehler: {e}"), None => format!("{done} Element(e) {}.", if action == "copy" { "kopiert" } else { "verschoben" }) });
        },
        "trash" => {
            if chosen.is_empty() { notify(shared, "Zuerst Dateien auswählen."); return; }
            let dialog = gtk::MessageDialog::new(Some(&win), gtk::DialogFlags::MODAL, gtk::MessageType::Question, gtk::ButtonsType::YesNo,
                &format!("{} Element(e) in den Papierkorb verschieben?", chosen.len()));
            let yes = dialog.run() == gtk::ResponseType::Yes; dialog.close();
            if yes {
                let mut done = 0; let mut failed = false;
                for p in &chosen {
                    if gtk::gio::File::for_path(p).trash(gtk::gio::Cancellable::NONE).is_ok() { done += 1; } else { failed = true; }
                }
                shared.borrow_mut().panes[i].selected.clear(); render(shared, i);
                notify(shared, &format!("{done} im Papierkorb.{}", if failed { " Manche Elemente konnten nicht verschoben werden." } else { "" }));
            }
        },
        "view" | "edit" => {
            if chosen.len() != 1 { notify(shared, "Zum Anzeigen genau eine Datei auswählen."); return; }
            if chosen[0].is_dir() { navigate(shared, i, chosen[0].clone()); }
            else { text_file(shared, &chosen[0], action == "edit"); }
        },
        _ => {}
    }
}

fn prompt(parent: &ApplicationWindow, title: &str, hint: &str, initial: &str) -> Option<String> {
    let dialog = gtk::Dialog::with_buttons(Some(title), Some(parent), gtk::DialogFlags::MODAL,
        &[("Abbrechen", gtk::ResponseType::Cancel), ("Übernehmen", gtk::ResponseType::Ok)]);
    dialog.set_default_size(390, 130);
    let content = dialog.content_area(); let label = Label::new(Some(hint)); label.set_xalign(0.0);
    content.pack_start(&label, false, false, 8);
    let entry = Entry::new(); entry.set_text(initial); content.pack_start(&entry, false, false, 8);
    dialog.show_all();
    let result = if dialog.run() == gtk::ResponseType::Ok { Some(entry.text().to_string()) } else { None };
    dialog.close(); result
}
fn open_file(shared: &Shared, path: &Path) {
    let uri = gtk::gio::File::for_path(path).uri();
    match gtk::gio::AppInfo::launch_default_for_uri(&uri, gtk::gio::AppLaunchContext::NONE) {
        Ok(_) => notify(shared, "Datei geöffnet."), Err(e) => notify(shared, &format!("Datei konnte nicht geöffnet werden: {e}"))
    }
}
fn text_file(shared: &Shared, path: &Path, edit: bool) {
    let content = match fs::metadata(path) {
        Ok(meta) if meta.len() > 2_000_000 => { notify(shared, "Für die Textansicht ist diese Datei zu groß."); return; },
        Err(e) => { notify(shared, &format!("Datei kann nicht gelesen werden: {e}")); return; },
        _ => match fs::read_to_string(path) {
            Ok(text) => text,
            Err(_) => { open_file(shared, path); return; }
        }
    };
    let win = shared.borrow().window.clone();
    let title = format!("{} — {}", if edit { "Bearbeiten" } else { "Anzeigen" }, path.file_name().unwrap_or_default().to_string_lossy());
    let buttons = if edit {
        vec![("Abbrechen", gtk::ResponseType::Cancel), ("Speichern", gtk::ResponseType::Ok)]
    } else {
        vec![("Schließen", gtk::ResponseType::Close)]
    };
    let dialog = gtk::Dialog::with_buttons(Some(&title), Some(&win), gtk::DialogFlags::MODAL, &buttons);
    dialog.set_default_size(850, 600);
    let area = dialog.content_area();
    let scroll = ScrolledWindow::new(None::<&gtk::Adjustment>, None::<&gtk::Adjustment>);
    scroll.set_hexpand(true); scroll.set_vexpand(true);
    let view = gtk::TextView::new();
    view.set_monospace(true); view.set_editable(edit); view.set_wrap_mode(gtk::WrapMode::None);
    let buffer = view.buffer().unwrap(); buffer.set_text(&content);
    scroll.add(&view); area.pack_start(&scroll, true, true, 6);
    dialog.show_all();
    if dialog.run() == gtk::ResponseType::Ok && edit {
        let (start, end) = buffer.bounds();
        if let Some(text) = buffer.text(&start, &end, true) {
            match fs::write(path, text.as_str()) {
                Ok(_) => notify(shared, "Änderungen gespeichert."),
                Err(e) => notify(shared, &format!("Speichern fehlgeschlagen: {e}"))
            }
        }
    }
    dialog.close();
}
fn unique_target(path: &Path) -> PathBuf {
    if !path.exists() { return path.to_path_buf(); }
    let stem = path.file_stem().unwrap_or_default().to_string_lossy();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    for n in 2..10000 { let candidate = path.with_file_name(format!("{stem} ({n}){ext}")); if !candidate.exists() { return candidate; } }
    path.to_path_buf()
}
fn copy_path(source: &Path, target: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(source)?;
    if meta.file_type().is_symlink() { std::os::unix::fs::symlink(fs::read_link(source)?, target) }
    else if meta.is_dir() {
        let source_real = fs::canonicalize(source)?;
        let target_parent_real = fs::canonicalize(target.parent().unwrap_or_else(|| Path::new(".")))?;
        if target.starts_with(source) || target_parent_real.starts_with(source_real) {
            return Err(std::io::Error::other("Ziel liegt im Quellordner"));
        }
        fs::create_dir(target)?;
        for e in fs::read_dir(source)? { let e = e?; copy_path(&e.path(), &target.join(e.file_name()))?; }
        Ok(())
    } else { fs::copy(source, target).map(|_| ()) }
}
fn remove_path(path: &Path) -> std::io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() { fs::remove_dir_all(path) } else { fs::remove_file(path) }
}
fn file_size(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") }
    else if bytes < 1048576 { format!("{:.1} KB", bytes as f64 / 1024.0) }
    else if bytes < 1073741824 { format!("{:.1} MB", bytes as f64 / 1048576.0) }
    else { format!("{:.1} GB", bytes as f64 / 1073741824.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_preserves_nested_files_and_avoids_overwrite() {
        let root = std::env::temp_dir().join(format!("dualfile-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("from/nested")).unwrap();
        fs::create_dir(root.join("to")).unwrap();
        fs::write(root.join("from/nested/example.txt"), "hello").unwrap();
        copy_path(&root.join("from"), &root.join("to/from")).unwrap();
        assert_eq!(fs::read_to_string(root.join("to/from/nested/example.txt")).unwrap(), "hello");
        let next = unique_target(&root.join("to/from"));
        assert_ne!(next, root.join("to/from"));
        assert!(!next.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn copy_rejects_destination_inside_source() {
        let root = std::env::temp_dir().join(format!("dualfile-nested-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("from/sub")).unwrap();
        assert!(copy_path(&root.join("from"), &root.join("from/sub/copy")).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
