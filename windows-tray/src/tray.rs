use crate::backend::{Action, Config, Controller};
use std::{
    mem::size_of,
    path::PathBuf,
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender},
        Mutex, OnceLock,
    },
};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{Controls::Dialogs::*, Shell::*, WindowsAndMessaging::*},
};

const TRAY: u32 = WM_APP + 1;
const RESULT: u32 = WM_APP + 2;
static CHOOSING: AtomicBool = AtomicBool::new(false);
static PENDING: Mutex<crate::control::Pending> = Mutex::new(crate::control::Pending::new());
static JOBS: OnceLock<SyncSender<Action>> = OnceLock::new();
static LAST: Mutex<String> = Mutex::new(String::new());
static LABEL: OnceLock<String> = OnceLock::new();
static CONFIG_PATH: OnceLock<PathBuf> = OnceLock::new();
static BULB_ICON: AtomicUsize = AtomicUsize::new(0);
static TASKBAR_MESSAGE: OnceLock<u32> = OnceLock::new();
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn show_error(s: &str) {
    unsafe {
        MessageBoxW(
            null_mut(),
            wide(s).as_ptr(),
            wide("Bulb Bridge").as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}
fn icon(hwnd: HWND, message: &str) -> NOTIFYICONDATAW {
    let mut data: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
    data.hWnd = hwnd;
    data.uID = 1;
    data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
    data.uCallbackMessage = TRAY;
    unsafe {
        let handle = BULB_ICON.load(Ordering::Relaxed);
        data.hIcon = if handle == 0 {
            LoadIconW(null_mut(), IDI_APPLICATION)
        } else {
            handle as HICON
        };
    }
    for (i, c) in message.encode_utf16().take(127).enumerate() {
        data.szTip[i] = c;
    }
    data
}
unsafe fn refresh_icon(state: Option<&crate::backend::Snapshot>) {
    let on = state.and_then(|s| s.power);
    let colour = if on == Some(false) {
        (110, 123, 145)
    } else {
        state.map(|s| s.rgb()).unwrap_or((255, 204, 107))
    };
    let mut pixels = [0u8; 32 * 32 * 4];
    let mut mask = [255u8; 128];
    for y in 0..32 {
        for x in 0..32 {
            let mut count = 0;
            let mut base_count = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let px = x as f64 + (sx as f64 + 0.5) / 4.0;
                    let py = y as f64 + (sy as f64 + 0.5) / 4.0;
                    let body = (px - 16.0).powi(2) + (py - 12.0).powi(2) < 64.0
                        || ((12.0..20.0).contains(&px) && (16.0..23.0).contains(&py));
                    let base = (12.0..20.0).contains(&px) && (24.0..28.0).contains(&py);
                    let ray = on == Some(true)
                        && (((15.0..17.0).contains(&px) && (0.0..3.0).contains(&py))
                            || ((py - 12.0).abs() < 1.0
                                && ((2.0..5.0).contains(&px) || (27.0..30.0).contains(&px))));
                    if body || base || ray {
                        count += 1;
                        if base {
                            base_count += 1;
                        }
                    }
                }
            }
            if count > 0 {
                let alpha = count * 255 / 16;
                let (r, g, b) = if base_count > count / 2 {
                    (183, 192, 206)
                } else {
                    colour
                };
                let i = (y * 32 + x) * 4;
                pixels[i] = (b as usize * alpha / 255) as u8;
                pixels[i + 1] = (g as usize * alpha / 255) as u8;
                pixels[i + 2] = (r as usize * alpha / 255) as u8;
                pixels[i + 3] = alpha as u8;
                mask[y * 4 + x / 8] &= !(0x80 >> (x % 8));
            }
        }
    }
    let icon = CreateIcon(
        GetModuleHandleW(null()),
        32,
        32,
        1,
        32,
        mask.as_ptr(),
        pixels.as_ptr(),
    );
    if !icon.is_null() {
        let old = BULB_ICON.swap(icon as usize, Ordering::Relaxed);
        if old != 0 {
            DestroyIcon(old as HICON);
        }
    }
}
unsafe fn add(menu: HMENU, id: usize, text: &str) {
    AppendMenuW(menu, MF_STRING, id, wide(text).as_ptr());
}
unsafe fn popup(hwnd: HWND) {
    let menu = CreatePopupMenu();
    let brightness = CreatePopupMenu();
    let colours = CreatePopupMenu();
    add(
        menu,
        1,
        if crate::panel::snapshot().and_then(|s| s.power) == Some(true) {
            "Turn off"
        } else {
            "Turn on"
        },
    );
    for n in [5, 10, 25, 50, 75, 100] {
        add(brightness, 100 + n, &format!("{n}%"));
    }
    AppendMenuW(
        menu,
        MF_POPUP,
        brightness as usize,
        wide("Brightness").as_ptr(),
    );
    for (id, label) in [
        (301, "Warm white (2700 K)"),
        (302, "White (5000 K)"),
        (303, "Red"),
        (304, "Green"),
        (305, "Blue"),
        (306, "Purple"),
        (307, "Choose colour..."),
    ] {
        add(colours, id, label);
    }
    AppendMenuW(menu, MF_POPUP, colours as usize, wide("Colour").as_ptr());
    AppendMenuW(menu, MF_SEPARATOR, 0, null());
    add(menu, 6, "Rename bulb...");
    add(menu, 7, "Connect / change account...");
    add(menu, 3, "Read status");
    add(menu, 4, "About / connection");
    add(menu, 5, "Quit");
    let mut point = POINT { x: 0, y: 0 };
    GetCursorPos(&mut point);
    SetForegroundWindow(hwnd);
    let id = TrackPopupMenu(
        menu,
        TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
        point.x,
        point.y,
        0,
        hwnd,
        null(),
    ) as usize;
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);
    let action = match id {
        1 => Some(Action::Toggle),
        2 => Some(Action::Power(false)),
        7 => {
            if let Ok(exe) = std::env::current_exe() {
                use std::os::windows::process::CommandExt;
                let mut command = std::process::Command::new(exe);
                command.arg("--setup");
                if let Some(path) = CONFIG_PATH.get() {
                    command.arg(path);
                }
                match command.creation_flags(0x08000000).spawn() {
                    Ok(_) => {
                        DestroyWindow(hwnd);
                    }
                    Err(_) => show_error("Cannot open account setup"),
                }
            }
            None
        }
        6 => {
            crate::panel::begin_rename();
            None
        }
        3 => Some(Action::Status),
        4 => {
            let last = LAST.lock().unwrap().clone();
            MessageBoxW(hwnd,wide(&format!("{}\nNative Rust tray controller\n\n{}\n\nCommands are cloud requests; acceptance is not physical confirmation.\nBrightness retains the current colour. Colour selections also turn on the light.",LABEL.get().unwrap(),last)).as_ptr(),wide("Bulb Bridge").as_ptr(),MB_OK);
            None
        }
        5 => {
            DestroyWindow(hwnd);
            None
        }
        105 | 110 | 125 | 150 | 175 | 200 => Some(Action::Brightness((id - 100) as u8)),
        301 => Some(Action::Preset("warm")),
        302 => Some(Action::Preset("white")),
        303 => Some(Action::Preset("red")),
        304 => Some(Action::Preset("green")),
        305 => Some(Action::Preset("blue")),
        306 => Some(Action::Preset("purple")),
        307 => choose_colour(hwnd),
        _ => None,
    };
    if let Some(action) = action {
        enqueue(hwnd, action);
    }
}
pub fn choosing_colour() -> bool {
    CHOOSING.load(Ordering::Relaxed)
}
pub unsafe fn choose_colour(hwnd: HWND) -> Option<Action> {
    CHOOSING.store(true, Ordering::Relaxed);
    let mut custom = [0u32; 16];
    let mut choose: CHOOSECOLORW = std::mem::zeroed();
    choose.lStructSize = size_of::<CHOOSECOLORW>() as u32;
    choose.hwndOwner = hwnd;
    choose.lpCustColors = custom.as_mut_ptr();
    choose.Flags = CC_FULLOPEN | CC_RGBINIT;
    let selected = ChooseColorW(&mut choose) != 0;
    CHOOSING.store(false, Ordering::Relaxed);
    if selected {
        Some(Action::Colour(
            (choose.rgbResult & 255) as u8,
            ((choose.rgbResult >> 8) & 255) as u8,
            ((choose.rgbResult >> 16) & 255) as u8,
        ))
    } else {
        None
    }
}
pub fn enqueue(hwnd: HWND, action: Action) {
    unsafe {
        KillTimer(hwnd, 2);
    }
    let action = if matches!(action, Action::Toggle) {
        crate::panel::recent_power()
            .map(|on| Action::Power(!on))
            .unwrap_or(action)
    } else {
        action
    };
    if crate::panel::busy() {
        PENDING.lock().unwrap().push(action);
        crate::panel::update(None, "Latest change queued…".into(), true, false);
        return;
    }
    crate::panel::update(None, "Working…".into(), true, false);
    if JOBS.get().unwrap().try_send(action).is_err() {
        crate::panel::update(None, "Request unavailable. Try again.".into(), false, true);
    }
    unsafe {
        Shell_NotifyIconW(NIM_MODIFY, &icon(hwnd, "Bulb Bridge: working…"));
    }
}
struct Reply {
    ok: bool,
    message: String,
    show: bool,
    state: Option<crate::backend::Snapshot>,
}

