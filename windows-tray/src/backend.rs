use serde::Deserialize;
use serde_json::{json, Value};
use std::{path::PathBuf, time::Duration};

#[derive(Deserialize, Clone)]
pub struct Config {
    pub label: String,
    #[serde(flatten)]
    pub backend: Backend,
}
#[derive(Deserialize, Clone)]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum Backend {
    DirectCloud {
        credentials_path: PathBuf,
        device_id: String,
        #[serde(default = "component")]
        component: String,
    },
    Bridge {
        server_url: String,
        access_key: String,
    },
}
fn component() -> String {
    "main".into()
}
#[derive(Clone, Debug)]
pub enum Action {
    Toggle,
    Refresh,
    Power(bool),
    Brightness(u8),
    Colour(u8, u8, u8),
    Preset(&'static str),
    Status,
}

#[derive(Clone, Default, Debug, Deserialize)]
pub struct Snapshot {
    pub power: Option<bool>,
    pub brightness: Option<u8>,
    pub hue: Option<f64>,
    pub saturation: Option<f64>,
    pub temperature: Option<u32>,
    pub online: Option<bool>,
    pub timestamp: Option<String>,
    #[serde(default)]
    pub demo: bool,
}
impl Snapshot {
    pub fn describe(&self) -> String {
        format!(
            "{}Power: {} | Brightness: {}%\nTemperature: {} K | Online: {}\nCloud timestamp: {}",
            if self.demo { "DEMO\n" } else { "" },
            match self.power {
                Some(true) => "On",
                Some(false) => "Off",
                None => "Unknown",
            },
            self.brightness
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
            self.temperature
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into()),
            self.online
                .map(|v| v.to_string())
                .unwrap_or_else(|| "unknown".into()),
            self.timestamp.as_deref().unwrap_or("not supplied")
        )
    }
    pub fn rgb(&self) -> (u8, u8, u8) {
        if self.saturation.unwrap_or(0.0) < 5.0 {
            return if self.temperature.unwrap_or(5000) < 4000 {
                (255, 190, 92)
            } else {
                (255, 225, 151)
            };
        }
        let h = self.hue.unwrap_or(0.0).rem_euclid(100.0) / 100.0 * 6.0;
        let s = (self.saturation.unwrap_or(100.0) / 100.0).clamp(0.0, 1.0);
        let x = 1.0 - s;
        let f = h.fract();
        let (r, g, b) = match h as u8 {
            0 => (1.0, 1.0 - s * (1.0 - f), x),
            1 => (1.0 - s * f, 1.0, x),
            2 => (x, 1.0, 1.0 - s * (1.0 - f)),
            3 => (x, 1.0 - s * f, 1.0),
            4 => (1.0 - s * (1.0 - f), x, 1.0),
            _ => (1.0, x, 1.0 - s * f),
        };
        ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
    }
}

