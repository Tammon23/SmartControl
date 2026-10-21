use crate::panel::{fill, rect, rgb, text};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    net::TcpListener,
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::LibraryLoader::*,
    UI::{HiDpi::*, Shell::ShellExecuteW, WindowsAndMessaging::*},
};

// Public desktop PKCE registration used by the provider's official CLI. No client secret.
const CLIENT: &str = "d18cf96e-c626-4433-bf51-ddbb10c5d1ed";
const DONE: u32 = WM_APP + 40;
static MODEL: Mutex<Model> = Mutex::new(Model {
    phase: 0,
    message: String::new(),
    auth: None,
    devices: Vec::new(),
    selected: 0,
    offset: 0,
    saved: false,
});
static CANCEL: AtomicBool = AtomicBool::new(false);
static PATH: OnceLock<PathBuf> = OnceLock::new();
struct Model {
    phase: u8,
    message: String,
    auth: Option<Value>,
    devices: Vec<Device>,
    selected: usize,
    offset: usize,
    saved: bool,
}
#[derive(Clone)]
struct Device {
    id: String,
    label: String,
    component: String,
}
struct Login {
    auth: Value,
    devices: Vec<Device>,
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
pub fn default_config() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA").ok_or("Cannot locate Windows settings folder")?;
    let user = PathBuf::from(local).join("SmartControl/client.json");
    let legacy = std::env::current_exe()
        .map_err(|_| "Cannot locate application")?
        .with_file_name("client.json");
    Ok(if user.exists() || !legacy.exists() {
        user
    } else {
        legacy
    })
}
fn nonce() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
fn callback(target: &str, state: &str) -> Result<String, String> {
    let url = reqwest::Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| "Invalid callback")?;
    if url.path() != "/finish" {
        return Err("Invalid callback path".into());
    }
    let pairs: Vec<_> = url.query_pairs().collect();
    let states: Vec<_> = pairs.iter().filter(|(k, _)| k == "state").collect();
    if states.len() != 1 || states[0].1 != state {
        return Err("Invalid authorization state".into());
    }
    if pairs.iter().any(|(k, _)| k == "error") {
        return Err("Authorization was declined. Try again when ready.".into());
    }
    let codes: Vec<_> = pairs.iter().filter(|(k, _)| k == "code").collect();
    if codes.len() != 1 || codes[0].1.is_empty() {
        return Err("Authorization code is missing".into());
    }
    Ok(codes[0].1.to_string())
}
fn login() -> Result<Login, String> {
    let listener = [61973, 61974, 61975]
        .into_iter()
        .find_map(|port| TcpListener::bind(("127.0.0.1", port)).ok())
        .ok_or("Sign-in ports are busy. Close another sign-in window and retry.")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "Cannot start sign-in listener")?;
    let port = listener.local_addr().unwrap().port();
    let redirect = format!("http://localhost:{port}/finish");
    let verifier = nonce();
    let state = nonce();
    let pkce = challenge(&verifier);
    let mut url =
        reqwest::Url::parse("https://oauthin-regional.api.smartthings.com/oauth/authorize")
            .unwrap();
    url.query_pairs_mut().extend_pairs([
        ("scope", "controller:stCli"),
        ("response_type", "code"),
        ("client_id", CLIENT),
        ("code_challenge", &pkce),
        ("code_challenge_method", "S256"),
        ("redirect_uri", &redirect),
        ("client_type", "USER_LEVEL"),
        ("state", &state),
    ]);
    unsafe {
        if ShellExecuteW(
            null_mut(),
            wide("open").as_ptr(),
            wide(url.as_str()).as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        ) as isize
            <= 32
        {
            return Err("Cannot open the browser".into());
        }
    }
    let started = Instant::now();
    let code = loop {
        if CANCEL.load(Ordering::Relaxed) {
            return Err("Sign-in cancelled".into());
        }
        if started.elapsed() > Duration::from_secs(600) {
            return Err("Sign-in timed out. Connect again to retry.".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .ok();
                stream.set_write_timeout(Some(Duration::from_secs(1))).ok();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while request.len() < 8192 {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            request.extend_from_slice(&buf[..n]);
                            if request.windows(4).any(|x| x == b"\r\n\r\n") {
                                break;
                            }
                        }
                    }
                }
                let request = String::from_utf8_lossy(&request);
                let mut first = request.lines().next().unwrap_or("").split_whitespace();
                let method = first.next().unwrap_or("");
                let target = first.next().unwrap_or("");
                let valid_host = request
                    .lines()
                    .any(|line| line.to_ascii_lowercase() == format!("host: localhost:{port}"));
                let result = if method == "GET" && valid_host {
                    callback(target, &state)
                } else {
                    Err("Invalid callback request".into())
                };
                let ok = result.is_ok();
                let declined = result
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.starts_with("Authorization was declined"));
                let html = if ok {
                    "<!doctype html><title>SmartControl</title><h1>Authorization received</h1><p>Return to SmartControl to select your bulb. You can close this window.</p>"
                } else {
                    "<!doctype html><title>SmartControl</title><h1>Sign-in was not completed</h1><p>Return to SmartControl. No account data was saved from this request.</p>"
                };
                let _=write!(stream,"HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",if ok{"200 OK"}else{"400 Bad Request"},html.len(),html);
                if ok {
                    break result.unwrap();
                }
                if declined {
                    return Err(result.unwrap_err());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50))
            }
            Err(_) => return Err("Sign-in listener failed".into()),
        }
    };
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot connect")?;
    let response = http
        .post("https://oauthin-regional.api.smartthings.com/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", CLIENT),
            ("code_verifier", &verifier),
            ("code", &code),
            ("redirect_uri", &redirect),
        ])
        .send()
        .map_err(|_| "Sign-in exchange timed out")?;
    if !response.status().is_success() {
        return Err("The provider could not complete authorization. Please retry.".into());
    }
    let token: Value = response
        .json()
        .map_err(|_| "Invalid authorization response")?;
    let access = token["access_token"]
        .as_str()
        .ok_or("Authorization did not return access")?;
    let refresh = token["refresh_token"]
        .as_str()
        .ok_or("Authorization did not return renewal access")?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let auth = json!({"access_token":access,"refresh_token":refresh,"expires_at":now+token["expires_in"].as_u64().unwrap_or(3600)});
    let mut devices = Vec::new();
    let mut next = Some("https://api.smartthings.com/v1/devices".to_string());
    for _ in 0..20 {
        if CANCEL.load(Ordering::Relaxed) {
            return Err("Sign-in cancelled".into());
        }
        let Some(page) = next.take() else { break };
        let url = reqwest::Url::parse(&page).map_err(|_| "Invalid device list URL")?;
        if url.scheme() != "https" || url.host_str() != Some("api.smartthings.com") {
            return Err("Unexpected device list URL".into());
        }
        let response = http
            .get(url)
            .bearer_auth(access)
            .send()
            .map_err(|_| "Cannot load your bulbs")?;
        if !response.status().is_success() {
            return Err("Device access was not granted. Please reconnect your account.".into());
        }
        let page: Value = response.json().map_err(|_| "Invalid device list")?;
        devices.extend(bulbs(&page));
        next = page["_links"]["next"]["href"].as_str().map(|s| {
            if s.starts_with('/') {
                format!("https://api.smartthings.com{s}")
            } else {
                s.to_string()
            }
        });
    }
    devices.sort_by_key(|d| d.label.to_lowercase());
    devices.dedup_by(|a, b| a.id == b.id && a.component == b.component);
    if devices.is_empty() {
        return Err(
            "No compatible bulbs found. Check the account and device connection, then reconnect."
                .into(),
        );
    }
    Ok(Login { auth, devices })
}
fn bulbs(page: &Value) -> Vec<Device> {
    let mut devices = Vec::new();
    for device in page["items"].as_array().into_iter().flatten() {
        for component in device["components"].as_array().into_iter().flatten() {
            let caps: Vec<_> = component["capabilities"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|c| c["id"].as_str())
                .collect();
            if caps.contains(&"switch")
                && (caps.contains(&"switchLevel") || caps.contains(&"colorControl"))
            {
                if let (Some(id), Some(c)) = (device["deviceId"].as_str(), component["id"].as_str())
                {
                    devices.push(Device {
                        id: id.into(),
                        label: device["label"]
                            .as_str()
                            .or(device["name"].as_str())
                            .unwrap_or("Bulb")
                            .into(),
                        component: c.into(),
                    });
                    break;
                }
            }
        }
    }
    devices
}
unsafe fn paint(dc: HDC, w: i32, h: i32) {
    SetMapMode(dc, MM_ANISOTROPIC);
    SetWindowExtEx(dc, 440, 520, null_mut());
    SetViewportExtEx(dc, w, h, null_mut());
    let m = MODEL.lock().unwrap();
    let fg = rgb(237, 241, 247);
    let muted = rgb(145, 155, 173);
    let accent = rgb(255, 204, 107);
    fill(dc, rect(0, 0, 440, 520), rgb(20, 24, 33), 0);
    text(
        dc,
        "SMARTCONTROL",
        rect(28, 22, 412, 46),
        12,
        600,
        muted,
        DT_SINGLELINE,
    );
    text(
        dc,
        if m.phase == 2 {
            "Choose your bulb"
        } else {
            "Connect your account"
        },
        rect(28, 63, 412, 104),
        25,
        600,
        fg,
        DT_SINGLELINE,
    );
    text(
        dc,
        if m.phase == 2 {
            "Select a bulb, then save the connection."
        } else {
            "Sign in securely in your browser. Your password stays with the account provider."
        },
        rect(28, 112, 412, 162),
        14,
        400,
        muted,
        DT_WORDBREAK,
    );
    if m.phase == 2 {
        for (row, device) in m.devices.iter().skip(m.offset).take(5).enumerate() {
            let index = row + m.offset;
            let y = 170 + row as i32 * 46;
            fill(
                dc,
                rect(28, y, 412, y + 40),
                if index == m.selected {
                    accent
                } else {
                    rgb(37, 44, 58)
                },
                10,
            );
            text(
                dc,
                &device.label,
                rect(40, y, 400, y + 40),
                15,
                500,
                if index == m.selected {
                    rgb(25, 27, 34)
                } else {
                    fg
                },
                DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
        text(
            dc,
            &format!("{} bulb(s) · scroll to see more", m.devices.len()),
            rect(28, 403, 412, 424),
            11,
            400,
            muted,
            DT_SINGLELINE,
        );
    } else {
        text(
            dc,
            &m.message,
            rect(28, 183, 412, 370),
            14,
            400,
            fg,
            DT_WORDBREAK,
        );
    }
    fill(
        dc,
        rect(28, 436, 412, 480),
        if m.phase == 1 {
            rgb(55, 62, 77)
        } else {
            accent
        },
        12,
    );
    text(
        dc,
        match m.phase {
            1 => "Waiting for browser…",
            2 => "Save connection",
            _ => "Connect account",
        },
        rect(28, 436, 412, 480),
        15,
        600,
        if m.phase == 1 { muted } else { rgb(25, 27, 34) },
        DT_SINGLELINE | DT_CENTER | DT_VCENTER,
    );
}
unsafe extern "system" fn procedure(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut p: PAINTSTRUCT = std::mem::zeroed();
            let dc = BeginPaint(hwnd, &mut p);
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let mem = CreateCompatibleDC(dc);
            let bitmap = CreateCompatibleBitmap(dc, r.right.max(1), r.bottom.max(1));
            let old = SelectObject(mem, bitmap);
            paint(mem, r.right, r.bottom);
            SetMapMode(mem, MM_TEXT);
            BitBlt(dc, 0, 0, r.right, r.bottom, mem, 0, 0, SRCCOPY);
            SelectObject(mem, old);
            DeleteObject(bitmap);
            DeleteDC(mem);
            EndPaint(hwnd, &p);
            0
        }
        WM_ERASEBKGND => 1,
        DONE => {
            let result = Box::from_raw(l as *mut Result<Login, String>);
            let mut m = MODEL.lock().unwrap();
            match *result {
                Ok(login) => {
                    m.phase = 2;
                    m.auth = Some(login.auth);
                    m.devices = login.devices;
                    m.selected = 0;
                    m.offset = 0;
                }
                Err(error) => {
                    m.phase = 0;
                    m.message = error;
                }
            }
            drop(m);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_LBUTTONUP => {
            let mut r: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut r);
            let x = (l as i16 as i32) * 440 / r.right.max(1);
            let y = ((l >> 16) as i16 as i32) * 520 / r.bottom.max(1);
            let mut m = MODEL.lock().unwrap();
            if m.phase == 2 && (170..400).contains(&y) {
                let index = m.offset + ((y - 170) / 46) as usize;
                if index < m.devices.len() {
                    m.selected = index;
                }
            }
            if (28..412).contains(&x) && (436..480).contains(&y) {
                if m.phase == 0 {
                    m.phase = 1;
                    m.message="Complete authorization in your browser, then return here to select a bulb.".into();
                    let handle = hwnd as usize;
                    std::thread::spawn(move || {
                        let result = Box::into_raw(Box::new(login()));
                        unsafe {
                            if PostMessageW(handle as HWND, DONE, 0, result as LPARAM) == 0 {
                                drop(Box::from_raw(result));
                            }
                        }
                    });
                } else if m.phase == 2 {
                    let result = (|| -> Result<(), String> {
                        let device = &m.devices[m.selected];
                        let local = PathBuf::from(
                            std::env::var_os("LOCALAPPDATA").ok_or("Cannot locate settings")?,
                        );
                        let auth_path = local.join("SmartControl/auth.bin");
                        crate::credentials::save(
                            &auth_path,
                            m.auth.as_ref().ok_or("Missing sign-in")?,
                        )?;
                        let config = json!({"label":device.label,"backend":"direct_cloud","credentials_path":auth_path,"device_id":device.id,"component":device.component});
                        let path = PATH.get().unwrap();
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)
                                .map_err(|_| "Cannot create settings")?;
                        }
                        let temporary = path.with_extension("pending");
                        std::fs::write(&temporary, serde_json::to_vec_pretty(&config).unwrap())
                            .map_err(|_| "Cannot save connection")?;
                        std::fs::rename(temporary, path)
                            .map_err(|_| "Cannot replace connection")?;
                        Ok(())
                    })();
                    match result {
                        Ok(()) => {
                            m.saved = true;
                            m.auth = None;
                            drop(m);
                            DestroyWindow(hwnd);
                            return 0;
                        }
                        Err(e) => {
                            m.message = e;
                            m.phase = 0;
                        }
                    }
                }
            }
            drop(m);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_MOUSEWHEEL => {
            let mut m = MODEL.lock().unwrap();
            if m.phase == 2 {
                let delta = (w >> 16) as i16;
                if delta < 0 {
                    m.offset = (m.offset + 1).min(m.devices.len().saturating_sub(5));
                } else {
                    m.offset = m.offset.saturating_sub(1);
                }
            }
            drop(m);
            InvalidateRect(hwnd, null(), 0);
            0
        }
        WM_CLOSE => {
            CANCEL.store(true, Ordering::Relaxed);
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            0
        }
        _ => DefWindowProcW(hwnd, msg, w, l),
    }
}
pub fn run(path: &Path) -> Result<bool, String> {
    unsafe {
        PATH.set(path.into()).map_err(|_| "Setup already opened")?;
        CANCEL.store(false, Ordering::Relaxed);
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        MODEL.lock().unwrap().message="Connect the account you use to control your bulb. No light settings will be changed during setup.".into();
        let instance = GetModuleHandleW(null());
        let name = wide("SmartControlSetup");
        let mut class: WNDCLASSW = std::mem::zeroed();
        class.lpfnWndProc = Some(procedure);
        class.hInstance = instance;
        class.lpszClassName = name.as_ptr();
        class.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        if RegisterClassW(&class) == 0 {
            return Err("Cannot open setup".into());
        }
        let dpi = GetDpiForSystem().max(96) as i32;
        let mut bounds = rect(0, 0, 440 * dpi / 96, 520 * dpi / 96);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        AdjustWindowRectEx(&mut bounds, style, 0, 0);
        let hwnd = CreateWindowExW(
            0,
            name.as_ptr(),
            wide("SmartControl · Set up your bulb").as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            bounds.right - bounds.left,
            bounds.bottom - bounds.top,
            null_mut(),
            null_mut(),
            instance,
            null(),
        );
        if hwnd.is_null() {
            return Err("Cannot create setup window".into());
        }
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let value = GetMessageW(&mut msg, null_mut(), 0, 0);
            if value == 0 {
                break;
            }
            if value < 0 {
                return Err("Setup window failed".into());
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        Ok(MODEL.lock().unwrap().saved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pkce_rfc7636_vector() {
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
    #[test]
    fn callback_requires_state_and_rejects_errors_and_duplicate_parameters() {
        assert_eq!(
            callback("/finish?state=nonce&code=abc", "nonce").unwrap(),
            "abc"
        );
        for target in [
            "/finish?code=abc",
            "/finish?state=other&code=abc",
            "/finish?state=nonce&state=nonce&code=abc",
            "/finish?state=nonce&code=a&code=b",
            "/finish?state=nonce&error=access_denied",
            "/other?state=nonce&code=a",
        ] {
            assert!(callback(target, "nonce").is_err());
        }
    }
    #[test]
    fn selects_compatible_components_only() {
        let page = json!({"items":[{"deviceId":"bulb","label":"Desk","components":[{"id":"main","capabilities":[{"id":"switch"},{"id":"switchLevel"}]}]},{"deviceId":"sensor","components":[{"id":"main","capabilities":[{"id":"temperatureMeasurement"}]}]}]});
        let result = bulbs(&page);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, "bulb");
        assert_eq!(result[0].component, "main");
    }
}
