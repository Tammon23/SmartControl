use crate::{
    backend::{Action, Backend, Config},
    panel::{fill, rect, rgb, text},
};
use bulb_automation::{Ledger, LightAction, Rule, Settings, Trigger};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{Mutex, OnceLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{
        Controls::EM_SETLIMITTEXT, Input::KeyboardAndMouse::*, Shell::SetWindowSubclass,
        WindowsAndMessaging::*,
    },
};

static STORE: OnceLock<Mutex<Store>> = OnceLock::new();
static WINDOW: Mutex<usize> = Mutex::new(0);
static UI: Mutex<Ui> = Mutex::new(Ui {
    page: 0,
    offset: 0,
    editing: None,
    status: String::new(),
});
struct Store {
    settings: Settings,
    ledger: Ledger,
    path: PathBuf,
    ledger_path: PathBuf,
    last: String,
}
struct Ui {
    page: u8,
    offset: usize,
    editing: Option<Rule>,
    status: String,
}
const W: i32 = 460;
const H: i32 = 650;
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read automation settings")?)
        .map_err(|_| "Invalid automation settings".into())
}
fn write<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| "Cannot create settings folder")?;
    }
    let temp = path.with_extension("pending");
    std::fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|_| "Cannot encode rules")?,
    )
    .map_err(|_| "Cannot save automation settings")?;
    std::fs::rename(temp, path).map_err(|_| "Cannot replace automation settings".into())
}
fn save(settings: Settings) -> Result<(), String> {
    bulb_automation::validate(&settings)?;
    let mut store = STORE
        .get()
        .ok_or("Automations are unavailable")?
        .lock()
        .unwrap();
    let mut ledger = store.ledger.clone();
    bulb_automation::baseline(&mut ledger, &settings, now())?;
    write(&store.ledger_path, &ledger)?;
    write(&store.path, &settings)?;
    store.settings = settings;
    store.ledger = ledger;
    Ok(())
}
pub fn actions(action: &LightAction) -> Vec<Action> {
    if action.power == Some(false) {
        return vec![Action::Power(false)];
    }
    let mut result = Vec::new();
    if action.power == Some(true) {
        result.push(Action::Power(true));
    }
    if let Some(preset) = action.preset.as_deref() {
        let name = match preset {
            "warm" => "warm",
            "white" => "white",
            "red" => "red",
            "green" => "green",
            "blue" => "blue",
            "purple" => "purple",
            _ => return result,
        };
        result.push(Action::Preset(name));
    }
    if let Some([r, g, b]) = action.colour {
        result.push(Action::Colour(r, g, b));
    }
    if let Some(level) = action.brightness {
        result.push(Action::Brightness(level));
    }
    result
}
unsafe fn post(owner: HWND, action: &LightAction) {
    let data = Box::into_raw(Box::new(actions(action)));
    if PostMessageW(owner, crate::tray::AUTOMATION, 0, data as LPARAM) == 0 {
        drop(Box::from_raw(data));
    }
}
pub fn start(owner: HWND, config: &Config) -> Result<(), String> {
    let key = match &config.backend {
        Backend::DirectCloud {
            device_id,
            component,
            ..
        } => format!("{device_id}:{component}"),
        Backend::Bridge { server_url, .. } => server_url.clone(),
    };
    let scope = Sha256::digest(key.as_bytes())
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let base =
        PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or("Cannot locate local settings")?)
            .join("SmartControl");
    let path = base.join(format!("automation-{scope}.json"));
    let ledger_path = base.join(format!("automation-ledger-{scope}.json"));
    let mut settings: Settings = if path.exists() {
        read(&path)?
    } else {
        Settings::default()
    };
    settings.timezone = "local".into();
    bulb_automation::validate(&settings)?;
    let ledger = if ledger_path.exists() {
        read(&ledger_path)?
    } else {
        let mut ledger = Ledger::default();
        bulb_automation::baseline(&mut ledger, &settings, now())?;
        write(&ledger_path, &ledger)?;
        ledger
    };
    STORE
        .set(Mutex::new(Store {
            settings,
            ledger,
            path,
            ledger_path,
            last: String::new(),
        }))
        .map_err(|_| "Automations already started")?;
    let owner = owner as usize;
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(15));
        let mut store = STORE.get().unwrap().lock().unwrap();
        match bulb_automation::due(&store.settings, &store.ledger, now()) {
            Ok(occurrences) => {
                for occurrence in occurrences {
                    let mut ledger = store.ledger.clone();
                    bulb_automation::mark(&mut ledger, &occurrence);
                    if let Err(e) = write(&store.ledger_path, &ledger) {
                        store.last = e;
                        break;
                    }
                    store.ledger = ledger;
                    store.last = format!("Queued: {}", occurrence.rule.name);
                    unsafe {
                        post(owner as HWND, &occurrence.rule.action);
                    }
                }
            }
            Err(e) => store.last = e,
        }
    });
    Ok(())
}
fn summary(rule: &Rule) -> String {
    match rule.trigger {
        Trigger::Time { hour, minute } => format!("Daily at {hour:02}:{minute:02}"),
        Trigger::Sunset { offset_minutes } => format!("Daily at sunset {offset_minutes:+} min"),
    }
}
fn upcoming(settings: &Settings, rule: &Rule) -> String {
    match bulb_automation::preview(settings, rule, now()) {
        Ok(Some(at)) => chrono::DateTime::from_timestamp(at, 0)
            .map(|t| {
                format!(
                    "Next: {}{}",
                    t.with_timezone(&chrono::Local).format("%a %H:%M"),
                    if rule.enabled { "" } else { " (if enabled)" }
                )
            })
            .unwrap_or_default(),
        Ok(None) => "No upcoming sunset for this location".into(),
        Err(e) => e,
    }
}
unsafe fn clear_fields(hwnd: HWND) {
    for id in [1001, 1002, 1003, 1005, 1006] {
        let field = GetDlgItem(hwnd, id);
        if !field.is_null() {
            DestroyWindow(field);
        }
    }
}
unsafe fn field(hwnd: HWND, id: usize, value: &str, x: i32, y: i32, width: i32) {
    let mut bounds: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut bounds);
    let x = x * bounds.right / W;
    let y = y * bounds.bottom / H;
    let width = width * bounds.right / W;
    let height = 30 * bounds.bottom / H;
    let edit = CreateWindowExW(
        0,
        wide("EDIT").as_ptr(),
        wide(value).as_ptr(),
        WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
        x,
        y,
        width,
        height,
        hwnd,
        id as HMENU,
        GetModuleHandleW(null()),
        null(),
    );
    let name = wide("RulesFont");
    let mut font = GetPropW(hwnd, name.as_ptr());
    if font.is_null() {
        font = CreateFontW(
            -19 * bounds.bottom / H,
            0,
            0,
            0,
            400,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            0,
            0,
            CLEARTYPE_QUALITY as u32,
            0,
            wide("Segoe UI").as_ptr(),
        ) as HANDLE;
        SetPropW(hwnd, name.as_ptr(), font);
    }
    SetWindowSubclass(edit, Some(crate::panel::edit_procedure), 1, 0);
    SendMessageW(edit, WM_SETFONT, font as usize, 1);
    SendMessageW(edit, EM_SETLIMITTEXT, 80, 0);
}
unsafe fn resize_fields(hwnd: HWND) {
    let mut bounds: RECT = std::mem::zeroed();
    GetClientRect(hwnd, &mut bounds);
    if bounds.right <= 0 || bounds.bottom <= 0 {
        return;
    }
    let font = CreateFontW(
        -19 * bounds.bottom / H,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET as u32,
        0,
        0,
        CLEARTYPE_QUALITY as u32,
        0,
        wide("Segoe UI").as_ptr(),
    );
    let name = wide("RulesFont");
    let old = GetPropW(hwnd, name.as_ptr());
    SetPropW(hwnd, name.as_ptr(), font as HANDLE);
    for (id, x, y, width) in [
        (1001, 36, 145, 388),
        (1002, 36, 260, 156),
        (1003, 36, 377, 132),
        (1005, 36, 205, 388),
        (1006, 36, 295, 388),
    ] {
        let edit = GetDlgItem(hwnd, id);
        if !edit.is_null() {
            MoveWindow(
                edit,
                x * bounds.right / W,
                y * bounds.bottom / H,
                width * bounds.right / W,
                30 * bounds.bottom / H,
                1,
            );
            SendMessageW(edit, WM_SETFONT, font as usize, 1);
        }
    }
    if !old.is_null() {
        DeleteObject(old as HGDIOBJ);
    }
}
unsafe fn value(hwnd: HWND, id: i32) -> String {
    let mut buffer = [0u16; 256];
    let count = GetWindowTextW(GetDlgItem(hwnd, id), buffer.as_mut_ptr(), 256);
    String::from_utf16_lossy(&buffer[..count.max(0) as usize])
        .trim()
        .into()
}
unsafe fn fields(hwnd: HWND) {
    clear_fields(hwnd);
    let ui = UI.lock().unwrap();
    if ui.page == 1 {
        if let Some(rule) = &ui.editing {
            field(hwnd, 1001, &rule.name, 36, 145, 388);
            let time = match rule.trigger {
                Trigger::Time { hour, minute } => format!("{hour:02}:{minute:02}"),
                Trigger::Sunset { offset_minutes } => offset_minutes.to_string(),
            };
            field(hwnd, 1002, &time, 36, 260, 156);
            field(
                hwnd,
                1003,
                &rule
                    .action
                    .brightness
                    .map(|v| v.to_string())
                    .unwrap_or_default(),
                36,
                377,
                132,
            );
        }
    } else if ui.page == 2 {
        let store = STORE.get().unwrap().lock().unwrap();
        field(
            hwnd,
            1005,
            &store
                .settings
                .latitude
                .map(|v| v.to_string())
                .unwrap_or_default(),
            36,
            205,
            388,
        );
        field(
            hwnd,
            1006,
            &store
                .settings
                .longitude
                .map(|v| v.to_string())
                .unwrap_or_default(),
            36,
            295,
            388,
        );
    }
    drop(ui);
    InvalidateRect(hwnd, null(), 0);
}
unsafe fn draft(hwnd: HWND) -> Result<Rule, String> {
    let mut rule = UI.lock().unwrap().editing.clone().ok_or("Select a rule")?;
    rule.name = value(hwnd, 1001);
    let time = value(hwnd, 1002);
    rule.trigger = match rule.trigger {
        Trigger::Time { .. } => {
            let (h, m) = time.split_once(':').ok_or("Use a time such as 18:30")?;
            Trigger::Time {
                hour: h.parse().map_err(|_| "Invalid hour")?,
                minute: m.parse().map_err(|_| "Invalid minute")?,
            }
        }
        Trigger::Sunset { .. } => Trigger::Sunset {
            offset_minutes: time
                .parse()
                .map_err(|_| "Use a sunset offset such as -30")?,
        },
    };
    let brightness = value(hwnd, 1003);
    rule.action.brightness = if brightness.is_empty() {
        None
    } else {
        Some(
            brightness
                .parse()
                .map_err(|_| "Brightness must be 0–100 or blank")?,
        )
    };
    if rule.action.power == Some(false) {
        rule.action.brightness = None;
        rule.action.preset = None;
        rule.action.colour = None;
    }
    Ok(rule)
}
unsafe fn persist_rule(hwnd: HWND) -> Result<(), String> {
    let rule = draft(hwnd)?;
    let mut settings = STORE.get().unwrap().lock().unwrap().settings.clone();
    if let Some(old) = settings.rules.iter_mut().find(|old| old.id == rule.id) {
        *old = rule;
    } else {
        settings.rules.push(rule);
    }
    save(settings)?;
    let mut ui = UI.lock().unwrap();
    ui.page = 0;
    ui.editing = None;
    ui.status = "Rule saved. Existing past events will not run.".into();
    drop(ui);
    fields(hwnd);
    Ok(())
}
unsafe fn button(dc: HDC, label: &str, r: RECT, accent: bool) {
    fill(
        dc,
        r,
        if accent {
            rgb(255, 204, 107)
        } else {
            rgb(42, 50, 65)
        },
        10,
    );
    text(
        dc,
        label,
        r,
        16,
        600,
        if accent {
            rgb(28, 30, 37)
        } else {
            rgb(237, 241, 247)
        },
        DT_SINGLELINE | DT_CENTER | DT_VCENTER,
    );
}
unsafe fn input_background(dc: HDC, r: RECT) {
    fill(dc, r, rgb(70, 80, 98), 10);
    fill(
        dc,
        rect(r.left + 1, r.top + 1, r.right - 1, r.bottom - 1),
        rgb(31, 37, 49),
        8,
    );
}
unsafe fn paint(dc: HDC, width: i32, height: i32) {
    SetMapMode(dc, MM_ANISOTROPIC);
    SetWindowExtEx(dc, W, H, null_mut());
    SetViewportExtEx(dc, width, height, null_mut());
    let ui = UI.lock().unwrap();
    let store = STORE.get().unwrap().lock().unwrap();
    let fg = rgb(237, 241, 247);
    let muted = rgb(145, 155, 173);
    fill(dc, rect(0, 0, W, H), rgb(20, 24, 33), 0);
    text(
        dc,
        "AUTOMATIONS",
        rect(24, 18, 436, 40),
        14,
        600,
        muted,
        DT_SINGLELINE,
    );
    text(
        dc,
        if ui.page == 1 {
            "Edit rule"
        } else if ui.page == 2 {
            "Manual location"
        } else {
            "Rules for this bulb"
        },
        rect(24, 45, 436, 80),
        26,
        600,
        fg,
        DT_SINGLELINE,
    );
    if ui.page != 1 {
        button(dc, "Rules", rect(24, 88, 220, 123), ui.page == 0);
        button(dc, "Location", rect(236, 88, 436, 123), ui.page == 2);
    }
    if ui.page == 0 {
        if store.settings.rules.is_empty() {
            text(
                dc,
                "No rules yet. Add a daily time or sunset rule. New rules start disabled.",
                rect(24, 155, 436, 260),
                15,
                400,
                muted,
                DT_WORDBREAK,
            );
        }
        for (row, rule) in store
            .settings
            .rules
            .iter()
            .skip(ui.offset)
            .take(6)
            .enumerate()
        {
            let top = 145 + row as i32 * 51;
            fill(dc, rect(24, top, 436, top + 45), rgb(31, 37, 49), 10);
            text(
                dc,
                &rule.name,
                rect(36, top + 3, 345, top + 24),
                16,
                600,
                fg,
                DT_SINGLELINE | DT_END_ELLIPSIS,
            );
            text(
                dc,
                &summary(rule),
                rect(36, top + 25, 345, top + 44),
                11,
                400,
                muted,
                DT_SINGLELINE,
            );
            button(
                dc,
                if rule.enabled { "On" } else { "Off" },
                rect(365, top + 7, 425, top + 37),
                rule.enabled,
            );
        }
        text(
            dc,
            "Runs on this PC using Windows local time. The app must be running.",
            rect(24, 466, 436, 508),
            14,
            400,
            muted,
            DT_WORDBREAK,
        );
        button(dc, "+ New rule", rect(24, 526, 436, 570), true);
    } else if ui.page == 1 {
        if let Some(rule) = &ui.editing {
            button(
                dc,
                if rule.enabled { "Enabled" } else { "Disabled" },
                rect(308, 86, 436, 116),
                rule.enabled,
            );
            text(
                dc,
                "Name",
                rect(24, 117, 290, 140),
                14,
                600,
                muted,
                DT_SINGLELINE,
            );
            input_background(dc, rect(24, 139, 436, 179));
            text(
                dc,
                &rule.name,
                rect(36, 145, 424, 175),
                17,
                400,
                fg,
                DT_SINGLELINE,
            );
            button(
                dc,
                match rule.trigger {
                    Trigger::Time { .. } => "Trigger: daily time",
                    _ => "Trigger: sunset",
                },
                rect(24, 194, 436, 230),
                false,
            );
            text(
                dc,
                match rule.trigger {
                    Trigger::Time { .. } => "Local time (HH:MM)",
                    _ => "Minutes from sunset (-30 means before)",
                },
                rect(24, 236, 436, 257),
                14,
                600,
                muted,
                DT_SINGLELINE,
            );
            input_background(dc, rect(24, 254, 204, 295));
            let shown = match rule.trigger {
                Trigger::Time { hour, minute } => format!("{hour:02}:{minute:02}"),
                Trigger::Sunset { offset_minutes } => offset_minutes.to_string(),
            };
            text(
                dc,
                &shown,
                rect(36, 260, 192, 290),
                17,
                400,
                fg,
                DT_SINGLELINE,
            );
            button(
                dc,
                match rule.action.power {
                    Some(true) => "Power: on",
                    Some(false) => "Power: off",
                    None => "Power: keep current",
                },
                rect(24, 310, 436, 346),
                false,
            );
            text(
                dc,
                "Brightness (optional)",
                rect(24, 351, 436, 374),
                14,
                600,
                muted,
                DT_SINGLELINE,
            );
            input_background(dc, rect(24, 371, 180, 412));
            text(
                dc,
                &rule
                    .action
                    .brightness
                    .map(|b| b.to_string())
                    .unwrap_or_default(),
                rect(36, 377, 168, 407),
                17,
                400,
                fg,
                DT_SINGLELINE,
            );
            text(
                dc,
                "0–100%, or blank to keep",
                rect(195, 380, 436, 407),
                14,
                400,
                muted,
                DT_SINGLELINE,
            );
            button(
                dc,
                &format!(
                    "Colour: {}",
                    rule.action
                        .colour
                        .map(|[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"))
                        .unwrap_or_else(|| rule
                            .action
                            .preset
                            .clone()
                            .unwrap_or_else(|| "keep current".into()))
                ),
                rect(24, 426, 250, 462),
                false,
            );
            button(dc, "Pick colour…", rect(260, 426, 436, 462), false);
            button(dc, "Save rule", rect(24, 481, 220, 522), true);
            button(dc, "Cancel", rect(236, 481, 436, 522), false);
            button(dc, "Test these actions now", rect(24, 535, 320, 574), false);
            button(dc, "Delete", rect(336, 535, 436, 574), false);
        }
    } else {
        text(dc,"Windows supplies the timezone. Enter coordinates manually for sunset; no location service is used.",rect(24,136,436,174),16,400,muted,DT_WORDBREAK);
        text(
            dc,
            "Latitude (-90 to 90)",
            rect(24, 179, 436, 202),
            14,
            600,
            muted,
            DT_SINGLELINE,
        );
        input_background(dc, rect(24, 199, 436, 239));
        text(
            dc,
            &store
                .settings
                .latitude
                .map(|v| v.to_string())
                .unwrap_or_default(),
            rect(36, 205, 424, 235),
            17,
            400,
            fg,
            DT_SINGLELINE,
        );
        text(
            dc,
            "Longitude (-180 to 180)",
            rect(24, 269, 436, 292),
            14,
            600,
            muted,
            DT_SINGLELINE,
        );
        input_background(dc, rect(24, 289, 436, 329));
        text(
            dc,
            &store
                .settings
                .longitude
                .map(|v| v.to_string())
                .unwrap_or_default(),
            rect(36, 295, 424, 325),
            17,
            400,
            fg,
            DT_SINGLELINE,
        );
        button(
            dc,
            "Use Toronto coordinates",
            rect(24, 355, 436, 395),
            false,
        );
        button(dc, "Save location", rect(24, 421, 436, 465), true);
    }
    let note = if !ui.status.is_empty() {
        ui.status.clone()
    } else if ui.page == 1 {
        ui.editing
            .as_ref()
            .map(|r| upcoming(&store.settings, r))
            .unwrap_or_default()
    } else if !store.last.is_empty() {
        store.last.clone()
    } else {
        "Rules run once per day. Keep Windows awake for scheduled events.".into()
    };
    text(
        dc,
        &note,
        rect(24, 591, 436, 645),
        14,
        400,
        muted,
        DT_WORDBREAK,
    );
}
unsafe fn clicked(hwnd: HWND, owner: HWND, x: i32, y: i32) -> Result<(), String> {
    let page = UI.lock().unwrap().page;
    if page != 1 && (88..=123).contains(&y) {
        UI.lock().unwrap().page = if x < 228 { 0 } else { 2 };
        UI.lock().unwrap().status.clear();
        fields(hwnd);
        return Ok(());
    }
    if page == 0 {
        if (145..451).contains(&y) {
            let index = UI.lock().unwrap().offset + ((y - 145) / 51) as usize;
            let mut settings = STORE.get().unwrap().lock().unwrap().settings.clone();
            if let Some(rule) = settings.rules.get_mut(index) {
                if x >= 355 {
                    rule.enabled = !rule.enabled;
                    save(settings)?;
                } else {
                    let rule = rule.clone();
                    let mut ui = UI.lock().unwrap();
                    ui.page = 1;
                    ui.editing = Some(rule);
                    ui.status.clear();
                    drop(ui);
                    fields(hwnd);
                }
            }
        }
        if (526..570).contains(&y) {
            let mut ui = UI.lock().unwrap();
            ui.page = 1;
            ui.status.clear();
            ui.editing = Some(Rule {
                id: format!(
                    "r{}",
                    SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos()
                ),
                name: "Evening light".into(),
                enabled: false,
                trigger: Trigger::Time {
                    hour: 18,
                    minute: 0,
                },
                action: LightAction {
                    power: Some(true),
                    brightness: None,
                    preset: None,
                    colour: None,
                },
            });
            drop(ui);
            fields(hwnd);
        }
    } else if page == 1 {
        if (481..522).contains(&y) {
            if x < 228 {
                return persist_rule(hwnd);
            }
            UI.lock().unwrap().page = 0;
            UI.lock().unwrap().status.clear();
            fields(hwnd);
            return Ok(());
        }
        if (535..574).contains(&y) {
            let rule = draft(hwnd)?;
            if x < 328 {
                let mut settings = STORE.get().unwrap().lock().unwrap().settings.clone();
                settings.rules = vec![rule.clone()];
                bulb_automation::validate(&settings)?;
                post(owner, &rule.action);
                UI.lock().unwrap().status =
                    "Test actions queued. This does not enable the rule.".into();
            } else {
                let mut settings = STORE.get().unwrap().lock().unwrap().settings.clone();
                settings.rules.retain(|r| r.id != rule.id);
                save(settings)?;
                UI.lock().unwrap().page = 0;
                UI.lock().unwrap().status = "Rule deleted".into();
                fields(hwnd);
            }
            return Ok(());
        }
        if (86..116).contains(&y) && x >= 308 {
            let mut rule = draft(hwnd)?;
            rule.enabled = !rule.enabled;
            UI.lock().unwrap().editing = Some(rule);
        } else if (194..230).contains(&y) {
            let mut rule = draft(hwnd)?;
            rule.trigger = match rule.trigger {
                Trigger::Time { .. } => Trigger::Sunset { offset_minutes: 0 },
                _ => Trigger::Time {
                    hour: 18,
                    minute: 0,
                },
            };
            UI.lock().unwrap().editing = Some(rule);
            fields(hwnd);
        } else if (310..346).contains(&y) {
            let mut rule = draft(hwnd)?;
            rule.action.power = match rule.action.power {
                Some(true) => Some(false),
                Some(false) => None,
                None => Some(true),
            };
            if rule.action.power == Some(false) {
                rule.action.brightness = None;
            }
            if rule.action.power != Some(true) {
                rule.action.preset = None;
                rule.action.colour = None;
            }
            UI.lock().unwrap().editing = Some(rule);
            fields(hwnd);
        } else if (426..462).contains(&y) {
            let mut rule = draft(hwnd)?;
            if x >= 260 {
                let initial = rule.action.colour.unwrap_or([255, 255, 255]);
                if let Some(Action::Colour(r, g, b)) =
                    crate::tray::choose_colour_from(hwnd, (initial[0], initial[1], initial[2]))
                {
                    if [r, g, b] == [0, 0, 0] {
                        return Err("Use Power off instead of choosing black".into());
                    }
                    rule.action.colour = Some([r, g, b]);
                    rule.action.preset = None;
                    rule.action.power = Some(true);
                    UI.lock().unwrap().editing = Some(rule);
                }
                return Ok(());
            }
            rule.action.colour = None;
            let choices = [
                None,
                Some("warm"),
                Some("white"),
                Some("red"),
                Some("green"),
                Some("blue"),
                Some("purple"),
            ];
            let i = choices
                .iter()
                .position(|c| *c == rule.action.preset.as_deref())
                .unwrap_or(0);
            rule.action.preset = choices[(i + 1) % choices.len()].map(str::to_string);
            if rule.action.preset.is_some() {
                rule.action.power = Some(true);
            }
            UI.lock().unwrap().editing = Some(rule);
        }
    } else if page == 2 {
        if (355..395).contains(&y) {
            SetWindowTextW(GetDlgItem(hwnd, 1005), wide("43.6532").as_ptr());
            SetWindowTextW(GetDlgItem(hwnd, 1006), wide("-79.3832").as_ptr());
        }
        if (421..465).contains(&y) {
            let lat = value(hwnd, 1005);
            let lon = value(hwnd, 1006);
            let mut settings = STORE.get().unwrap().lock().unwrap().settings.clone();
            settings.latitude = if lat.is_empty() {
                None
            } else {
                Some(lat.parse().map_err(|_| "Invalid latitude")?)
            };
            settings.longitude = if lon.is_empty() {
                None
            } else {
                Some(lon.parse().map_err(|_| "Invalid longitude")?)
            };
            save(settings)?;
            UI.lock().unwrap().status = "Manual location saved".into();
        }
    }
    Ok(())
}
unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut p: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut p);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let memory = CreateCompatibleDC(dc);
            let bitmap = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
            let old = SelectObject(memory, bitmap);
            paint(memory, r.right, r.bottom);
            SetMapMode(memory, MM_TEXT);
            BitBlt(dc, 0, 0, r.right, r.bottom, memory, 0, 0, SRCCOPY);
            SelectObject(memory, old);
            DeleteObject(bitmap);
            DeleteDC(memory);
            EndPaint(hwnd, &p);
            0
        }
        WM_COMMAND if w & 0xffff == 901 => {
            let page = UI.lock().unwrap().page;
            let result = if page == 1 {
                persist_rule(hwnd)
            } else if page == 2 {
                clicked(hwnd, GetWindow(hwnd, GW_OWNER), 40, 440)
            } else {
                Ok(())
            };
            if let Err(e) = result {
                UI.lock().unwrap().status = e;
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_COMMAND if w & 0xffff == 902 => {
            let page = UI.lock().unwrap().page;
            if page == 1 {
                UI.lock().unwrap().page = 0;
                fields(hwnd);
            } else {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_GETMINMAXINFO => {
            let info = &mut *(l as *mut MINMAXINFO);
            let mut bounds = rect(0, 0, W, H);
            AdjustWindowRectEx(
                &mut bounds,
                WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
                0,
                WS_EX_TOOLWINDOW,
            );
            info.ptMinTrackSize = POINT {
                x: bounds.right - bounds.left,
                y: bounds.bottom - bounds.top,
            };
            0
        }
        WM_SIZE => {
            resize_fields(hwnd);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_ERASEBKGND => 1,
        WM_CTLCOLOREDIT => {
            let dc = w as HDC;
            SetTextColor(dc, rgb(237, 241, 247));
            SetBkColor(dc, rgb(31, 37, 49));
            static BRUSH: OnceLock<usize> = OnceLock::new();
            *BRUSH.get_or_init(|| CreateSolidBrush(rgb(31, 37, 49)) as usize) as LRESULT
        }
        WM_LBUTTONUP => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let x = (l as i16 as i32) * W / r.right.max(1);
            let y = ((l >> 16) as i16 as i32) * H / r.bottom.max(1);
            if let Err(error) = clicked(hwnd, GetWindow(hwnd, GW_OWNER), x, y) {
                UI.lock().unwrap().status = error;
            }
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let mut ui = UI.lock().unwrap();
            if ui.page == 0 {
                let count = STORE.get().unwrap().lock().unwrap().settings.rules.len();
                ui.offset = if ((w >> 16) as i16) < 0 {
                    (ui.offset + 1).min(count.saturating_sub(6))
                } else {
                    ui.offset.saturating_sub(1)
                };
            }
            drop(ui);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_KEYDOWN if w == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            *WINDOW.lock().unwrap() = 0;
            let mut ui = UI.lock().unwrap();
            ui.editing = None;
            ui.status.clear();
            0
        }
        WM_NCDESTROY => {
            let font = RemovePropW(hwnd, wide("RulesFont").as_ptr());
            if !font.is_null() {
                DeleteObject(font as HGDIOBJ);
            }
            DefWindowProcW(hwnd, msg, w, l)
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
pub unsafe fn close() {
    let window = *WINDOW.lock().unwrap();
    if window != 0 {
        DestroyWindow(window as HWND);
    }
}
pub unsafe fn show(owner: HWND) -> Result<(), String> {
    if STORE.get().is_none() {
        return Err("Automation settings are unavailable".into());
    }
    let current = *WINDOW.lock().unwrap();
    if current != 0 {
        ShowWindow(current as HWND, SW_SHOW);
        SetForegroundWindow(current as HWND);
        return Ok(());
    }
    let name = wide("SmartControlRules");
    let instance = GetModuleHandleW(null());
    let mut class: WNDCLASSW = std::mem::zeroed();
    class.lpfnWndProc = Some(procedure);
    class.hInstance = instance;
    class.lpszClassName = name.as_ptr();
    class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
    RegisterClassW(&class);
    let mut bounds = rect(0, 0, 600, 780);
    let style = WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN;
    AdjustWindowRectEx(&mut bounds, style, 0, 0);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW,
        name.as_ptr(),
        wide("SmartControl · Local rules").as_ptr(),
        style,
        CW_USEDEFAULT,
        CW_USEDEFAULT,
        bounds.right - bounds.left,
        bounds.bottom - bounds.top,
        owner,
        null_mut(),
        instance,
        null(),
    );
    if hwnd.is_null() {
        return Err("Cannot open rules editor".into());
    }
    *WINDOW.lock().unwrap() = hwnd as usize;
    UI.lock().unwrap().page = 0;
    fields(hwnd);
    ShowWindow(hwnd, SW_SHOW);
    ShowWindow(hwnd, SW_SHOW);
    SetForegroundWindow(hwnd);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn custom_colour_runs_before_brightness() {
        let action = LightAction {
            power: Some(true),
            colour: Some([17, 34, 255]),
            brightness: Some(37),
            ..Default::default()
        };
        assert!(matches!(
            actions(&action).as_slice(),
            [
                Action::Power(true),
                Action::Colour(17, 34, 255),
                Action::Brightness(37)
            ]
        ));
    }
    #[test]
    fn resizing_preserves_native_edit_contents_and_selection() {
        unsafe {
            let name = wide("SmartControlRuleResizeTest");
            let instance = GetModuleHandleW(null());
            let mut class: WNDCLASSW = std::mem::zeroed();
            class.lpfnWndProc = Some(procedure);
            class.hInstance = instance;
            class.lpszClassName = name.as_ptr();
            RegisterClassW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                name.as_ptr(),
                name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                500,
                700,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!hwnd.is_null());
            field(hwnd, 1001, "Evening light", 36, 145, 388);
            let edit = GetDlgItem(hwnd, 1001);
            SendMessageW(edit, windows_sys::Win32::UI::Controls::EM_SETSEL, 2, 6);
            let mut before: RECT = std::mem::zeroed();
            GetWindowRect(edit, &mut before);
            SetWindowPos(
                hwnd,
                null_mut(),
                0,
                0,
                800,
                1000,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
            let mut after: RECT = std::mem::zeroed();
            GetWindowRect(edit, &mut after);
            assert!(after.right - after.left > before.right - before.left);
            assert_eq!(value(hwnd, 1001), "Evening light");
            let mut start = 0u32;
            let mut end = 0u32;
            SendMessageW(
                edit,
                windows_sys::Win32::UI::Controls::EM_GETSEL,
                &mut start as *mut _ as usize,
                &mut end as *mut _ as isize,
            );
            assert_eq!((start, end), (2, 6));
            *WINDOW.lock().unwrap() = hwnd as usize;
            SendMessageW(hwnd, WM_CLOSE, 0, 0);
            assert_eq!(IsWindow(hwnd), 0);
            assert_eq!(*WINDOW.lock().unwrap(), 0);
            let reopened = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                name.as_ptr(),
                name.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                500,
                700,
                null_mut(),
                null_mut(),
                instance,
                null(),
            );
            assert!(!reopened.is_null());
            *WINDOW.lock().unwrap() = reopened as usize;
            close();
            close();
            assert_eq!(IsWindow(reopened), 0);
            assert_eq!(*WINDOW.lock().unwrap(), 0);
            UnregisterClassW(name.as_ptr(), instance);
        }
    }
    #[test]
    fn off_never_sends_a_colour_that_could_wake_the_bulb() {
        let action = LightAction {
            power: Some(false),
            brightness: Some(50),
            preset: Some("red".into()),
            colour: None,
        };
        assert!(matches!(
            actions(&action).as_slice(),
            [Action::Power(false)]
        ));
    }
    #[test]
    fn colour_precedes_final_brightness() {
        let action = LightAction {
            power: Some(true),
            brightness: Some(37),
            preset: Some("blue".into()),
            colour: None,
        };
        assert!(matches!(
            actions(&action).as_slice(),
            [
                Action::Power(true),
                Action::Preset("blue"),
                Action::Brightness(37)
            ]
        ));
    }
}

pub fn preview(path: &Path, page: u8) -> Result<(), String> {
    let rule = Rule {
        id: "example".into(),
        name: "Evening light".into(),
        enabled: false,
        trigger: Trigger::Sunset {
            offset_minutes: -30,
        },
        action: LightAction {
            power: Some(true),
            brightness: Some(50),
            preset: Some("warm".into()),
            colour: None,
        },
    };
    let settings = Settings {
        timezone: "local".into(),
        latitude: Some(43.6532),
        longitude: Some(-79.3832),
        rules: vec![rule.clone()],
    };
    STORE
        .set(Mutex::new(Store {
            settings,
            ledger: Ledger::default(),
            path: PathBuf::new(),
            ledger_path: PathBuf::new(),
            last: String::new(),
        }))
        .map_err(|_| "Preview already initialized")?;
    {
        let mut ui = UI.lock().unwrap();
        ui.page = page;
        ui.editing = Some(rule);
    }
    unsafe {
        let screen = GetDC(null_mut());
        let dc = CreateCompatibleDC(screen);
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = W;
        info.bmiHeader.biHeight = -H;
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let mut bits = null_mut();
        let bitmap = CreateDIBSection(dc, &info, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
        if bitmap.is_null() {
            return Err("Cannot render rules preview".into());
        }
        let old = SelectObject(dc, bitmap);
        paint(dc, W, H);
        GdiFlush();
        let bytes = std::slice::from_raw_parts(bits as *const u8, (W * H * 4) as usize);
        let mut output = Vec::new();
        output.extend_from_slice(b"BM");
        output.extend_from_slice(&(54u32 + bytes.len() as u32).to_le_bytes());
        output.extend_from_slice(&[0; 4]);
        output.extend_from_slice(&54u32.to_le_bytes());
        output.extend_from_slice(&40u32.to_le_bytes());
        output.extend_from_slice(&W.to_le_bytes());
        output.extend_from_slice(&(-H).to_le_bytes());
        output.extend_from_slice(&1u16.to_le_bytes());
        output.extend_from_slice(&32u16.to_le_bytes());
        output.extend_from_slice(&[0; 24]);
        output.extend_from_slice(bytes);
        std::fs::write(path, output).map_err(|_| "Cannot save rules preview")?;
        SelectObject(dc, old);
        DeleteObject(bitmap);
        DeleteDC(dc);
        ReleaseDC(null_mut(), screen);
    }
    Ok(())
}
