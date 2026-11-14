//! Global shortcuts and their rebinding (all five can be rebound, and each can have a second binding for games that grab the first).

use std::collections::BTreeMap;

/// A key with modifiers, written as people write it: "Ctrl+Alt+K".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Combo {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// Windows virtual-key code.
    pub vk: u32,
}

/// Names for keys that are not a letter or digit.
const NAMED: &[(u32, &str)] = &[
    (0x70, "F1"), (0x71, "F2"), (0x72, "F3"), (0x73, "F4"), (0x74, "F5"), (0x75, "F6"), (0x76, "F7"), (0x77, "F8"),
    (0x78, "F9"), (0x79, "F10"), (0x7A, "F11"), (0x7B, "F12"), (0x20, "Space"), (0x0D, "Enter"), (0x2E, "Delete"),
    (0x24, "Home"), (0x23, "End"), (0x21, "PageUp"), (0x22, "PageDown"), (0x2D, "Insert"), (0x13, "Pause"),
    (0x60, "Num0"), (0x61, "Num1"), (0x62, "Num2"), (0x63, "Num3"), (0x64, "Num4"), (0x65, "Num5"), (0x66, "Num6"),
    (0x67, "Num7"), (0x68, "Num8"), (0x69, "Num9"), (0xC0, "`"), (0xBD, "-"), (0xBB, "="), (0xDB, "["), (0xDD, "]"),
    (0xBA, ";"), (0xDE, "'"), (0xBC, ","), (0xBE, "."), (0xBF, "/"), (0xDC, "\\"),
];

pub fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).unwrap().to_string(),
        _ => NAMED.iter().find(|(v, _)| *v == vk).map(|(_, n)| n.to_string()).unwrap_or_else(|| format!("Key{vk:02X}")),
    }
}

fn key_code(name: &str) -> Option<u32> {
    let up = name.to_ascii_uppercase();
    if up.len() == 1 && up.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(up.chars().next().unwrap() as u32);
    }
    NAMED.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(v, _)| *v)
}

impl Combo {
    pub const fn new(ctrl: bool, alt: bool, shift: bool, vk: u32) -> Self {
        Combo { ctrl, alt, shift, vk }
    }

    pub fn parse(s: &str) -> Option<Combo> {
        let mut c = Combo::new(false, false, false, 0);
        for part in s.split('+').map(str::trim) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => c.ctrl = true,
                "alt" => c.alt = true,
                "shift" => c.shift = true,
                "" => return None,
                _ if c.vk == 0 => c.vk = key_code(part)?,
                _ => return None,
            }
        }
        (c.vk != 0).then_some(c)
    }

    /// A global shortcut needs Ctrl or Alt, or it would take a key from every other program.
    pub fn usable(&self) -> bool {
        (self.ctrl || self.alt) && !matches!(self.vk, 0x10 | 0x11 | 0x12 | 0x5B | 0x5C)
    }
}

impl std::fmt::Display for Combo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        parts.push(key_name(self.vk));
        write!(f, "{}", parts.join("+"))
    }
}

/// The actions that have global shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Global {
    Layer,
    Bypass,
    NextLayout,
    Emergency,
    Quit,
    /// Hide or show cutout 1–9: the modifiers apply to the digit keys.
    Cutouts,
}

impl Global {
    pub const ALL: [Global; 6] = [Global::Layer, Global::Bypass, Global::NextLayout, Global::Cutouts, Global::Emergency, Global::Quit];

    pub fn id(self) -> &'static str {
        match self {
            Global::Layer => "layer",
            Global::Bypass => "bypass",
            Global::NextLayout => "nextLayout",
            Global::Emergency => "emergency",
            Global::Quit => "quit",
            Global::Cutouts => "cutouts",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Global::Layer => "Show or hide the layer",
            Global::Bypass => "Show original",
            Global::NextLayout => "Next layout",
            Global::Emergency => "Emergency off",
            Global::Quit => "Quit Kiritori",
            Global::Cutouts => "Hide or show cutout 1–9",
        }
    }

    pub fn default_combo(self) -> Combo {
        match self {
            Global::Layer => Combo::new(true, true, false, 'K' as u32),
            Global::Bypass => Combo::new(true, true, false, 'B' as u32),
            Global::NextLayout => Combo::new(true, true, false, 'L' as u32),
            Global::Emergency => Combo::new(true, true, true, 'K' as u32),
            Global::Quit => Combo::new(true, true, true, 'Q' as u32),
            // The key is a placeholder: the digits 1–9 are used.
            Global::Cutouts => Combo::new(true, true, false, '1' as u32),
        }
    }
}

