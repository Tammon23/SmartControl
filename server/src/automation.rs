use super::*;
use bulb_automation::{baseline, due, mark, preview, validate, Ledger, LightAction, Settings};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct RunResult {
    rule_id: String,
    scheduled_at: i64,
    attempted_at: i64,
    status: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Store {
    settings: Settings,
    #[serde(default)]
    ledger: Ledger,
    #[serde(default)]
    results: Vec<RunResult>,
    #[serde(skip)]
    pub(super) path: PathBuf,
}
impl Store {
    pub async fn load(tokens_file: &std::path::Path) -> Result<Self, String> {
        let path = tokens_file.with_file_name("automations.json");
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let document: Value =
                    serde_json::from_slice(&bytes).map_err(|_| "Invalid automation settings")?;
                let needs_baseline = document.get("ledger").is_none();
                let mut state: Self =
                    serde_json::from_slice(&bytes).map_err(|_| "Invalid automation settings")?;
                validate(&state.settings)?;
                if state.settings.timezone != "America/Toronto" {
                    return Err("Server automations require America/Toronto time".into());
                }
                state.path = path;
                if needs_baseline {
                    baseline(&mut state.ledger, &state.settings, now() as i64)?;
                    state.save()?;
                }
                Ok(state)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self {
                settings: Settings {
                    timezone: "America/Toronto".into(),
                    latitude: Some(43.6532),
                    longitude: Some(-79.3832),
                    rules: vec![],
                },
                ledger: Ledger::default(),
                results: vec![],
                path,
            }),
            Err(_) => Err("Cannot read automation settings".into()),
        }
    }
    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)
                .map_err(|_| "Cannot create automation data directory")?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        use std::io::Write;
        let temp = self.path.with_extension("tmp");
        let mut file = options
            .open(&temp)
            .map_err(|_| "Cannot write automation settings")?;
        file.write_all(
            &serde_json::to_vec_pretty(self).map_err(|_| "Cannot encode automation settings")?,
        )
        .and_then(|_| file.sync_all())
        .map_err(|_| "Cannot save automation settings")?;
        drop(file);
        std::fs::rename(temp, &self.path).map_err(|_| "Cannot replace automation settings")?;
        #[cfg(unix)]
        if let Some(parent) = self.path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::File::open(parent)
                .and_then(|f| f.sync_all())
                .map_err(|_| "Cannot sync automation directory")?;
        }
        Ok(())
    }
    fn reply(&self) -> Value {
        let next:Vec<_>=self.settings.rules.iter().map(|r|json!({"id":r.id,"next_at":preview(&self.settings,r,now() as i64).ok().flatten(),"enabled":r.enabled})).collect();
        json!({"settings":self.settings,"next":next,"results":self.results})
    }
}
pub(super) async fn get_settings(State(app): State<Arc<App>>) -> Json<Value> {
    Json(app.automation.lock().await.reply())
}
pub(super) async fn put_settings(
    State(app): State<Arc<App>>,
    Json(settings): Json<Settings>,
) -> Result<Json<Value>, Error> {
    validate(&settings).map_err(|e| fail(StatusCode::UNPROCESSABLE_ENTITY, &e))?;
    // This server is the Toronto rule set; clients cannot silently switch it to VM UTC.
    if settings.timezone != "America/Toronto"
        || settings.latitude != Some(43.6532)
        || settings.longitude != Some(-79.3832)
    {
        return Err(fail(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Server schedules use Toronto time and coordinates",
        ));
    }
    let mut state = app.automation.lock().await;
    let mut updated = state.clone();
    updated.settings = settings;
    baseline(&mut updated.ledger, &updated.settings, now() as i64)
        .map_err(|e| fail(StatusCode::UNPROCESSABLE_ENTITY, &e))?;
    updated
        .ledger
        .last_run
        .retain(|id, _| updated.settings.rules.iter().any(|r| &r.id == id));
    updated
        .save()
        .map_err(|e| fail(StatusCode::INTERNAL_SERVER_ERROR, &e))?;
    *state = updated;
    Ok(Json(state.reply()))
}
impl App {
    /// Starts only the server's own rule set; empty by default. Tests call tick directly.
    pub fn start_automations(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let app = self.clone();
        tokio::spawn(async move {
            let mut timer = tokio::time::interval(Duration::from_secs(15));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                timer.tick().await;
                app.automation_tick(now() as i64).await;
            }
        })
    }
    pub(super) async fn automation_tick(&self, at: i64) {
        let mut state = self.automation.lock().await;
        let Ok(events) = due(&state.settings, &state.ledger, at) else {
            return;
        };
        for occurrence in events {
            let mut claimed = state.clone();
            mark(&mut claimed.ledger, &occurrence);
            claimed.results.push(RunResult {
                rule_id: occurrence.rule.id.clone(),
                scheduled_at: occurrence.scheduled_at,
                attempted_at: at,
                status: "claimed".into(),
            });
            if claimed.results.len() > 40 {
                claimed.results.remove(0);
            }
            // Persist before sending. A crash or failure is deliberately not retried.
            if claimed.save().is_err() {
                return;
            }
            *state = claimed;
            let success = self
                .execute_automation(&occurrence.rule.action)
                .await
                .is_ok();
            state.results.last_mut().unwrap().status =
                if success { "accepted" } else { "failed" }.into();
            let _ = state.save(); // Durable claim remains even if result metadata cannot be saved.
        }
    }
    async fn execute_automation(&self, action: &LightAction) -> Result<(), Error> {
        let _guard = self.commands.lock().await;
        let wake = action.power == Some(true);
        if wake {
            let _ = self
                .send_locked(vec![self.command("switch", "on", vec![])])
                .await?;
            if !self.config.demo {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
        if let Some(preset) = &action.preset {
            let _ = self.send_locked(preset_commands(self, preset)?).await?;
        }
        if let Some([r, g, b]) = action.colour {
            let _ = self.send_locked(crate::rgb_commands(self, r, g, b)).await?;
        }
        if let Some(level) = action.brightness {
            let _ = self
                .send_locked(vec![self.command(
                    "switchLevel",
                    "setLevel",
                    vec![json!(level)],
                )])
                .await?;
        }
        // Explicit off wins over colour/brightness integrations that implicitly wake.
        if action.power == Some(false) {
            let _ = self
                .send_locked(vec![self.command("switch", "off", vec![])])
                .await?;
        }
        Ok(())
    }
}
