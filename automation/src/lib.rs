//! Pure scheduling calculations. Execution and durable claims belong to the caller.
use chrono::{DateTime, Days, Local, LocalResult, NaiveDate, TimeZone};
#[cfg(feature = "iana")]
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use sunrise::{Coordinates, SolarDay, SolarEvent};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub timezone: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub rules: Vec<Rule>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            timezone: "local".into(),
            latitude: None,
            longitude: None,
            rules: vec![],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub enabled: bool,
    pub trigger: Trigger,
    pub action: LightAction,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    Time { hour: u8, minute: u8 },
    Sunset { offset_minutes: i32 },
}
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LightAction {
    pub power: Option<bool>,
    pub brightness: Option<u8>,
    pub preset: Option<String>,
    pub colour: Option<[u8; 3]>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub last_run: HashMap<String, String>,
}
#[derive(Clone, Debug)]
pub struct Occurrence {
    pub rule: Rule,
    pub key: String,
    pub scheduled_at: i64,
}

pub fn validate(settings: &Settings) -> Result<(), String> {
    if !valid_timezone(&settings.timezone) {
        return Err("Choose local or a valid IANA time zone".into());
    }
    match (settings.latitude, settings.longitude) {
        (None, None) => {}
        (Some(a), Some(b)) if Coordinates::new(a, b).is_some() => {}
        _ => return Err("Provide both latitude (-90..90) and longitude (-180..180)".into()),
    }
    if settings.rules.len() > 20 {
        return Err("At most 20 rules are supported".into());
    }
    let mut ids = HashSet::new();
    for r in &settings.rules {
        if r.id.is_empty()
            || r.id.len() > 64
            || !r
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
            || !ids.insert(&r.id)
        {
            return Err(
                "Rule IDs must be unique and contain only letters, numbers, hyphens or underscores"
                    .into(),
            );
        }
        if r.name.trim().is_empty()
            || r.name.chars().count() > 80
            || r.name.chars().any(char::is_control)
        {
            return Err("Rule names must contain 1–80 printable characters".into());
        }
        match r.trigger {
            Trigger::Time { hour, minute } if hour > 23 || minute > 59 => {
                return Err("Use a 24-hour time".into())
            }
            Trigger::Sunset { offset_minutes } => {
                if !(-180..=180).contains(&offset_minutes) {
                    return Err("Sunset offset must be between -180 and 180 minutes".into());
                }
                if settings.latitude.is_none() {
                    return Err("Sunset requires latitude and longitude".into());
                }
            }
            _ => {}
        }
        if r.action.power.is_none()
            && r.action.brightness.is_none()
            && r.action.preset.is_none()
            && r.action.colour.is_none()
        {
            return Err("Choose at least one action".into());
        }
        if r.action.power == Some(false)
            && (r.action.brightness.is_some()
                || r.action.preset.is_some()
                || r.action.colour.is_some())
        {
            return Err("An off action cannot also change brightness or colour".into());
        }
        if (r.action.preset.is_some() || r.action.colour.is_some()) && r.action.power != Some(true)
        {
            return Err("A colour preset requires an explicit on action".into());
        }
        if r.action.colour.is_some() && r.action.preset.is_some() {
            return Err("Choose a preset or a custom colour, not both".into());
        }
        if r.action.colour == Some([0, 0, 0]) {
            return Err("Use an off action instead of black".into());
        }
        if r.action.brightness.is_some_and(|v| v > 100) {
            return Err("Brightness must be 0–100".into());
        }
        if r.action
            .preset
            .as_deref()
            .is_some_and(|p| !matches!(p, "red" | "green" | "blue" | "purple" | "warm" | "white"))
        {
            return Err("Unknown colour preset".into());
        }
    }
    Ok(())
}
fn valid_timezone(zone: &str) -> bool {
    if zone == "local" {
        return true;
    }
    #[cfg(feature = "iana")]
    {
        zone.parse::<Tz>().is_ok()
    }
    #[cfg(not(feature = "iana"))]
    {
        false
    }
}
#[cfg(feature = "iana")]
fn named_date(zone: &str, utc: DateTime<chrono::Utc>) -> Result<NaiveDate, String> {
    Ok(utc
        .with_timezone(&zone.parse::<Tz>().map_err(|_| "Invalid timezone")?)
        .date_naive())
}
#[cfg(not(feature = "iana"))]
fn named_date(_: &str, _: DateTime<chrono::Utc>) -> Result<NaiveDate, String> {
    Err("This build uses the Windows local timezone".into())
}
#[cfg(feature = "iana")]
fn named_time(zone: &str, time: chrono::NaiveDateTime) -> Option<i64> {
    earliest(zone.parse::<Tz>().ok()?.from_local_datetime(&time))
}
#[cfg(not(feature = "iana"))]
fn named_time(_: &str, _: chrono::NaiveDateTime) -> Option<i64> {
    None
}
fn date(settings: &Settings, now: i64) -> Result<NaiveDate, String> {
    let utc = DateTime::from_timestamp(now, 0).ok_or("Invalid timestamp")?;
    Ok(if settings.timezone == "local" {
        utc.with_timezone(&Local).date_naive()
    } else {
        named_date(&settings.timezone, utc)?
    })
}
fn earliest<T: TimeZone>(value: LocalResult<DateTime<T>>) -> Option<i64> {
    match value {
        LocalResult::Single(t) => Some(t.timestamp()),
        LocalResult::Ambiguous(a, b) => Some(a.timestamp().min(b.timestamp())),
        LocalResult::None => None, // A clock time skipped by spring-forward does not run.
    }
}
fn scheduled(settings: &Settings, rule: &Rule, day: NaiveDate) -> Option<i64> {
    match rule.trigger {
        Trigger::Time { hour, minute } => {
            let t = day.and_hms_opt(hour.into(), minute.into(), 0)?;
            if settings.timezone == "local" {
                earliest(Local.from_local_datetime(&t))
            } else {
                named_time(&settings.timezone, t)
            }
        }
        Trigger::Sunset { offset_minutes } => {
            let coords = Coordinates::new(settings.latitude?, settings.longitude?)?;
            SolarDay::new(coords, day)
                .event_time(SolarEvent::Sunset)
                .map(|t| t.timestamp() + i64::from(offset_minutes) * 60)
        }
    }
}
pub fn due(settings: &Settings, ledger: &Ledger, now_unix: i64) -> Result<Vec<Occurrence>, String> {
    validate(settings)?;
    let today = date(settings, now_unix)?;
    let mut found = vec![];
    for rule in settings.rules.iter().filter(|r| r.enabled) {
        for day in [today.pred_opt(), Some(today), today.succ_opt()]
            .into_iter()
            .flatten()
        {
            let key = day.to_string();
            if ledger
                .last_run
                .get(&rule.id)
                .is_some_and(|last| last >= &key)
            {
                continue;
            }
            if let Some(at) = scheduled(settings, rule, day) {
                if at <= now_unix && now_unix - at <= 600 {
                    found.push(Occurrence {
                        rule: rule.clone(),
                        key,
                        scheduled_at: at,
                    });
                }
            }
        }
    }
    found.sort_by(|a, b| (a.scheduled_at, &a.rule.id).cmp(&(b.scheduled_at, &b.rule.id)));
    Ok(found)
}
pub fn mark(ledger: &mut Ledger, occurrence: &Occurrence) {
    let old = ledger
        .last_run
        .entry(occurrence.rule.id.clone())
        .or_default();
    if *old < occurrence.key {
        *old = occurrence.key.clone();
    }
}
/// Baseline newly saved/enabled rules so past events are not replayed on save.
pub fn baseline(ledger: &mut Ledger, settings: &Settings, now_unix: i64) -> Result<(), String> {
    validate(settings)?;
    let today = date(settings, now_unix)?;
    for rule in &settings.rules {
        for day in [today.pred_opt(), Some(today), today.succ_opt()]
            .into_iter()
            .flatten()
        {
            if scheduled(settings, rule, day).is_some_and(|t| t <= now_unix) {
                mark(
                    ledger,
                    &Occurrence {
                        rule: rule.clone(),
                        key: day.to_string(),
                        scheduled_at: now_unix,
                    },
                );
            }
        }
    }
    Ok(())
}
pub fn preview(settings: &Settings, rule: &Rule, now_unix: i64) -> Result<Option<i64>, String> {
    validate(settings)?;
    let today = date(settings, now_unix)?;
    // Include yesterday for a sunset shifted past midnight. Polar days may have no event.
    Ok((0..9)
        .filter_map(|i| today.pred_opt()?.checked_add_days(Days::new(i)))
        .filter_map(|d| scheduled(settings, rule, d))
        .filter(|t| *t > now_unix)
        .min())
}

