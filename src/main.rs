use gtk4::{
    glib::{self, clone},
    gio,
    prelude::*,
    Align, Button, ComboBoxText, Entry, Grid, Label, Orientation, Separator,
};
use libadwaita::{Application, ApplicationWindow, HeaderBar, ToolbarView};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

const APP_ID: &str = "io.github.nfsmount";
const MAX_HISTORY: usize = 10;

fn main() -> glib::ExitCode {
    let app = Application::builder()
        .application_id(APP_ID)
        .build();
    app.connect_activate(build_ui);
    app.run()
}

// ── history persistence ───────────────────────────────────────────────────────

#[derive(Clone)]
struct HistoryEntry {
    remote: String,
    local: String,
    options: String,
}

fn history_path() -> PathBuf {
    let mut p = glib::user_config_dir();
    p.push("nfs-mount");
    p.push("history");
    p
}

fn load_history() -> Vec<HistoryEntry> {
    std::fs::read_to_string(history_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let remote  = parts.next()?.to_string();
            let local   = parts.next()?.to_string();
            let options = parts.next().unwrap_or("").to_string();
            // FIX: || not && — reject entries where either required field is empty
            if remote.is_empty() || local.is_empty() { return None; }
            Some(HistoryEntry { remote, local, options })
        })
        .collect()
}

fn save_history(history: &[HistoryEntry]) -> Result<(), std::io::Error> {
    let path = history_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;  // FIX: propagate instead of .ok()
    }
    let content = history
        .iter()
        .map(|e| {
            // FIX: strip control chars that would corrupt the tab-delimited format
            let r = e.remote.replace(['\t', '\n', '\r'], "");
            let l = e.local.replace(['\t', '\n', '\r'], "");
            let o = e.options.replace(['\t', '\n', '\r'], "");
            format!("{}\t{}\t{}", r, l, o)
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(path, content)  // FIX: propagate instead of .ok()
}

fn push_history(history: &mut Vec<HistoryEntry>, entry: HistoryEntry) -> Result<(), std::io::Error> {
    history.retain(|e| e.remote != entry.remote || e.local != entry.local);
    history.insert(0, entry);
    history.truncate(MAX_HISTORY);
    save_history(history)
}

fn rebuild_combo(combo: &ComboBoxText, history: &[HistoryEntry]) {
    combo.remove_all();
    combo.append(None, "— recent mounts —");
    for (i, e) in history.iter().enumerate() {
        let label = if e.options.is_empty() {
            format!("{}  →  {}", e.remote, e.local)
        } else {
            format!("{}  →  {}  ({})", e.remote, e.local, e.options)
        };
        combo.append(Some(&i.to_string()), &label);
    }
    combo.set_active(Some(0));
}

// ── backend ───────────────────────────────────────────────────────────────────

// /proc/mounts encodes spaces and special chars as \NNN octal sequences.
fn decode_proc_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    loop {
        match chars.next() {
            None => break,
            Some('\\') => {
                let d1 = chars.next();
                let d2 = chars.next();
                let d3 = chars.next();
                match (d1, d2, d3) {
                    (Some(a), Some(b), Some(c))
                        if matches!(a, '0'..='7')
                            && matches!(b, '0'..='7')
                            && matches!(c, '0'..='7') =>
                    {
                        let n = (a as u32 - '0' as u32) * 64
                            + (b as u32 - '0' as u32) * 8
                            + (c as u32 - '0' as u32);
                        if let Some(ch) = char::from_u32(n) {
                            out.push(ch);
                        }
                    }
                    _ => {
                        out.push('\\');
                        if let Some(a) = d1 { out.push(a); }
                        if let Some(b) = d2 { out.push(b); }
                        if let Some(c) = d3 { out.push(c); }
                    }
                }
            }
            Some(c) => out.push(c),
        }
    }
    out
}

// FIX: returns Option<bool> — None when /proc/mounts is unreadable (container/sandbox)
fn is_mounted(mount_point: &str) -> Option<bool> {
    let mp = mount_point.trim_end_matches('/');
    // FIX: decode \NNN octal escapes so paths with spaces compare correctly
    let content = std::fs::read_to_string("/proc/mounts").ok()?;
    Some(content.lines().any(|line| {
        let Some(raw) = line.split_whitespace().nth(1) else { return false };
        let decoded = decode_proc_path(raw);
        decoded.trim_end_matches('/') == mp
    }))
}

fn do_mount(remote: String, local: String, options: String) -> Result<(), String> {
    let mut args = vec!["mount".to_string(), "-t".to_string(), "nfs".to_string()];
    let opts = options.trim().to_string();
    if !opts.is_empty() {
        args.extend(["-o".to_string(), opts]);
    }
    args.push(remote.trim().to_string());
    args.push(local.trim().to_string());
    run_privileged(args)
}

fn do_umount(local: String) -> Result<(), String> {
    run_privileged(vec!["umount".to_string(), local.trim().to_string()])
}