/// Each action's bindings: a first and an optional second. Stored in settings.json as
/// `{"layer": ["Ctrl+Alt+K"], ...}`; anything missing or unreadable uses the default.
#[derive(Clone, Debug, PartialEq)]
pub struct Bindings(pub BTreeMap<Global, [Option<Combo>; 2]>);

impl Bindings {
    pub fn from_settings(map: &BTreeMap<String, Vec<String>>) -> Self {
        let mut out = BTreeMap::new();
        for g in Global::ALL {
            let mut slots = [Some(g.default_combo()), None];
            if let Some(v) = map.get(g.id()) {
                slots = [v.first().and_then(|s| Combo::parse(s)), v.get(1).and_then(|s| Combo::parse(s))];
                if slots[0].is_none() && slots[1].is_none() {
                    slots[0] = Some(g.default_combo());
                }
            }
            out.insert(g, slots);
        }
        Bindings(out)
    }

    pub fn to_settings(&self) -> BTreeMap<String, Vec<String>> {
        self.0
            .iter()
            .filter(|(g, s)| **s != [Some(g.default_combo()), None])
            .map(|(g, s)| (g.id().to_string(), s.iter().flatten().map(|c| c.to_string()).collect()))
            .collect()
    }

    pub fn get(&self, g: Global) -> [Option<Combo>; 2] {
        self.0.get(&g).copied().unwrap_or([Some(g.default_combo()), None])
    }

    /// Which other action already uses this combination, if any.
    pub fn clash(&self, c: Combo, except: (Global, usize)) -> Option<Global> {
        self.0.iter().find_map(|(g, slots)| {
            slots.iter().enumerate().any(|(i, s)| (*g, i) != except && s.is_some_and(|s| same(*g, s, c))).then_some(*g)
        })
    }
}

/// Two bindings collide; the cutout row stands for all nine digits.
fn same(g: Global, have: Combo, c: Combo) -> bool {
    let mods = |x: Combo| (x.ctrl, x.alt, x.shift);
    if g == Global::Cutouts {
        mods(have) == mods(c) && (0x31..=0x39).contains(&c.vk)
    } else {
        have == c
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_read_and_write() {
        let c = Combo::parse("Ctrl+Alt+K").unwrap();
        assert_eq!(c, Combo::new(true, true, false, 'K' as u32));
        assert_eq!(c.to_string(), "Ctrl+Alt+K");
        assert_eq!(Combo::parse("ctrl + shift + f9").unwrap().to_string(), "Ctrl+Shift+F9");
        assert_eq!(Combo::parse("Alt+/").unwrap().vk, 0xBF);
        assert!(Combo::parse("Ctrl+").is_none());
        assert!(Combo::parse("Ctrl+K+J").is_none());
        assert!(Combo::parse("Ctrl+Nonsense").is_none());
        assert!(!Combo::parse("K").unwrap().usable(), "a bare key would be taken from every program");
        assert!(Combo::parse("Alt+F9").unwrap().usable());
    }

    #[test]
    fn bindings_round_trip_through_settings() {
        let mut map = BTreeMap::new();
        map.insert("layer".to_string(), vec!["Ctrl+Alt+J".to_string(), "Ctrl+Shift+F12".to_string()]);
        map.insert("bypass".to_string(), vec!["garbage".to_string()]);
        let b = Bindings::from_settings(&map);
        assert_eq!(b.get(Global::Layer)[1].unwrap().to_string(), "Ctrl+Shift+F12");
        assert_eq!(b.get(Global::Bypass)[0], Some(Global::Bypass.default_combo()), "unreadable falls back to the default");
        let saved = b.to_settings();
        assert_eq!(saved.len(), 1, "defaults are not written: {saved:?}");
        assert_eq!(Bindings::from_settings(&saved), b);
    }

    #[test]
    fn clashes_are_found_including_the_digit_row() {
        let b = Bindings::from_settings(&BTreeMap::new());
        assert_eq!(b.clash(Combo::parse("Ctrl+Alt+B").unwrap(), (Global::Layer, 0)), Some(Global::Bypass));
        assert_eq!(b.clash(Combo::parse("Ctrl+Alt+5").unwrap(), (Global::Layer, 0)), Some(Global::Cutouts));
        assert_eq!(b.clash(Combo::parse("Ctrl+Alt+K").unwrap(), (Global::Layer, 0)), None, "its own slot is not a clash");
        assert_eq!(b.clash(Combo::parse("Ctrl+Alt+J").unwrap(), (Global::Layer, 0)), None);
    }
}
