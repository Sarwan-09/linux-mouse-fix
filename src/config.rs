//! Settings: built-in defaults, the TOML file merged over them, and validation.

use std::collections::BTreeMap;

use evdev::KeyCode;
use serde::{Deserialize, Serialize};

pub const DEFAULTS_TOML: &str = r#"
enabled = true

[scroll]
smooth = "high"
speed = "medium"
acceleration = true
reverse = false

[drag]
scale = 1.5
invert = false

[buttons.middle]
click = "middle-click"
hold = "show-desktop"
drag = "spaces"
scroll = "app-switcher"

[buttons.front]
click = "middle-click"
double-click = "overview"
drag = "spaces"
scroll = "zoom"

[buttons.back]
click = "back"
drag = "navigate"
scroll = "desktop-and-apps"
"#;

/// Button names in the settings file and the evdev buttons they stand for.
pub const BUTTONS: &[(&str, KeyCode)] = &[
    ("middle", KeyCode::BTN_MIDDLE),
    ("back", KeyCode::BTN_SIDE),
    ("front", KeyCode::BTN_EXTRA),
    ("button6", KeyCode::BTN_FORWARD),
    ("button7", KeyCode::BTN_BACK),
    ("button8", KeyCode::BTN_TASK),
];
/// Actions that are mouse clicks rather than key presses.
pub const CLICKS: &[(&str, KeyCode)] = &[
    ("middle-click", KeyCode::BTN_MIDDLE),
    ("primary-click", KeyCode::BTN_LEFT),
    ("secondary-click", KeyCode::BTN_RIGHT),
    ("back", KeyCode::BTN_SIDE),
    ("forward", KeyCode::BTN_EXTRA),
];
/// Named actions and the GNOME shortcuts that perform them.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("overview", "super"),
    ("app-grid", "super+a"),
    ("show-desktop", "super+d"),
    ("workspace-left", "super+pageup"),
    ("workspace-right", "super+pagedown"),
];
pub const DRAG_EFFECTS: &[&str] = &["none", "spaces", "navigate"];
pub const SCROLL_EFFECTS: &[&str] = &[
    "none", "zoom", "app-switcher", "spaces", "desktop-and-apps", "precise", "swift",
];
pub const SMOOTHNESS: &[&str] = &["off", "regular", "high"];
pub const SPEEDS: &[&str] = &["low", "medium", "high"];
const KEY_ALIASES: &[(&str, &str)] = &[
    ("super", "LEFTMETA"),
    ("ctrl", "LEFTCTRL"),
    ("control", "LEFTCTRL"),
    ("alt", "LEFTALT"),
    ("shift", "LEFTSHIFT"),
    ("esc", "ESC"),
    ("del", "DELETE"),
];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    pub enabled: bool,
    pub scroll: ScrollConfig,
    pub drag: DragConfig,
    /// button name -> trigger ("click", "drag", ...) -> action or effect name
    pub buttons: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScrollConfig {
    pub smooth: String,
    pub speed: String,
    pub acceleration: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DragConfig {
    pub scale: f64,
    pub invert: bool,
}

pub fn lookup<T: Copy>(table: &[(&str, T)], name: &str) -> Option<T> {
    table.iter().find(|(n, _)| *n == name).map(|&(_, v)| v)
}

/// "ctrl+shift+t" -> [KEY_LEFTCTRL, KEY_LEFTSHIFT, KEY_T]
pub fn parse_keys(spec: &str) -> Result<Vec<KeyCode>, String> {
    spec.to_lowercase()
        .split('+')
        .map(|part| {
            let part = part.trim();
            let name = lookup(KEY_ALIASES, part)
                .map(str::to_string)
                .unwrap_or_else(|| part.to_uppercase());
            format!("KEY_{name}")
                .parse::<KeyCode>()
                .map_err(|_| format!("unknown key {name:?} in {spec:?}"))
        })
        .collect()
}

/// Every key name parse_keys accepts (without aliases), lowercase, with its code.
pub fn key_names() -> BTreeMap<String, u16> {
    (1..0x300u16)
        .filter_map(|code| {
            let name = format!("{:?}", KeyCode(code));
            name.strip_prefix("KEY_").map(|n| (n.to_lowercase(), code))
        })
        .collect()
}

fn merge(base: &mut toml::Table, over: toml::Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(b)), toml::Value::Table(o)) => merge(b, o),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