pub struct Controller {
    config: Config,
    http: reqwest::blocking::Client,
}
impl Controller {
    pub fn new(config: Config) -> Result<Self, String> {
        match &config.backend {
            Backend::DirectCloud {
                credentials_path,
                device_id,
                component,
            } => {
                if !credentials_path.is_file()
                    || device_id.len() != 36
                    || !device_id
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() || b == b'-')
                    || component.is_empty()
                {
                    return Err("Invalid direct connection configuration".into());
                }
            }
            Backend::Bridge {
                server_url,
                access_key,
            } => {
                let url = reqwest::Url::parse(server_url).map_err(|_| "Invalid server URL")?;
                if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
                    return Err("Use an HTTP(S) server URL".into());
                }
                if access_key.len() < 32 {
                    return Err("Configure the tray's server access key".into());
                }
            }
        }
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot create HTTP client")?;
        Ok(Self { config, http })
    }

    pub fn execute(&self, action: Action) -> Result<String, String> {
        if matches!(action, Action::Toggle) {
            let power = self
                .snapshot()?
                .power
                .ok_or("Power state is unavailable. Refresh before toggling.")?;
            return self.execute(Action::Power(!power));
        }
        if matches!(action, Action::Status | Action::Refresh) {
            return self.status();
        }
        match &self.config.backend {
            Backend::Bridge {
                server_url,
                access_key,
            } => {
                let (route, body) = bridge_request(&action);
                let response = self
                    .http
                    .post(format!("{}{}", server_url.trim_end_matches('/'), route))
                    .bearer_auth(access_key)
                    .json(&body)
                    .send()
                    .map_err(|_| "Server unreachable or request timed out")?;
                if !response.status().is_success() {
                    return Err(format!(
                        "Server returned HTTP {}. Check connection and access key.",
                        response.status().as_u16()
                    ));
                }
                let reply: Value = response.json().map_err(|_| "Invalid server reply")?;
                if reply["status"] != "accepted" {
                    return Err("Server did not accept the command".into());
                }
                Ok(if reply["demo"] == true {
                    "Demo: command accepted; no bulb changed"
                } else {
                    "Command accepted. Use Read status to confirm."
                }
                .into())
            }
            Backend::DirectCloud { component, .. } => {
                let batch:Vec<Value>=commands(&action).into_iter().map(|(cap,command,args)|json!({"component":component,"capability":cap,"command":command,"arguments":args})).collect();
                self.direct_request(Some(json!({"commands":batch})))?;
                Ok("Command accepted".into())
            }
        }
    }

    pub fn status(&self) -> Result<String, String> {
        Ok(self.snapshot()?.describe())
    }

    pub fn snapshot(&self) -> Result<Snapshot, String> {
        match &self.config.backend {
            Backend::Bridge {
                server_url,
                access_key,
            } => {
                let response = self
                    .http
                    .get(format!(
                        "{}/api/bulb?fresh=true",
                        server_url.trim_end_matches('/')
                    ))
                    .bearer_auth(access_key)
                    .send()
                    .map_err(|_| "Server unreachable")?;
                if !response.status().is_success() {
                    return Err(format!(
                        "Server returned HTTP {}",
                        response.status().as_u16()
                    ));
                }
                response.json().map_err(|_| "Invalid server status".into())
            }
            Backend::DirectCloud { component, .. } => {
                let value = self.direct_request(None)?;
                let c = &value["components"][component];
                if c.is_null() {
                    return Err("Configured component not found".into());
                }
                Ok(Snapshot {
                    power: match c["switch"]["switch"]["value"].as_str() {
                        Some("on") => Some(true),
                        Some("off") => Some(false),
                        _ => None,
                    },
                    brightness: c["switchLevel"]["level"]["value"]
                        .as_u64()
                        .filter(|v| *v <= 100)
                        .map(|v| v as u8),
                    hue: c["colorControl"]["hue"]["value"].as_f64(),
                    saturation: c["colorControl"]["saturation"]["value"].as_f64(),
                    temperature: c["colorTemperature"]["colorTemperature"]["value"]
                        .as_u64()
                        .map(|v| v as u32),
                    online: match c["healthCheck"]["DeviceWatch-DeviceStatus"]["value"].as_str() {
                        Some("online") => Some(true),
                        Some("offline") => Some(false),
                        _ => None,
                    },
                    timestamp: c["switch"]["switch"]["timestamp"]
                        .as_str()
                        .map(str::to_string),
                    demo: false,
                })
            }
        }
    }

    fn direct_request(&self, body: Option<Value>) -> Result<Value, String> {
        let Backend::DirectCloud {
            credentials_path,
            device_id,
            ..
        } = &self.config.backend
        else {
            unreachable!()
        };
        let mut auth: Value = serde_json::from_slice(
            &std::fs::read(credentials_path).map_err(|_| "Cannot read sign-in credentials")?,
        )
        .map_err(|_| "Invalid sign-in credentials")?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        if auth["expires_at"].as_u64().unwrap_or(0) <= now + 60 {
            let refresh = auth["refresh_token"]
                .as_str()
                .ok_or("Sign in again using the documented connection setup")?;
            let response = self
                .http
                .post("https://auth-global.api.smartthings.com/oauth/token")
                .form(&[
                    ("grant_type", "refresh_token"),
                    ("client_id", "d18cf96e-c626-4433-bf51-ddbb10c5d1ed"),
                    ("refresh_token", refresh),
                ])
                .send()
                .map_err(|_| "Cannot renew sign-in")?;
            if !response.status().is_success() {
                return Err("Sign-in renewal failed. Reconnect your account.".into());
            }
            let token: Value = response.json().map_err(|_| "Invalid sign-in response")?;
            let access = token["access_token"]
                .as_str()
                .ok_or("Sign-in response has no access token")?;
            auth["access_token"] = json!(access);
            if let Some(refresh) = token["refresh_token"].as_str() {
                auth["refresh_token"] = json!(refresh);
            }
            auth["expires_at"] = json!(now + token["expires_in"].as_u64().unwrap_or(3600));
            std::fs::write(credentials_path, serde_json::to_vec(&auth).unwrap())
                .map_err(|_| "Cannot save renewed sign-in")?;
        }
        let token = auth["access_token"]
            .as_str()
            .ok_or("Missing sign-in token")?;
        let base = format!("https://api.smartthings.com/v1/devices/{device_id}");
        let request = if let Some(body) = body {
            self.http.post(format!("{base}/commands")).json(&body)
        } else {
            self.http.get(format!("{base}/status"))
        };
        let response = request
            .bearer_auth(token)
            .send()
            .map_err(|_| "Light connection timed out")?;
        if !response.status().is_success() {
            return Err(format!(
                "Light service returned HTTP {}",
                response.status().as_u16()
            ));
        }
        response.json().map_err(|_| "Invalid light response".into())
    }
}

