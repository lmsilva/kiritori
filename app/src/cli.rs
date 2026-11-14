//! The command line: lets game launchers and frontends drive Kiritori per game.
//!
//! ```text
//! kiritori                                   start, or open the running copy's tray menu
//! kiritori --apply <profile> [--layout <name>]
//! kiritori --launch <exe> [--layout <name>] [-- <arguments for the app>]
//! kiritori --bypass [on|off]
//! kiritori --pick
//! kiritori --profiles
//! kiritori --edit
//! kiritori --quit
//! ```
//!
//! A second launch hands its arguments to the running copy, which parses them again; a first
//! launch acts on them once it has started.

use crate::store::StoredProfile;

pub const USAGE: &str = "Usage:
  kiritori --apply <profile> [--layout <name>]
  kiritori --launch <exe> [--layout <name>] [-- <app arguments>]
  kiritori --bypass [on|off]
  kiritori --pick
  kiritori --profiles
  kiritori --settings
  kiritori --edit
  kiritori --import <file>
  kiritori --export <profile> <file>
  kiritori --quit
  kiritori --help";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// No arguments: start, or open the running copy's tray menu.
    Menu,
    /// Use this profile (and layout) whenever its app is in front, even if its auto-apply is
    /// Ask or Off.
    Apply { profile: String, layout: Option<String> },
    /// Start an app with its profile's layout ready.
    Launch { exe: String, layout: Option<String>, args: Vec<String> },
    /// Show original: on, off, or toggle (None).
    Bypass(Option<bool>),
    Pick,
    /// Open the Profiles panel.
    Profiles,
    /// Open the Settings panel.
    Settings,
    /// Open the layer over the app in front (the same as Ctrl+Alt+K).
    Edit,
    /// Take over from a running copy (used by Restart as administrator).
    Replace,
    /// Add a profile from a shared file.
    Import(String),
    /// Write a profile to a file to share.
    Export { profile: String, file: String },
    Quit,
}

/// `--help`, `-h`, `/?` or `-?` as the first argument: show the usage and stop.
pub fn wants_help(args: &[String]) -> bool {
    args.first().is_some_and(|a| matches!(a.as_str(), "--help" | "-h" | "/?" | "-?"))
}

/// Parse the arguments after the program name. Errors are written for a person.
pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut it = args.iter().map(String::as_str).peekable();
    let Some(first) = it.next() else { return Ok(Command::Menu) };
    let mut layout = None;
    let cmd = match first {
        "--apply" => {
            let profile = it.next().filter(|v| !v.starts_with("--")).ok_or("--apply needs a profile name")?.to_string();
            Command::Apply { profile, layout: None }
        }
        "--launch" => {
            let exe = it.next().filter(|v| !v.starts_with("--")).ok_or("--launch needs the path of an app")?.to_string();
            Command::Launch { exe, layout: None, args: Vec::new() }
        }
        "--bypass" => match it.peek().copied() {
            Some("on") => {
                it.next();
                Command::Bypass(Some(true))
            }
            Some("off") => {
                it.next();
                Command::Bypass(Some(false))
            }
            _ => Command::Bypass(None),
        },
        "--pick" => Command::Pick,
        "--profiles" => Command::Profiles,
        "--settings" => Command::Settings,
        "--edit" => Command::Edit,
        "--replace" => Command::Replace,
        "--import" => Command::Import(it.next().filter(|v| !v.starts_with("--")).ok_or("--import needs the path of a profile file")?.to_string()),
        "--export" => {
            let profile = it.next().filter(|v| !v.starts_with("--")).ok_or("--export needs a profile name and a file")?.to_string();
            let file = it.next().filter(|v| !v.starts_with("--")).ok_or("--export needs a file to write")?.to_string();
            Command::Export { profile, file }
        }
        "--quit" => Command::Quit,
        other => return Err(format!("unknown option {other}")),
    };
    let mut rest = Vec::new();
    while let Some(a) = it.next() {
        match a {
            "--layout" => layout = Some(it.next().ok_or("--layout needs a layout name")?.to_string()),
            "--" => {
                rest = it.by_ref().map(str::to_string).collect();
            }
            other => return Err(format!("unexpected {other}")),
        }
    }
    Ok(match cmd {
        Command::Apply { profile, .. } => Command::Apply { profile, layout },
        Command::Launch { exe, .. } => Command::Launch { exe, layout, args: rest },
        _ if layout.is_some() => return Err(format!("--layout only goes with --apply or --launch, not {first}")),
        _ if !rest.is_empty() => return Err(format!("app arguments only go with --launch, not {first}")),
        c => c,
    })
}

fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// A profile by its name or file stem, ignoring case.
pub fn find_profile(profiles: &[StoredProfile], name: &str) -> Option<usize> {
    let n = norm(name);
    profiles.iter().position(|p| norm(&p.profile.name) == n || norm(&p.stem) == n)
}

/// A profile whose match rule names this executable (and, when the rule has one, its folder).
pub fn profile_for_exe(profiles: &[StoredProfile], exe: &str) -> Option<usize> {
    let path = exe.replace('/', "\\");
    let file = path.rsplit('\\').next().unwrap_or(&path).to_lowercase();
    let dir = path.rsplit_once('\\').map(|(d, _)| d.to_lowercase()).unwrap_or_default();
    profiles.iter().position(|p| {
        let r = &p.profile.match_rule;
        r.exe.to_lowercase() == file && (r.folder.is_empty() || dir.ends_with(&r.folder.to_lowercase().trim_end_matches('\\').to_string()))
    })
}

/// A layout of a profile by its name or id, ignoring case.
pub fn find_layout(p: &crate::model::profile::Profile, name: &str) -> Option<String> {
    let n = norm(name);
    p.layouts.iter().find(|l| norm(&l.name) == n || norm(&l.id) == n).map(|l| l.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Command, String> {
        parse(&s.split_whitespace().map(String::from).collect::<Vec<_>>())
    }

    #[test]
    fn help_is_asked_for_not_an_error() {
        let a = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert!(wants_help(&a("--help")) && wants_help(&a("-h")) && wants_help(&a("/?")));
        assert!(!wants_help(&a("--apply --help")) && !wants_help(&[]));
    }

    #[test]
    fn options_parse() {
        assert_eq!(p(""), Ok(Command::Menu));
        assert_eq!(p("--apply sample"), Ok(Command::Apply { profile: "sample".into(), layout: None }));
        assert_eq!(p("--apply sample --layout single"), Ok(Command::Apply { profile: "sample".into(), layout: Some("single".into()) }));
        assert_eq!(p("--bypass"), Ok(Command::Bypass(None)));
        assert_eq!(p("--bypass on"), Ok(Command::Bypass(Some(true))));
        assert_eq!(p("--bypass off"), Ok(Command::Bypass(Some(false))));
        assert_eq!(p("--pick"), Ok(Command::Pick));
        assert_eq!(p("--profiles"), Ok(Command::Profiles));
        assert_eq!(p("--import a.json"), Ok(Command::Import("a.json".into())));
        assert_eq!(p("--export sample out.json"), Ok(Command::Export { profile: "sample".into(), file: "out.json".into() }));
        assert!(p("--export sample").is_err());
        assert_eq!(p("--quit"), Ok(Command::Quit));
        assert_eq!(
            p("--launch C:\\g\\game.exe --layout single -- -fullscreen -x"),
            Ok(Command::Launch { exe: "C:\\g\\game.exe".into(), layout: Some("single".into()), args: vec!["-fullscreen".into(), "-x".into()] })
        );
    }

    #[test]
    fn mistakes_are_explained() {
        assert!(p("--apply").is_err());
        assert!(p("--apply --layout x").is_err());
        assert!(p("--launch").is_err());
        assert!(p("--bypass maybe").is_err());
        assert!(p("--quit --layout x").is_err());
        assert!(p("--frobnicate").is_err());
        assert!(p("--apply a --layout").is_err());
    }

    #[test]
    fn profiles_and_layouts_are_found_by_name_or_file() {
        use crate::model::layout::{DisplayMode, DisplayRef};
        use crate::model::profile::{Profile, WindowFacts};
        let facts = WindowFacts { exe_path: r"C:\Games\Sample\Binaries\Win64\Sample-Win64-Shipping.exe".into(), window_class: "UnrealWindow".into(), title: "Sample Game".into() };
        let prof = Profile::new_for(&facts, DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover }, &[]);
        let list = vec![StoredProfile { stem: "sample-game".into(), profile: prof }];
        assert_eq!(find_profile(&list, "SAMPLE GAME"), Some(0));
        assert_eq!(find_profile(&list, "sample-game"), Some(0));
        assert_eq!(find_profile(&list, "other"), None);
        assert_eq!(profile_for_exe(&list, r"C:\Games\Sample\Binaries\Win64\Sample-Win64-Shipping.exe"), Some(0));
        assert_eq!(profile_for_exe(&list, r"C:\Elsewhere\Sample-Win64-Shipping.exe"), None, "a generic exe also needs its folder");
        let first = list[0].profile.layouts[0].name.clone();
        assert!(find_layout(&list[0].profile, &first.to_uppercase()).is_some());
        assert!(find_layout(&list[0].profile, "nope").is_none());
    }
}