unsafe extern "system" fn wndproc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        TRAY if lparam as u32 == WM_LBUTTONUP => {
            crate::panel::toggle_visibility();
            0
        }
        TRAY if lparam as u32 == WM_RBUTTONUP => {
            popup(hwnd);
            0
        }
        RESULT => {
            let result = Box::from_raw(lparam as *mut Reply);
            crate::panel::update(
                result.state.clone(),
                result.message.clone(),
                false,
                !result.ok,
            );
            if result.state.is_some() {
                refresh_icon(result.state.as_ref());
            }
            *LAST.lock().unwrap() = result.message.clone();
            let tip = format!(
                "Bulb Bridge: {}",
                result.message.lines().next().unwrap_or("Ready")
            );
            Shell_NotifyIconW(NIM_MODIFY, &icon(hwnd, &tip));
            if result.show {
                MessageBoxW(
                    hwnd,
                    wide(&result.message).as_ptr(),
                    wide("Bulb Bridge").as_ptr(),
                    MB_OK
                        | if result.ok {
                            MB_ICONINFORMATION
                        } else {
                            MB_ICONERROR
                        },
                );
            }
            if result.message.starts_with("Sent; confirmation delayed") {
                SetTimer(hwnd, 2, 5000, None);
            }
            let next = PENDING.lock().unwrap().pop();
            if let Some(action) = next {
                enqueue(hwnd, action);
            }
            0
        }
        WM_TIMER if wparam == 2 => {
            KillTimer(hwnd, 2);
            enqueue(hwnd, Action::Refresh);
            0
        }
        WM_DESTROY => {
            Shell_NotifyIconW(NIM_DELETE, &icon(hwnd, ""));
            PostQuitMessage(0);
            0
        }
        _ if TASKBAR_MESSAGE.get() == Some(&message) => {
            Shell_NotifyIconW(NIM_ADD, &icon(hwnd, "Bulb Bridge: ready"));
            0
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}
pub fn rename(label: &str) -> Result<(), String> {
    let label = label.trim();
    if label.is_empty() || label.chars().count() > 60 {
        return Err("Use a name between 1 and 60 characters.".into());
    }
    let path = CONFIG_PATH.get().ok_or("Configuration unavailable")?;
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|_| "Cannot read configuration")?)
            .map_err(|_| "Invalid configuration")?;
    config["label"] = serde_json::Value::String(label.into());
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, serde_json::to_vec_pretty(&config).unwrap())
        .map_err(|_| "Cannot save name")?;
    std::fs::rename(&temporary, path).map_err(|_| "Cannot replace configuration")?;
    Ok(())
}
pub fn run() -> Result<(), String> {
    let force = std::env::args().nth(1).as_deref() == Some("--setup");
    let explicit = std::env::args_os().nth(if force { 2 } else { 1 });
    let path = match explicit {
        Some(path) => PathBuf::from(path),
        None => crate::setup::default_config()?,
    };
    if force || !path.exists() {
        if !crate::setup::run(&path)? {
            return Ok(());
        }
    }
    let config: Config = serde_json::from_slice(
        &std::fs::read(&path).map_err(|_| "Put client.json beside bulb-tray.exe (see README)")?,
    )
    .map_err(|_| "Invalid client.json")?;
    CONFIG_PATH.set(path).ok();
    let controller = Controller::new(config.clone())?;
    LABEL
        .set(config.label.clone())
        .map_err(|_| "Tray already initialized")?;
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
        // A per-user-session mutex prevents duplicate tray instances.
        let singleton = CreateMutexW(null(), 0, wide("Local\\BulbBridgeTray").as_ptr());
        if singleton.is_null() {
            return Err("Cannot create application lock".into());
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(singleton);
            return Ok(());
        }
        let instance = GetModuleHandleW(null());
        let name = wide("BulbBridgeTrayWindow");
        TASKBAR_MESSAGE
            .set(RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()))
            .ok();
        refresh_icon(None);
        let mut class: WNDCLASSW = std::mem::zeroed();
        class.lpfnWndProc = Some(wndproc);
        class.hInstance = instance;
        class.lpszClassName = name.as_ptr();
        if RegisterClassW(&class) == 0 {
            return Err("Cannot register tray window".into());
        }
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("Bulb Bridge").as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err("Cannot create tray window".into());
        }
        let (tx, rx) = mpsc::sync_channel(1);
        JOBS.set(tx)
            .map_err(|_| "Request worker already initialized")?;
        crate::panel::create(hwnd, &config.label)?;
        let hwnd_value = hwnd as usize;
        std::thread::spawn(move || {
            let mut previous = None::<crate::backend::Snapshot>;
            let mut last_write = None::<std::time::Instant>;
            for mut action in rx {
                if matches!(action, Action::Toggle) {
                    if let Some(on) = previous.as_ref().and_then(|s| s.power) {
                        action = Action::Power(!on);
                    }
                }

                let show = matches!(action, Action::Status);
                let read = matches!(action, Action::Status | Action::Refresh);
                let (ok, message, state) = if read {
                    match controller.snapshot() {
                        Ok(s) => {
                            let note = if show {
                                s.describe()
                            } else {
                                "Connected".into()
                            };
                            (true, note, Some(s))
                        }
                        Err(e) => (false, e, None),
                    }
                } else {
                    if let Some(last) = last_write {
                        std::thread::sleep(
                            std::time::Duration::from_millis(750).saturating_sub(last.elapsed()),
                        );
                    }
                    let wake = if crate::control::is_colour(&action)
                        && previous.as_ref().and_then(|s| s.power) == Some(false)
                    {
                        controller.execute(Action::Power(true)).map(|_| {
                            std::thread::sleep(std::time::Duration::from_millis(500));
                        })
                    } else {
                        Ok(())
                    };
                    let outcome = wake.and_then(|_| controller.execute(action.clone()));
                    last_write = Some(std::time::Instant::now());
                    match outcome {
                        Ok(_) => {
                            let mut latest = None;
                            let mut confirmed = false;
                            for delay in [350, 700, 1200, 1800] {
                                std::thread::sleep(std::time::Duration::from_millis(delay));
                                match controller.snapshot() {
                                    Ok(state) => {
                                        confirmed = crate::control::matches(&action, &state);
                                        latest = Some(state);
                                        if confirmed {
                                            break;
                                        }
                                    }
                                    Err(_) => break,
                                }
                            }
                            (
                                true,
                                if confirmed {
                                    "Connected".into()
                                } else {
                                    "Sent; confirmation delayed. Refresh to check.".into()
                                },
                                latest,
                            )
                        }
                        Err(error) => (false, error, None),
                    }
                };
                if let Some(ref snapshot) = state {
                    previous = Some(snapshot.clone());
                }
                let message = Box::new(Reply {
                    ok,
                    message,
                    show,
                    state,
                });
                let ptr = Box::into_raw(message);
                if PostMessageW(hwnd_value as HWND, RESULT, 0, ptr as LPARAM) == 0 {
                    drop(Box::from_raw(ptr));
                }
            }
        });
        if Shell_NotifyIconW(
            NIM_ADD,
            &icon(hwnd, &format!("Bulb Bridge: {}", config.label)),
        ) == 0
        {
            return Err("Cannot add system tray icon".into());
        }
        *LAST.lock().unwrap() = "Ready. Click the tray icon for controls.".into();
        enqueue(hwnd, Action::Refresh);
        let mut message: MSG = std::mem::zeroed();
        loop {
            let result = GetMessageW(&mut message, null_mut(), 0, 0);
            if result == 0 {
                break;
            }
            if result < 0 {
                return Err("Windows message loop failed".into());
            }
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        CloseHandle(singleton);
        let handle = BULB_ICON.swap(0, Ordering::Relaxed);
        if handle != 0 {
            DestroyIcon(handle as HICON);
        }
    }
    Ok(())
}

#[cfg(test)]
mod rename_tests {
    use super::*;
    #[test]
    fn rename_persists_label_and_preserves_connection() {
        let path = std::env::temp_dir().join(format!("bulb-rename-{}.json", std::process::id()));
        std::fs::write(&path,r#"{"label":"Old","backend":"bridge","server_url":"https://example.com","access_key":"fake"}"#).unwrap();
        CONFIG_PATH.set(path.clone()).unwrap();
        rename("  Desk light  ").unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["label"], "Desk light");
        assert_eq!(value["access_key"], "fake");
        assert!(rename(" ").is_err());
        assert!(rename(&"x".repeat(61)).is_err());
        std::fs::remove_file(path).unwrap();
    }
}