fn run_privileged(args: Vec<String>) -> Result<(), String> {
    // FIX: absolute path avoids PATH hijacking and polkit action mismatch
    let out = std::process::Command::new("/usr/bin/pkexec")
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to launch pkexec: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Err(if !stderr.is_empty() { stderr } else if !stdout.is_empty() { stdout } else { "Command failed".into() })
}

fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

// ── UI helpers ────────────────────────────────────────────────────────────────

fn refresh_status(local_entry: &Entry, status_label: &Label) {
    let text = local_entry.text();
    let mp = text.trim();
    if mp.is_empty() {
        status_label.set_markup("<span color='gray'>● enter a mount point above</span>");
    } else {
        // FIX: handle None (unreadable /proc/mounts) as a distinct "unknown" state
        match is_mounted(mp) {
            Some(true)  => status_label.set_markup("<span color='#2ec27e' weight='bold'>● Mounted</span>"),
            Some(false) => status_label.set_markup("<span color='#e01b24'>● Not mounted</span>"),
            None        => status_label.set_markup("<span color='gray'>● status unknown</span>"),
        }
    }
}

fn set_busy(busy: bool, btns: &[&Button]) {
    for b in btns {
        b.set_sensitive(!busy);
    }
}

// ── UI builder ────────────────────────────────────────────────────────────────

