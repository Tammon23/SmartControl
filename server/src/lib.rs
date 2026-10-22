use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use subtle::ConstantTimeEq;
use tokio::sync::Mutex;
mod automation;

pub type Error = (StatusCode, Json<Value>);
fn fail(code: StatusCode, message: &str) -> Error {
    (code, Json(json!({"error":message})))
}
fn upstream() -> Error {
    fail(
        StatusCode::BAD_GATEWAY,
        "SmartThings request failed; check server credentials, device access, and connectivity",
    )
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn random_key() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn default_bind() -> String {
    "127.0.0.1:8787".into()
}
fn default_component() -> String {
    "main".into()
}
fn default_base() -> String {
    "https://api.smartthings.com/v1".into()
}
fn default_auth_url() -> String {
    "https://api.smartthings.com/oauth/authorize".into()
}
fn default_token_url() -> String {
    "https://api.smartthings.com/oauth/token".into()
}
fn default_tokens_file() -> PathBuf {
    "data/tokens.json".into()
}

#[derive(Clone, Deserialize)]
pub struct ClientKey {
    pub name: String,
    pub token: String,
}
#[derive(Clone, Deserialize)]
pub struct OAuthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    #[serde(default = "default_auth_url")]
    pub authorize_url: String,
    #[serde(default = "default_token_url")]
    pub token_url: String,
}
#[derive(Clone, Deserialize)]
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default)]
    pub demo: bool,
    pub clients: Vec<ClientKey>,
    pub admin_token: String,
    pub device_id: String,
    #[serde(default = "default_component")]
    pub component: String,
    #[serde(default = "default_base")]
    pub smartthings_base: String,
    #[serde(default = "default_tokens_file")]
    pub tokens_file: PathBuf,
    pub oauth: Option<OAuthConfig>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Tokens {
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: Option<String>,
    pub expires_at: u64,
}
#[derive(Deserialize)]
struct TokenReply {
    access_token: String,
    refresh_token: Option<String>,
    expires_in: u64,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct BulbState {
    pub power: Option<bool>,
    pub brightness: Option<u8>,
    pub hue: Option<f64>,
    pub saturation: Option<f64>,
    pub temperature: Option<u32>,
    pub colour_mode: Option<String>,
    pub online: Option<bool>,
    pub timestamp: Option<String>,
    pub demo: bool,
}
impl Default for BulbState {
    fn default() -> Self {
        Self {
            power: Some(false),
            brightness: Some(50),
            hue: Some(0.0),
            saturation: Some(0.0),
            temperature: Some(5000),
            colour_mode: Some("white".into()),
            online: Some(true),
            timestamp: None,
            demo: true,
        }
    }
}
struct Cache {
    at: Instant,
    value: BulbState,
}
pub struct App {
    config: Config,
    http: reqwest::Client,
    tokens: Mutex<Tokens>,
    cache: Mutex<Option<Cache>>,
    demo: Mutex<BulbState>,
    commands: Mutex<()>,
    pending: Mutex<HashMap<String, u64>>,
    automation: Mutex<automation::Store>,
}

impl App {
    pub async fn new(config: Config) -> Result<Arc<Self>, String> {
        if config.clients.is_empty()
            || config
                .clients
                .iter()
                .any(|c| c.token.len() < 32 || c.token.starts_with("CHANGE"))
            || config.admin_token.len() < 32
            || config.admin_token.starts_with("CHANGE")
        {
            return Err("Set unique random client and admin keys of at least 32 characters (use bulb-server keygen)".into());
        }
        if config
            .clients
            .iter()
            .any(|c| same(&c.token, &config.admin_token))
            || config
                .clients
                .iter()
                .enumerate()
                .any(|(i, c)| config.clients[..i].iter().any(|p| same(&p.token, &c.token)))
        {
            return Err("Client and admin keys must all be different".into());
        }
        if config.device_id.is_empty()
            || !config
                .device_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit() || b == b'-')
        {
            return Err("Invalid SmartThings device ID".into());
        }
        if !config
            .component
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("Invalid component".into());
        }
        validate_url(&config.smartthings_base)?;
        if let Some(o) = &config.oauth {
            validate_url(&o.authorize_url)?;
            validate_url(&o.token_url)?;
        }
        let tokens = match tokio::fs::read(&config.tokens_file).await {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Invalid tokens file")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Tokens::default(),
            Err(_) => return Err("Cannot read tokens file".into()),
        };
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| "Cannot create HTTP client")?;
        let automation = automation::Store::load(&config.tokens_file).await?;
        Ok(Arc::new(Self {
            config,
            http,
            tokens: Mutex::new(tokens),
            cache: Mutex::new(None),
            demo: Mutex::new(BulbState::default()),
            commands: Mutex::new(()),
            pending: Mutex::new(HashMap::new()),
            automation: Mutex::new(automation),
        }))
    }

    async fn persist(&self, tokens: &Tokens) -> Result<(), Error> {
        let path = &self.config.tokens_file;
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent).await.map_err(|_| {
                    fail(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "Cannot create token directory",
                    )
                })?;
            }
        }
        let bytes = serde_json::to_vec_pretty(tokens).map_err(|_| upstream())?;
        let temp = path.with_extension("tmp");
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let mut file = options
            .open(&temp)
            .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Cannot write token file"))?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| fail(StatusCode::INTERNAL_SERVER_ERROR, "Cannot save tokens"))?;
        drop(file);
        tokio::fs::rename(temp, path).await.map_err(|_| {
            fail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot replace token file",
            )
        })?;
        Ok(())
    }

    async fn token(&self) -> Result<String, Error> {
        let mut tokens = self.tokens.lock().await;
        if !tokens.access_token.is_empty() && tokens.expires_at > now() + 60 {
            return Ok(tokens.access_token.clone());
        }
        let oauth = self.config.oauth.as_ref().ok_or_else(|| {
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "SmartThings authorization is missing or expired",
            )
        })?;
        let refresh = tokens.refresh_token.as_deref().ok_or_else(|| {
            fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "Authorize the server's SmartThings OAuth app first",
            )
        })?;
        let response = self
            .http
            .post(&oauth.token_url)
            .basic_auth(&oauth.client_id, Some(&oauth.client_secret))
            .form(&[
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh),
                ("client_id", oauth.client_id.as_str()),
            ])
            .send()
            .await
            .map_err(|_| upstream())?;
        if !response.status().is_success() {
            return Err(fail(
                StatusCode::SERVICE_UNAVAILABLE,
                "SmartThings token refresh failed; reauthorize the server",
            ));
        }
        let reply: TokenReply = response.json().await.map_err(|_| upstream())?;
        let updated = Tokens {
            access_token: reply.access_token,
            refresh_token: reply.refresh_token.or_else(|| tokens.refresh_token.clone()),
            expires_at: now() + reply.expires_in,
        };
        // Keep a rotated token in memory even if persistence fails, so it is not lost in this process.
        *tokens = updated;
        self.persist(&tokens).await?;
        Ok(tokens.access_token.clone())
    }

    async fn read(&self, fresh: bool) -> Result<BulbState, Error> {
        if self.config.demo {
            return Ok(self.demo.lock().await.clone());
        }
        // Hold the cache lock to coalesce concurrent reads from both clients.
        let mut cache = self.cache.lock().await;
        if !fresh {
            if let Some(c) = &*cache {
                if c.at.elapsed() < Duration::from_secs(3) {
                    return Ok(c.value.clone());
                }
            }
        }
        let token = self.token().await?;
        let response = self
            .http
            .get(format!(
                "{}/devices/{}/status",
                self.config.smartthings_base.trim_end_matches('/'),
                self.config.device_id
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| upstream())?;
        if !response.status().is_success() {
            return Err(upstream());
        }
        let value: Value = response.json().await.map_err(|_| upstream())?;
        let component = value
            .get("components")
            .and_then(|v| v.get(&self.config.component))
            .ok_or_else(upstream)?;
        let state = normalize(component);
        *cache = Some(Cache {
            at: Instant::now(),
            value: state.clone(),
        });
        Ok(state)
    }

    async fn send(&self, commands: Vec<Value>) -> Result<Json<Value>, Error> {
        // Serialize writes without blocking tray or Kindle UI threads.
        let _guard = self.commands.lock().await;
        self.send_locked(commands).await
    }

    async fn send_colour(&self, commands: Vec<Value>) -> Result<Json<Value>, Error> {
        // Keep the colour request and optional wake atomic relative to other writes.
        let _guard = self.commands.lock().await;
        let was_off = self.read(true).await?.power == Some(false);
        let accepted = self.send_locked(commands).await?;
        if was_off {
            if !self.config.demo {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            let _ = self
                .send_locked(vec![self.command("switch", "on", vec![])])
                .await?;
        }
        Ok(accepted)
    }

    // Caller owns the command mutex, including across colour/wake sequencing.
    async fn send_locked(&self, commands: Vec<Value>) -> Result<Json<Value>, Error> {
        if self.config.demo {
            let mut state = self.demo.lock().await;
            for command in &commands {
                apply_demo(&mut state, command);
            }
            return Ok(Json(
                json!({"status":"accepted","confirmed":false,"demo":true}),
            ));
        }
        let token = self.token().await?;
        let response = self
            .http
            .post(format!(
                "{}/devices/{}/commands",
                self.config.smartthings_base.trim_end_matches('/'),
                self.config.device_id
            ))
            .bearer_auth(token)
            .json(&json!({"commands":commands}))
            .send()
            .await
            .map_err(|_| upstream())?;
        // Invalidate even after an uncertain failure: the command might have applied upstream.
        *self.cache.lock().await = None;
        if !response.status().is_success() {
            return Err(upstream());
        }
        let response: Value = response.json().await.map_err(|_| upstream())?;
        let results = response
            .get("results")
            .and_then(Value::as_array)
            .ok_or_else(upstream)?;
        if results.len() != commands.len()
            || results
                .iter()
                .any(|r| r.get("status").and_then(Value::as_str) != Some("ACCEPTED"))
        {
            return Err(upstream());
        }
        Ok(Json(
            json!({"status":"accepted","confirmed":false,"demo":false}),
        ))
    }

    fn command(&self, cap: &str, command: &str, args: Vec<Value>) -> Value {
        json!({"component":self.config.component,"capability":cap,"command":command,"arguments":args})
    }
}
fn validate_url(value: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(value).map_err(|_| "Invalid upstream URL")?;
    if url.scheme() != "https"
        && !(url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
    {
        return Err("Upstream URLs must use HTTPS (HTTP loopback allowed for testing)".into());
    }
    Ok(())
}
fn same(a: &str, b: &str) -> bool {
    let x = Sha256::digest(a.as_bytes());
    let y = Sha256::digest(b.as_bytes());
    bool::from(x.ct_eq(&y))
}
fn bearer(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}
async fn protect(
    State(app): State<Arc<App>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let authorized = bearer(request.headers())
        .is_some_and(|token| app.config.clients.iter().any(|c| same(token, &c.token)));
    if !authorized {
        return fail(
            StatusCode::UNAUTHORIZED,
            "A valid client access key is required",
        )
        .into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}
pub fn router(app: Arc<App>) -> Router {
    let protected = Router::new()
        .route("/api/bulb", get(read_bulb))
        .route("/api/bulb/power", post(power))
        .route("/api/bulb/brightness", post(brightness))
        .route("/api/bulb/colour", post(colour))
        .route("/api/bulb/temperature", post(temperature))
        .route("/api/bulb/preset", post(preset))
        .route(
            "/api/automations",
            get(automation::get_settings).put(automation::put_settings),
        )
        .layer(middleware::from_fn_with_state(app.clone(), protect));
    Router::new()
        .merge(protected)
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/admin/oauth/start", post(oauth_start))
        .route("/oauth/callback", get(oauth_callback))
        .layer(axum::extract::DefaultBodyLimit::max(16384))
        .with_state(app)
}
#[derive(Deserialize)]
struct ReadOptions {
    #[serde(default)]
    fresh: bool,
}
async fn read_bulb(
    State(app): State<Arc<App>>,
    Query(q): Query<ReadOptions>,
) -> Result<Json<BulbState>, Error> {
    Ok(Json(app.read(q.fresh).await?))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Power {
    on: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Brightness {
    percent: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Colour {
    r: u8,
    g: u8,
    b: u8,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Temperature {
    kelvin: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    name: String,
}
async fn power(State(app): State<Arc<App>>, Json(p): Json<Power>) -> Result<Json<Value>, Error> {
    app.send(vec![app.command(
        "switch",
        if p.on { "on" } else { "off" },
        vec![],
    )])
    .await
}
async fn brightness(
    State(app): State<Arc<App>>,
    Json(p): Json<Brightness>,
) -> Result<Json<Value>, Error> {
    if p.percent > 100 {
        return Err(fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Brightness must be 0 through 100",
        ));
    }
    app.send(vec![app.command(
        "switchLevel",
        "setLevel",
        vec![json!(p.percent)],
    )])
    .await
}
fn rgb_commands(app: &App, r: u8, g: u8, b: u8) -> Vec<Value> {
    let (h, s, v) = rgb_hsv(r, g, b);
    if v == 0.0 {
        return vec![app.command("switch", "off", vec![])];
    }
    vec![app.command(
        "colorControl",
        "setColor",
        vec![json!({"hue":(h * 10000.0).round() / 10000.0,
                    "saturation":(s * 10000.0).round() / 10000.0})],
    )]
}
pub fn rgb_hsv(r: u8, g: u8, b: u8) -> (f64, f64, f64) {
    let (r, g, b) = (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (
        h / 6.0 * 100.0,
        if max == 0.0 { 0.0 } else { d / max * 100.0 },
        max,
    )
}
async fn colour(State(app): State<Arc<App>>, Json(p): Json<Colour>) -> Result<Json<Value>, Error> {
    let commands = rgb_commands(&app, p.r, p.g, p.b);
    if p.r == 0 && p.g == 0 && p.b == 0 {
        app.send(commands).await
    } else {
        app.send_colour(commands).await
    }
}
async fn temperature(
    State(app): State<Arc<App>>,
    Json(p): Json<Temperature>,
) -> Result<Json<Value>, Error> {
    // Deliberately expose only the two presets used by this client, not an unverified full bulb range.
    if !matches!(p.kelvin, 2700 | 5000) {
        return Err(fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Supported presets: 2700 or 5000 kelvin",
        ));
    }
    app.send_colour(vec![app.command(
        "colorTemperature",
        "setColorTemperature",
        vec![json!(p.kelvin)],
    )])
    .await
}
async fn preset(State(app): State<Arc<App>>, Json(p): Json<Preset>) -> Result<Json<Value>, Error> {
    app.send_colour(preset_commands(&app, &p.name)?).await
}
fn preset_commands(app: &App, name: &str) -> Result<Vec<Value>, Error> {
    Ok(match name {
        "red" => rgb_commands(app, 255, 0, 0),
        "green" => rgb_commands(app, 0, 255, 0),
        "blue" => rgb_commands(app, 0, 0, 255),
        "purple" => rgb_commands(app, 180, 0, 255),
        "warm" | "white" => vec![app.command(
            "colorTemperature",
            "setColorTemperature",
            vec![json!(if name == "warm" { 2700 } else { 5000 })],
        )],
        _ => return Err(fail(StatusCode::UNPROCESSABLE_ENTITY, "Unknown preset")),
    })
}
fn normalize(value: &Value) -> BulbState {
    let attr = |cap: &str, name: &str| value.get(cap).and_then(|v| v.get(name));
    let val = |cap: &str, name: &str| attr(cap, name).and_then(|v| v.get("value"));
    BulbState {
        power: val("switch", "switch")
            .and_then(Value::as_str)
            .and_then(|s| match s {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            }),
        brightness: val("switchLevel", "level")
            .and_then(Value::as_u64)
            .filter(|v| *v <= 100)
            .map(|v| v as u8),
        hue: val("colorControl", "hue").and_then(Value::as_f64),
        saturation: val("colorControl", "saturation").and_then(Value::as_f64),
        temperature: val("colorTemperature", "colorTemperature")
            .and_then(Value::as_u64)
            .map(|v| v as u32),
        colour_mode: {
            let colour = [
                attr("colorControl", "hue").and_then(|v| v["timestamp"].as_str()),
                attr("colorControl", "saturation").and_then(|v| v["timestamp"].as_str()),
            ]
            .into_iter()
            .flatten()
            .max();
            let white =
                attr("colorTemperature", "colorTemperature").and_then(|v| v["timestamp"].as_str());
            match (colour, white) {
                (Some(a), Some(b)) => Some(if b > a { "white" } else { "colour" }.into()),
                _ => None,
            }
        },
        online: val("healthCheck", "DeviceWatch-DeviceStatus")
            .and_then(Value::as_str)
            .and_then(|s| match s {
                "online" => Some(true),
                "offline" => Some(false),
                _ => None,
            }),
        timestamp: attr("switch", "switch")
            .and_then(|v| v.get("timestamp"))
            .and_then(Value::as_str)
            .map(str::to_string),
        demo: false,
    }
}
fn apply_demo(state: &mut BulbState, c: &Value) {
    let first = &c["arguments"][0];
    match c["command"].as_str().unwrap_or("") {
        "on" => state.power = Some(true),
        "off" => state.power = Some(false),
        "setLevel" => state.brightness = first.as_u64().map(|v| v as u8),
        "setColor" => {
            state.colour_mode = Some("colour".into());
            state.hue = first["hue"].as_f64();
            state.saturation = first["saturation"].as_f64();
        }
        "setColorTemperature" => {
            state.colour_mode = Some("white".into());
            state.temperature = first.as_u64().map(|v| v as u32);
            state.saturation = Some(0.0);
        }
        _ => {}
    }
}
async fn oauth_start(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
) -> Result<Json<Value>, Error> {
    if !bearer(&headers).is_some_and(|t| same(t, &app.config.admin_token)) {
        return Err(fail(StatusCode::UNAUTHORIZED, "Admin access key required"));
    }
    let oauth = app.config.oauth.as_ref().ok_or_else(|| {
        fail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Configure your own SmartThings OAuth client first",
        )
    })?;
    let state = random_key();
    let mut pending = app.pending.lock().await;
    pending.retain(|_, expiry| *expiry > now());
    if pending.len() >= 5 {
        return Err(fail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many pending sign-ins",
        ));
    }
    pending.insert(state.clone(), now() + 600);
    let mut url = reqwest::Url::parse(&oauth.authorize_url).map_err(|_| upstream())?;
    url.query_pairs_mut().extend_pairs([
        ("client_id", oauth.client_id.as_str()),
        ("response_type", "code"),
        ("redirect_uri", oauth.redirect_uri.as_str()),
        ("scope", "r:devices:* x:devices:*"),
        ("state", state.as_str()),
    ]);
    Ok(Json(json!({"authorize_url":url.as_str(),"expires_in":600})))
}
#[derive(Deserialize)]
struct Callback {
    state: Option<String>,
    code: Option<String>,
    error: Option<String>,
}
async fn oauth_callback(
    State(app): State<Arc<App>>,
    Query(q): Query<Callback>,
) -> Result<&'static str, Error> {
    let oauth = app
        .config
        .oauth
        .as_ref()
        .ok_or_else(|| fail(StatusCode::SERVICE_UNAVAILABLE, "OAuth not configured"))?;
    let state = q
        .state
        .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "Missing OAuth state"))?;
    {
        let mut pending = app.pending.lock().await;
        if pending.get(&state).is_none_or(|expiry| *expiry <= now()) {
            return Err(fail(
                StatusCode::BAD_REQUEST,
                "Invalid or expired OAuth state",
            ));
        }
        pending.remove(&state);
    }
    if q.error.is_some() {
        return Err(fail(
            StatusCode::BAD_REQUEST,
            "SmartThings authorization was declined",
        ));
    }
    let code = q
        .code
        .ok_or_else(|| fail(StatusCode::BAD_REQUEST, "Missing authorization code"))?;
    // Serialize exchange with token refresh and persistence.
    let mut tokens = app.tokens.lock().await;
    let response = app
        .http
        .post(&oauth.token_url)
        .basic_auth(&oauth.client_id, Some(&oauth.client_secret))
        .form(&[
            ("grant_type", "authorization_code"),
            ("client_id", oauth.client_id.as_str()),
            ("redirect_uri", oauth.redirect_uri.as_str()),
            ("code", code.as_str()),
        ])
        .send()
        .await
        .map_err(|_| upstream())?;
    if !response.status().is_success() {
        return Err(upstream());
    }
    let reply: TokenReply = response.json().await.map_err(|_| upstream())?;
    *tokens = Tokens {
        access_token: reply.access_token,
        refresh_token: reply.refresh_token,
        expires_at: now() + reply.expires_in,
    };
    app.persist(&tokens).await?;
    drop(tokens);
    *app.cache.lock().await = None;
    Ok("Bulb Bridge is authorized. You can close this window.")
}

#[cfg(test)]
mod tests;