fn defaults_table() -> toml::Table {
    toml::from_str(DEFAULTS_TOML).expect("built-in defaults parse")
}

pub fn defaults() -> Config {
    defaults_table().try_into().expect("built-in defaults are valid")
}

/// Parses a settings file's text, fills in defaults and validates the result.
pub fn parse(text: &str) -> Result<Config, String> {
    let over: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut table = defaults_table();
    merge(&mut table, over);
    let config: Config = table.try_into().map_err(|e: toml::de::Error| e.to_string())?;
    validate(&config)?;
    Ok(config)
}

pub fn load(path: &str) -> Result<Config, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
    parse(&text)
}

/// Rejects typos in the settings file up front, not on first use.
pub fn validate(config: &Config) -> Result<(), String> {
    let check = |ok: bool, message: String| if ok { Ok(()) } else { Err(message) };
    let scroll = &config.scroll;
    check(SMOOTHNESS.contains(&scroll.smooth.as_str()),
          "scroll.smooth must be off, regular or high".into())?;
    check(SPEEDS.contains(&scroll.speed.as_str()),
          "scroll.speed must be low, medium or high".into())?;
    check(config.drag.scale > 0.0 && config.drag.scale <= 10.0,
          "drag.scale must be a number between 0 and 10".into())?;
    for (name, spec) in &config.buttons {
        let names: Vec<_> = BUTTONS.iter().map(|(n, _)| *n).collect();
        check(lookup(BUTTONS, name).is_some(),
              format!("unknown button {name:?}, expected one of {}", names.join(", ")))?;
        for trigger in ["click", "double-click", "hold"] {
            if let Some(action) = spec.get(trigger) {
                if action != "none" && lookup(CLICKS, action).is_none() {
                    parse_keys(lookup(SHORTCUTS, action).unwrap_or(action))?;
                }
            }
        }
        let drag = spec.get("drag").map_or("none", String::as_str);
        check(DRAG_EFFECTS.contains(&drag),
              format!("buttons.{name}.drag: unknown effect {drag:?}"))?;
        let scroll = spec.get("scroll").map_or("none", String::as_str);
        check(SCROLL_EFFECTS.contains(&scroll),
              format!("buttons.{name}.scroll: unknown effect {scroll:?}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid() {
        validate(&defaults()).unwrap();
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = parse("enabled = false\n[scroll]\nspeed = \"high\"\n[buttons.back]\nhold = \"ctrl+shift+t\"\n").unwrap();
        assert!(!c.enabled);
        assert_eq!(c.scroll.speed, "high");
        assert_eq!(c.scroll.smooth, "high");
        assert_eq!(c.buttons["back"]["hold"], "ctrl+shift+t");
        assert_eq!(c.buttons["back"]["click"], "back");
    }

    #[test]
    fn integer_scale_is_accepted() {
        assert_eq!(parse("[drag]\nscale = 2\n").unwrap().drag.scale, 2.0);
    }

    #[test]
    fn typos_are_rejected() {
        assert!(parse("[scroll]\nspeed = \"warp\"\n").is_err());
        assert!(parse("[buttons.front]\nclick = \"supr+x\"\n").is_err());
        assert!(parse("[buttons.thumb]\nclick = \"back\"\n").is_err());
        assert!(parse("[buttons.back]\ndrag = \"fly\"\n").is_err());
        assert!(parse("enabled = \"yes\"\n").is_err());
        assert!(parse("[drag]\nscale = 0\n").is_err());
    }

    #[test]
    fn keys_parse() {
        assert_eq!(parse_keys("ctrl+shift+t").unwrap(),
                   vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_T]);
        assert_eq!(parse_keys("super+pagedown").unwrap(),
                   vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_PAGEDOWN]);
        let names = key_names();
        assert_eq!(names["a"], KeyCode::KEY_A.0);
        assert_eq!(names["f5"], KeyCode::KEY_F5.0);
    }
}