fn build_ui(app: &Application) {
    let history = Rc::new(RefCell::new(load_history()));

    // ── recent mounts combo ───────────────────────────────────────────────────

    let recent_combo = ComboBoxText::new();
    rebuild_combo(&recent_combo, &history.borrow());

    let recent_row = gtk4::Box::new(Orientation::Horizontal, 8);
    recent_row.set_margin_top(16);
    recent_row.set_margin_bottom(8);
    recent_row.set_margin_start(20);
    recent_row.set_margin_end(20);
    recent_row.append(&Label::builder().label("Recent:").halign(Align::End).build());
    recent_row.append(&recent_combo);

    // ── form entries ──────────────────────────────────────────────────────────

    let remote_entry = Entry::builder()
        .placeholder_text("192.168.1.100:/exports/share")
        .hexpand(true)
        .build();

    let local_entry = Entry::builder()
        .placeholder_text("/mnt/mynas")
        .hexpand(true)
        .build();

    let options_entry = Entry::builder()
        .placeholder_text("vers=4,rw  (optional)")
        .hexpand(true)
        .build();

    let make_lbl = |text: &str| Label::builder().label(text).halign(Align::End).build();

    let grid = Grid::builder()
        .row_spacing(10)
        .column_spacing(12)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(20)
        .margin_end(20)
        .build();

    grid.attach(&make_lbl("Remote:"), 0, 0, 1, 1);
    grid.attach(&remote_entry, 1, 0, 1, 1);
    grid.attach(&make_lbl("Mount point:"), 0, 1, 1, 1);
    grid.attach(&local_entry, 1, 1, 1, 1);
    grid.attach(&make_lbl("Options:"), 0, 2, 1, 1);
    grid.attach(&options_entry, 1, 2, 1, 1);

    // ── status + feedback ─────────────────────────────────────────────────────

    let status_label = Label::builder()
        .use_markup(true)
        .halign(Align::Start)
        .margin_start(20)
        .margin_end(20)
        .margin_top(8)
        .build();
    status_label.set_markup("<span color='gray'>● enter a mount point above</span>");

    let info_label = Label::builder()
        .use_markup(true)
        .halign(Align::Start)
        .wrap(true)
        .margin_start(20)
        .margin_end(20)
        .build();

    // ── buttons ───────────────────────────────────────────────────────────────

    let refresh_btn = Button::with_label("Refresh");

    let umount_btn = Button::with_label("Unmount");
    umount_btn.add_css_class("destructive-action");

    let mount_btn = Button::with_label("Mount");
    mount_btn.add_css_class("suggested-action");

    let btn_row = gtk4::Box::new(Orientation::Horizontal, 8);
    btn_row.set_halign(Align::End);
    btn_row.set_margin_top(8);
    btn_row.set_margin_bottom(20);
    btn_row.set_margin_start(20);
    btn_row.set_margin_end(20);
    btn_row.append(&refresh_btn);
    btn_row.append(&umount_btn);
    btn_row.append(&mount_btn);

    let vbox = gtk4::Box::new(Orientation::Vertical, 0);
    vbox.append(&recent_row);
    vbox.append(&Separator::new(Orientation::Horizontal));
    vbox.append(&grid);
    vbox.append(&Separator::new(Orientation::Horizontal));
    vbox.append(&status_label);
    vbox.append(&info_label);
    vbox.append(&btn_row);

    // ── signals ───────────────────────────────────────────────────────────────

    // selecting a recent mount populates all three entries
    recent_combo.connect_changed(clone!(
        #[weak] remote_entry, #[weak] local_entry, #[weak] options_entry,
        #[strong] history,
        move |combo| {
            let Some(id) = combo.active_id() else { return };
            let Ok(i) = id.parse::<usize>() else { return };
            let h = history.borrow();
            if let Some(e) = h.get(i) {
                remote_entry.set_text(&e.remote);
                local_entry.set_text(&e.local);
                options_entry.set_text(&e.options);
            }
        }
    ));

    local_entry.connect_changed(clone!(
        #[weak] status_label,
        move |entry| refresh_status(entry, &status_label)
    ));

    refresh_btn.connect_clicked(clone!(
        #[weak] local_entry, #[weak] status_label,
        move |_| refresh_status(&local_entry, &status_label)
    ));

    // mount
    mount_btn.connect_clicked(clone!(
        #[weak] remote_entry, #[weak] local_entry, #[weak] options_entry,
        #[weak] status_label, #[weak] info_label,
        #[weak] mount_btn, #[weak] umount_btn, #[weak] refresh_btn,
        #[weak] recent_combo,
        #[strong] history,
        move |_| {
            // FIX: trim early so push_history stores clean values (dedup works correctly)
            let remote = remote_entry.text().trim().to_string();
            let local  = local_entry.text().trim().to_string();
            let opts   = options_entry.text().trim().to_string();

            if remote.is_empty() || local.is_empty() {
                info_label.set_text("Remote and mount point are required.");
                return;
            }

            set_busy(true, &[&mount_btn, &umount_btn, &refresh_btn]);
            recent_combo.set_sensitive(false);  // FIX: lock combo during async op
            info_label.set_text("Mounting…");

            let (r2, l2, o2) = (remote.clone(), local.clone(), opts.clone());
            let (local_entry, status_label, info_label) =
                (local_entry.clone(), status_label.clone(), info_label.clone());
            let (mount_btn, umount_btn, refresh_btn) =
                (mount_btn.clone(), umount_btn.clone(), refresh_btn.clone());
            let (recent_combo, history) = (recent_combo.clone(), history.clone());

            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || do_mount(remote, local, opts))
                    .await
                    .unwrap_or_else(|_| Err("Thread panicked".into()));

                set_busy(false, &[&mount_btn, &umount_btn, &refresh_btn]);
                recent_combo.set_sensitive(true);  // FIX: restore combo
                match result {
                    Ok(()) => {
                        refresh_status(&local_entry, &status_label);
                        // FIX: surface history-save errors rather than silently dropping them
                        if let Err(e) = push_history(&mut history.borrow_mut(), HistoryEntry {
                            remote: r2, local: l2, options: o2,
                        }) {
                            info_label.set_markup(&format!(
                                "<span color='#e01b24'>Mounted, but failed to save history: {}</span>",
                                escape_markup(&e.to_string())
                            ));
                        } else {
                            info_label.set_text("");
                        }
                        rebuild_combo(&recent_combo, &history.borrow());
                    }
                    Err(e) => info_label.set_markup(&format!(
                        "<span color='#e01b24'>{}</span>", escape_markup(&e)
                    )),
                }
            });
        }
    ));

    // unmount
    umount_btn.connect_clicked(clone!(
        #[weak] local_entry,
        #[weak] status_label, #[weak] info_label,
        #[weak] mount_btn, #[weak] umount_btn, #[weak] refresh_btn,
        #[weak] recent_combo,  // FIX: capture so it can be locked during async op
        move |_| {
            let local = local_entry.text().trim().to_string();

            if local.is_empty() {
                info_label.set_text("Mount point is required.");
                return;
            }

            set_busy(true, &[&mount_btn, &umount_btn, &refresh_btn]);
            recent_combo.set_sensitive(false);  // FIX: lock combo during async op
            info_label.set_text("Unmounting…");

            let (local_entry, status_label, info_label) =
                (local_entry.clone(), status_label.clone(), info_label.clone());
            let (mount_btn, umount_btn, refresh_btn) =
                (mount_btn.clone(), umount_btn.clone(), refresh_btn.clone());
            let recent_combo = recent_combo.clone();

            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || do_umount(local))
                    .await
                    .unwrap_or_else(|_| Err("Thread panicked".into()));

                set_busy(false, &[&mount_btn, &umount_btn, &refresh_btn]);
                recent_combo.set_sensitive(true);  // FIX: restore combo
                match result {
                    Ok(()) => {
                        info_label.set_text("");
                        refresh_status(&local_entry, &status_label);
                    }
                    Err(e) => info_label.set_markup(&format!(
                        "<span color='#e01b24'>{}</span>", escape_markup(&e)
                    )),
                }
            });
        }
    ));

    // ── window ────────────────────────────────────────────────────────────────

    let toolbar_view = ToolbarView::new();
    toolbar_view.add_top_bar(&HeaderBar::new());
    toolbar_view.set_content(Some(&vbox));

    let window = ApplicationWindow::builder()
        .application(app)
        .title("NFS Mount Manager")
        .default_width(500)
        .content(&toolbar_view)
        .build();

    window.present();
}
