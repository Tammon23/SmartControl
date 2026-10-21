use crate::backend::{Action, Snapshot};
use std::collections::VecDeque;

#[derive(Default)]
pub struct Pending(VecDeque<Action>);
fn group(action: &Action) -> u8 {
    match action {
        Action::Colour(..) | Action::Preset(_) => 1,
        Action::Brightness(_) => 2,
        Action::Power(_) | Action::Toggle => 3,
        _ => 4,
    }
}
impl Pending {
    pub const fn new() -> Self {
        Self(VecDeque::new())
    }

    pub fn push(&mut self, action: Action) {
        let kind = group(&action);
        self.0.retain(|old| group(old) != kind);
        self.0.push_back(action);
    }
    pub fn pop(&mut self) -> Option<Action> {
        self.0.pop_front()
    }
}
pub fn is_colour(action: &Action) -> bool {
    match action {
        Action::Colour(r, g, b) => (*r, *g, *b) != (0, 0, 0),
        Action::Preset(_) => true,
        _ => false,
    }
}
pub fn matches(action: &Action, state: &Snapshot) -> bool {
    match action {
        Action::Power(on) => state.power == Some(*on),
        Action::Brightness(n) => state
            .brightness
            .is_some_and(|actual| actual.abs_diff(*n) <= 1),
        Action::Colour(r, g, b) => {
            if (*r, *g, *b) == (0, 0, 0) {
                return state.power == Some(false);
            }
            let max = (*r).max(*g).max(*b) as f64;
            let min = (*r).min(*g).min(*b) as f64;
            let delta = max - min;
            let hue = if delta == 0.0 {
                0.0
            } else if max == *r as f64 {
                ((*g as f64 - *b as f64) / delta).rem_euclid(6.0)
            } else if max == *g as f64 {
                (*b as f64 - *r as f64) / delta + 2.0
            } else {
                (*r as f64 - *g as f64) / delta + 4.0
            } / 6.0
                * 100.0;
            state.power == Some(true)
                && state.colour_mode.as_deref() != Some("white")
                && state
                    .saturation
                    .is_some_and(|s| (s - delta / max * 100.0).abs() < 2.0)
                && (delta == 0.0
                    || state.hue.is_some_and(|h| {
                        let diff = (h - hue).abs();
                        diff.min(100.0 - diff) < 2.0
                    }))
        }
        Action::Preset(name) => match *name {
            "warm" | "white" => {
                state.power == Some(true)
                    && state.colour_mode.as_deref() != Some("colour")
                    && state.temperature.is_some_and(|k| {
                        k.abs_diff(if *name == "warm" { 2700 } else { 5000 }) < 100
                    })
            }
            "red" => matches(&Action::Colour(255, 0, 0), state),
            "green" => matches(&Action::Colour(0, 255, 0), state),
            "blue" => matches(&Action::Colour(0, 0, 255), state),
            "purple" => matches(&Action::Colour(180, 0, 255), state),
            _ => false,
        },
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rapid_colour_clicks_keep_latest_and_preserve_other_controls() {
        let mut q = Pending::default();
        q.push(Action::Preset("red"));
        q.push(Action::Brightness(50));
        q.push(Action::Preset("blue"));
        assert!(matches!(q.pop(), Some(Action::Brightness(50))));
        assert!(matches!(q.pop(), Some(Action::Preset("blue"))));
        assert!(q.pop().is_none());
    }
    #[test]
    fn stale_status_does_not_confirm_a_colour_change() {
        let green = Snapshot {
            power: Some(true),
            hue: Some(33.33),
            saturation: Some(100.0),
            ..Snapshot::default()
        };
        assert!(!matches(&Action::Preset("blue"), &green));
        assert!(matches(&Action::Preset("green"), &green));
        let white = Snapshot {
            colour_mode: Some("white".into()),
            temperature: Some(5000),
            ..green
        };
        assert!(!matches(&Action::Preset("green"), &white));
        assert!(matches(&Action::Preset("white"), &white));
    }
}
