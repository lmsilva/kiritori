//! Pure data and logic: cutouts, placements, layouts, profiles, history. No system calls, so
//! everything here is unit-tested.

pub mod display;
pub mod geometry;
pub mod history;
pub mod layout;
pub mod profile;
pub mod shortcut;
pub mod suggest;

#[cfg(test)]
mod format_tests {
    use super::layout::*;
    use super::profile::*;

    /// An example profile file must load unchanged and round-trip.
    const EXAMPLE: &str = r#"{
  "schema": 1,
  "name": "Sample Game",
  "match": { "exe": "game.exe", "windowClass": "", "titleContains": "" },
  "autoApply": "auto",
  "keepWhenUnfocused": false,
  "defaultLayout": "single-screen",
  "layouts": [
    {
      "id": "single-screen",
      "name": "Single screen",
      "sourceAspect": [16, 9],
      "displays": {
        "main": { "hardwareId": "<monitor device path>", "label": "Display 1", "mode": "takeover" }
      },
      "cutouts": [
        {
          "number": 1, "name": "Main view", "color": "cyan",
          "rects": [ { "op": "add", "x": 0.5, "y": 0, "w": 0.5, "h": 0.5 } ],
          "placement": { "display": "main", "x": 0.125, "y": 0, "w": 0.75, "h": 0.75,
                         "visible": true, "locked": false, "z": 1, "filter": "smooth" }
        },
        {
          "number": 2, "name": "Controls", "color": "amber",
          "rects": [ { "op": "add", "x": 0.50625, "y": 0.51111, "w": 0.48828, "h": 0.34722 } ],
          "placement": { "display": "main", "x": 0.32422, "y": 0.75, "w": 0.35156, "h": 0.25,
                         "visible": true, "locked": false, "z": 2, "filter": "smooth" }
        }
      ]
    }
  ]
}"#;

    #[test]
    fn spec_example_loads_and_round_trips() {
        let p: Profile = serde_json::from_str(EXAMPLE).expect("example parses");
        assert_eq!(p.name, "Sample Game");
        assert_eq!(p.remove_frame, FrameRemoval::Auto, "missing new fields take their defaults");
        let l = p.active().unwrap();
        assert_eq!(l.cutouts.len(), 2);
        assert_eq!(l.cutouts[1].color, CutoutColor::Amber);
        assert_eq!(l.cutouts[0].placement.filter, Filter::Smooth);
        assert_eq!(l.displays["main"].mode, DisplayMode::Takeover);

        let json = serde_json::to_string_pretty(&p).unwrap();
        let back: Profile = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(json.contains("\"op\": \"add\""));
        assert!(json.contains("\"windowClass\""));
        assert!(!json.contains("before_fill") && !json.contains("beforeFill"));
    }
}

#[cfg(test)]
mod standin_profile {
    /// The hand-written test profile shipped with the stand-in must stay loadable.
    #[test]
    fn loads() {
        let text = include_str!("../../../tools/standin/profiles/kiritori-stand-in.json");
        let p: super::profile::Profile = serde_json::from_str(text).unwrap();
        assert_eq!(p.active().unwrap().cutouts.len(), 4);
    }
}