pub fn bridge_request(action: &Action) -> (&'static str, Value) {
    match action {
        Action::Power(on) => ("/api/bulb/power", json!({"on":on})),
        Action::Brightness(percent) => ("/api/bulb/brightness", json!({"percent":percent})),
        Action::Colour(r, g, b) => ("/api/bulb/colour", json!({"r":r,"g":g,"b":b})),
        Action::Preset(name) => ("/api/bulb/preset", json!({"name":name})),
        Action::Status | Action::Refresh | Action::Toggle => ("/api/bulb", Value::Null),
    }
}
pub fn commands(action: &Action) -> Vec<(&'static str, &'static str, Vec<Value>)> {
    match action {
        Action::Power(on) => vec![("switch", if *on { "on" } else { "off" }, vec![])],
        Action::Brightness(n) => vec![("switchLevel", "setLevel", vec![json!(n)])],
        Action::Colour(r, g, b) => {
            let (r, g, b) = (*r as f64 / 255.0, *g as f64 / 255.0, *b as f64 / 255.0);
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let delta = max - min;
            if max == 0.0 {
                return vec![("switch", "off", vec![])];
            }
            let h = if delta == 0.0 {
                0.0
            } else if max == r {
                ((g - b) / delta).rem_euclid(6.0)
            } else if max == g {
                (b - r) / delta + 2.0
            } else {
                (r - g) / delta + 4.0
            };
            vec![
                (
                    "colorControl",
                    "setColor",
                    vec![json!({"hue":h/6.0*100.0,"saturation":delta/max*100.0})],
                ),
                (
                    "switchLevel",
                    "setLevel",
                    vec![json!((max * 100.0).round().max(1.0) as u8)],
                ),
                ("switch", "on", vec![]),
            ]
        }
        Action::Preset(name) => match *name {
            "red" => commands(&Action::Colour(255, 0, 0)),
            "green" => commands(&Action::Colour(0, 255, 0)),
            "blue" => commands(&Action::Colour(0, 0, 255)),
            "purple" => commands(&Action::Colour(180, 0, 255)),
            "warm" | "white" => vec![
                (
                    "colorTemperature",
                    "setColorTemperature",
                    vec![json!(if *name == "warm" { 2700 } else { 5000 })],
                ),
                ("switch", "on", vec![]),
            ],
            _ => vec![],
        },
        Action::Status | Action::Refresh | Action::Toggle => vec![],
    }
}

#[cfg(test)]
mod toggle_tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpListener,
    };
    fn controller(listener: &TcpListener) -> Controller {
        Controller::new(Config {
            label: "Test".into(),
            backend: Backend::Bridge {
                server_url: format!("http://{}", listener.local_addr().unwrap()),
                access_key: "a".repeat(32),
            },
        })
        .unwrap()
    }
    fn request(listener: &TcpListener, body: &str) -> String {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut data = Vec::new();
        let mut buf = [0; 1024];
        loop {
            let n = stream.read(&mut buf).unwrap();
            assert!(n > 0);
            data.extend_from_slice(&buf[..n]);
            if let Some(end) = data.windows(4).position(|x| x == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&data[..end]);
                let size = headers
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|s| s.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + size {
                    break;
                }
            }
        }
        write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
        String::from_utf8(data).unwrap()
    }
    #[test]
    fn toggle_reads_fresh_power_then_sends_opposite() {
        for power in [true, false] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let c = controller(&listener);
            let worker = std::thread::spawn(move || {
                let read = request(&listener, &format!("{{\"power\":{power}}}"));
                assert!(read.starts_with("GET /api/bulb?fresh=true "));
                let write = request(&listener, "{\"status\":\"accepted\"}");
                assert!(write.starts_with("POST /api/bulb/power "));
                let body = write.split("\r\n\r\n").nth(1).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(body).unwrap(),
                    json!({"on":!power})
                );
            });
            c.execute(Action::Toggle).unwrap();
            worker.join().unwrap();
        }
    }
    #[test]
    fn toggle_rejects_unknown_power() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let c = controller(&listener);
        let worker = std::thread::spawn(move || {
            request(&listener, "{}");
        });
        assert!(c
            .execute(Action::Toggle)
            .unwrap_err()
            .contains("Power state is unavailable"));
        worker.join().unwrap();
    }
}