#[cfg(all(test, feature = "iana"))]
mod tests {
    use super::*;
    fn at(t: &str) -> i64 {
        DateTime::parse_from_rfc3339(t).unwrap().timestamp()
    }
    fn settings(hour: u8, minute: u8) -> Settings {
        Settings {
            timezone: "America/Toronto".into(),
            latitude: Some(43.6532),
            longitude: Some(-79.3832),
            rules: vec![Rule {
                id: "test".into(),
                name: "Test".into(),
                enabled: true,
                trigger: Trigger::Time { hour, minute },
                action: LightAction {
                    power: Some(true),
                    ..Default::default()
                },
            }],
        }
    }
    #[test]
    fn custom_colours_round_trip_and_reject_conflicting_actions() {
        let mut s = settings(18, 0);
        s.rules[0].action.colour = Some([17, 34, 255]);
        let saved = serde_json::to_string(&s).unwrap();
        let loaded: Settings = serde_json::from_str(&saved).unwrap();
        assert_eq!(loaded, s);
        assert_eq!(validate(&loaded), Ok(()));
        s.rules[0].action.preset = Some("blue".into());
        assert!(validate(&s).is_err());
        s.rules[0].action.preset = None;
        s.rules[0].action.colour = Some([0, 0, 0]);
        assert!(validate(&s).is_err());
        let old: LightAction = serde_json::from_str(r#"{"power":true,"preset":"red"}"#).unwrap();
        assert_eq!(old.colour, None);
    }
    #[test]
    fn daily_claims_and_ten_minute_catchup() {
        let s = settings(18, 30);
        let mut l = Ledger::default();
        let now = at("2026-10-20T22:35:00Z");
        let d = due(&s, &l, now).unwrap();
        assert_eq!(d.len(), 1);
        mark(&mut l, &d[0]);
        assert!(due(&s, &l, now).unwrap().is_empty());
        assert!(due(&s, &Ledger::default(), now + 601).unwrap().is_empty());
        assert_eq!(due(&s, &l, now + 86400).unwrap().len(), 1);
    }
    #[test]
    fn saving_and_enabling_does_not_replay_past_event() {
        let s = settings(18, 30);
        let mut l = Ledger::default();
        let now = at("2026-10-20T22:35:00Z");
        baseline(&mut l, &s, now).unwrap();
        assert!(due(&s, &l, now).unwrap().is_empty());
    }
    #[test]
    fn dst_skip_and_repeat_are_safe() {
        let s = settings(2, 30);
        assert!(due(&s, &Ledger::default(), at("2026-03-08T07:35:00Z"))
            .unwrap()
            .is_empty());
        let s = settings(1, 30);
        let mut l = Ledger::default();
        let d = due(&s, &l, at("2026-11-01T05:35:00Z")).unwrap();
        assert_eq!(d.len(), 1);
        mark(&mut l, &d[0]);
        assert!(due(&s, &l, at("2026-11-01T06:35:00Z")).unwrap().is_empty());
    }
    #[test]
    fn toronto_sunset_and_polar_no_event() {
        let mut s = settings(0, 0);
        s.rules[0].trigger = Trigger::Sunset { offset_minutes: 0 };
        let t = preview(&s, &s.rules[0], at("2026-10-20T04:00:00Z"))
            .unwrap()
            .unwrap();
        assert!(t > at("2026-10-20T22:00:00Z") && t < at("2026-10-20T23:00:00Z"));
        s.latitude = Some(89.0);
        s.longitude = Some(0.0);
        assert_eq!(
            preview(&s, &s.rules[0], at("2026-06-20T00:00:00Z")).unwrap(),
            None
        );
    }
    #[test]
    fn invalid_offsets_and_conflicting_off_are_rejected() {
        let mut s = settings(0, 0);
        s.rules[0].trigger = Trigger::Sunset {
            offset_minutes: i32::MIN,
        };
        assert!(validate(&s).is_err());
        s.rules[0].trigger = Trigger::Time { hour: 0, minute: 0 };
        s.rules[0].action = LightAction {
            power: None,
            brightness: None,
            preset: Some("blue".into()),
            colour: None,
        };
        assert!(validate(&s).is_err());
        s.rules[0].action.power = Some(true);
        assert_eq!(validate(&s), Ok(()));
        s.rules[0].trigger = Trigger::Time { hour: 0, minute: 0 };
        s.rules[0].action = LightAction {
            power: Some(false),
            brightness: Some(50),
            preset: None,
            colour: None,
        };
        assert!(validate(&s).is_err());
    }
}

#[cfg(test)]
mod local_tests {
    use super::*;
    #[test]
    fn local_rules_use_the_operating_system_timezone() {
        let at = earliest(Local.with_ymd_and_hms(2030, 1, 15, 11, 0, 0)).unwrap();
        let expected = earliest(Local.with_ymd_and_hms(2030, 1, 15, 12, 34, 0)).unwrap();
        let mut settings = Settings::default();
        let rule = Rule {
            id: "local".into(),
            name: "Local".into(),
            enabled: true,
            trigger: Trigger::Time {
                hour: 12,
                minute: 34,
            },
            action: LightAction {
                power: Some(true),
                ..Default::default()
            },
        };
        settings.rules.push(rule.clone());
        assert_eq!(validate(&settings), Ok(()));
        assert_eq!(preview(&settings, &rule, at).unwrap(), Some(expected));
        #[cfg(not(feature = "iana"))]
        {
            settings.timezone = "America/Toronto".into();
            assert!(validate(&settings).is_err());
        }
    }
}
