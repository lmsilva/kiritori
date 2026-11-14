//! Reads and writes profiles and settings inside the Kiritori folder.
//! Nothing is written anywhere else. If the folder is read-only, Kiritori keeps working with
//! changes in memory only and says so.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::profile::{Profile, SCHEMA, slug, unique};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub schema: u32,
    pub theme: Theme,
    pub start_with_windows: bool,
    pub show_apply_toast: bool,
    /// Set after the first-run notification has been shown.
    pub first_run_done: bool,
    /// Set after the layer's first-open hints have been shown.
    pub layer_hints_done: bool,
    /// Global shortcuts changed from their defaults: action → first and second binding.
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub shortcuts: std::collections::BTreeMap<String, Vec<String>>,
    /// Show the performance readout in the layer.
    pub performance: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            schema: SCHEMA,
            theme: Theme::System,
            start_with_windows: false,
            show_apply_toast: true,
            first_run_done: false,
            layer_hints_done: false,
            shortcuts: Default::default(),
            performance: false,
        }
    }
}

/// A window frame Kiritori removed, as recorded in the running marker.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrameRecord {
    pub hwnd: i64,
    pub style: i64,
    pub ex_style: i64,
    pub rect: [i32; 4],
}

/// Written while Kiritori runs and deleted on a clean exit. Finding it at start-up means the
/// last run crashed or was killed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunMarker {
    pub pid: u32,
    /// Window frames removed and not yet put back.
    #[serde(default)]
    pub frames: Vec<FrameRecord>,
}

/// A profile as it lives on disk: its file stem (`sample-game`) and contents.
#[derive(Clone, Debug)]
pub struct StoredProfile {
    pub stem: String,
    pub profile: Profile,
}

pub struct Store {
    root: PathBuf,
    writable: bool,
    /// Profiles already backed up this session; the backup holds the previous session's file.
    backed_up: HashSet<String>,
}

#[derive(Debug)]
pub struct LoadReport {
    pub profiles: Vec<StoredProfile>,
    /// Files that could not be read, with the reason, so the user can be told.
    pub problems: Vec<(PathBuf, String)>,
}

impl Store {
    /// The folder holding the running executable.
    pub fn beside_exe() -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        Ok(Self::open(exe.parent().unwrap_or(Path::new("."))))
    }

    pub fn open(root: &Path) -> Self {
        let root = root.to_path_buf();
        let writable = ["profiles", "backups", "logs"].iter().all(|d| fs::create_dir_all(root.join(d)).is_ok()) && {
            let probe = root.join(".write-test");
            let ok = fs::write(&probe, b"ok").is_ok();
            let _ = fs::remove_file(&probe);
            ok
        };
        Store { root, writable, backed_up: HashSet::new() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// False when the folder cannot be written; the tray then shows "Not saving".
    pub fn writable(&self) -> bool {
        self.writable
    }

    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    fn profile_path(&self, stem: &str) -> PathBuf {
        self.root.join("profiles").join(format!("{stem}.json"))
    }

    fn backup_path(&self, stem: &str) -> PathBuf {
        self.root.join("backups").join(format!("{stem}.previous.json"))
    }

    pub fn load_profiles(&self) -> LoadReport {
        let mut report = LoadReport { profiles: Vec::new(), problems: Vec::new() };
        let Ok(entries) = fs::read_dir(self.root.join("profiles")) else { return report };
        let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
        paths.sort();
        for path in paths {
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            match fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<Profile>(&t).map_err(|e| e.to_string())) {
                Ok(profile) if profile.schema <= SCHEMA => report.profiles.push(StoredProfile { stem, profile }),
                Ok(profile) => report.problems.push((path, format!("made by a newer Kiritori (schema {})", profile.schema))),
                Err(e) => report.problems.push((path, e)),
            }
        }
        report
    }

    /// A file stem for a new profile, unique among existing files.
    pub fn new_stem(&self, name: &str) -> String {
        let taken: Vec<String> = fs::read_dir(self.root.join("profiles"))
            .map(|d| d.filter_map(|e| e.ok()?.path().file_stem()?.to_str().map(String::from)).collect())
            .unwrap_or_default();
        unique(&slug(name, "profile"), &taken)
    }

    /// Save a profile. The first save in a session keeps the existing file as the backup.
    /// Returns Ok(false) without writing when the folder is read-only.
    pub fn save_profile(&mut self, stem: &str, profile: &Profile) -> io::Result<bool> {
        if !self.writable {
            return Ok(false);
        }
        let path = self.profile_path(stem);
        if self.backed_up.insert(stem.to_string()) && path.exists() {
            fs::copy(&path, self.backup_path(stem))?;
        }
        let mut tidy = profile.clone();
        for l in &mut tidy.layouts {
            l.tidy();
        }
        write_atomic(&path, serde_json::to_string_pretty(&tidy)?.as_bytes())?;
        Ok(true)
    }

    /// The previous session's version of a profile, if one was kept.
    pub fn load_backup(&self, stem: &str) -> Option<Profile> {
        serde_json::from_str(&fs::read_to_string(self.backup_path(stem)).ok()?).ok()
    }

    /// Write a profile as one shareable file (the same JSON as in `profiles\`).
    pub fn export_profile(profile: &Profile, path: &Path) -> io::Result<()> {
        let mut tidy = profile.clone();
        for l in &mut tidy.layouts {
            l.tidy();
        }
        write_atomic(path, serde_json::to_string_pretty(&tidy)?.as_bytes())
    }

    /// Read a profile file someone shared. Errors are written for a person.
    pub fn read_profile_file(path: &Path) -> Result<Profile, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("could not read the file: {e}"))?;
        let p: Profile = serde_json::from_str(&text).map_err(|e| format!("this is not a Kiritori profile ({e})"))?;
        if p.schema > SCHEMA {
            return Err(format!("it was made by a newer Kiritori (schema {})", p.schema));
        }
        if p.layouts.is_empty() || p.match_rule.exe.trim().is_empty() {
            return Err("the profile has no layouts or no app".into());
        }
        Ok(p)
    }

    /// Remove a profile, keeping a copy as `backups/<stem>.deleted.json` so Import can bring it
    /// back.
    pub fn delete_profile(&mut self, stem: &str) -> io::Result<()> {
        if self.writable {
            let p = self.profile_path(stem);
            if p.exists() {
                let keep = self.root.join("backups").join(format!("{stem}.deleted.json"));
                if fs::copy(&p, &keep).is_err() {
                    crate::log!("could not keep a copy of {stem}.json before deleting it");
                }
                fs::remove_file(p)?;
            }
        }
        Ok(())
    }

    pub fn marker_path(&self) -> PathBuf {
        self.root.join("kiritori.running")
    }

    /// The marker left by the last run, if it did not exit cleanly.
    pub fn read_marker(&self) -> Option<RunMarker> {
        serde_json::from_str(&fs::read_to_string(self.marker_path()).ok()?).ok()
    }

    pub fn write_marker(&self, m: &RunMarker) {
        if self.writable {
            let _ = write_atomic(&self.marker_path(), serde_json::to_string_pretty(m).unwrap_or_default().as_bytes());
        }
    }

    pub fn remove_marker(&self) {
        let _ = fs::remove_file(self.marker_path());
    }

    pub fn load_settings(&self) -> Settings {
        fs::read_to_string(self.root.join("settings.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save_settings(&self, s: &Settings) -> io::Result<bool> {
        if !self.writable {
            return Ok(false);
        }
        write_atomic(&self.root.join("settings.json"), serde_json::to_string_pretty(s)?.as_bytes())?;
        Ok(true)
    }
}

/// Write to a temporary file, then rename over the target, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::{Rect, Size};
    use crate::model::layout::{DisplayMode, DisplayRef, Frame};
    use crate::model::profile::WindowFacts;

    fn temp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("kiritori-test-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample() -> Profile {
        let w = WindowFacts { exe_path: r"C:\g\kiritori-standin.exe".into(), window_class: "KiritoriStandIn".into(), title: "Sample Game".into() };
        let d = DisplayRef { hardware_id: "x".into(), label: "Display 1".into(), mode: DisplayMode::Takeover };
        let mut p = Profile::new_for(&w, d, &[]);
        let f = Frame { source_px: Size::new(1920.0, 1080.0), canvas_px: Size::new(1920.0, 1080.0), source_on_canvas: Rect::new(0.0, 0.0, 1.0, 1.0) };
        p.layouts[0].add_cutout(Rect::new(0.5, 0.0, 0.5, 0.5), &f);
        p
    }

    #[test]
    fn creates_folders_and_round_trips_profiles() {
        let root = temp_root("roundtrip");
        let mut s = Store::open(&root);
        assert!(s.writable());
        for d in ["profiles", "backups", "logs"] {
            assert!(root.join(d).is_dir());
        }
        let p = sample();
        let stem = s.new_stem(&p.name);
        assert_eq!(stem, "sample-game");
        assert!(s.save_profile(&stem, &p).unwrap());
        let loaded = s.load_profiles();
        assert!(loaded.problems.is_empty());
        assert_eq!(loaded.profiles.len(), 1);
        assert_eq!(loaded.profiles[0].stem, "sample-game");
        assert_eq!(loaded.profiles[0].profile, p);
        // A second profile with the same name gets its own file.
        assert_eq!(s.new_stem(&p.name), "sample-game-2");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn first_save_of_a_session_keeps_the_previous_file() {
        let root = temp_root("backup");
        let mut p = sample();
        {
            let mut s = Store::open(&root);
            s.save_profile("sample-game", &p).unwrap(); // session 1: nothing to back up yet
            assert!(s.load_backup("sample-game").is_none());
        }
        let session1 = p.clone();
        let mut s = Store::open(&root); // session 2
        p.name = "Renamed".into();
        s.save_profile("sample-game", &p).unwrap();
        p.name = "Renamed again".into();
        s.save_profile("sample-game", &p).unwrap();
        assert_eq!(s.load_backup("sample-game").unwrap(), session1, "backup is the previous session, not the previous save");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn broken_and_newer_files_are_reported_not_loaded() {
        let root = temp_root("broken");
        let mut s = Store::open(&root);
        s.save_profile("good", &sample()).unwrap();
        fs::write(root.join("profiles").join("broken.json"), "{ not json").unwrap();
        let mut newer = serde_json::to_value(sample()).unwrap();
        newer["schema"] = 99.into();
        fs::write(root.join("profiles").join("newer.json"), newer.to_string()).unwrap();
        let r = s.load_profiles();
        assert_eq!(r.profiles.len(), 1);
        assert_eq!(r.problems.len(), 2);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_default_and_round_trip() {
        let root = temp_root("settings");
        let s = Store::open(&root);
        let mut st = s.load_settings();
        assert_eq!(st, Settings::default());
        assert!(!st.start_with_windows, "off by default");
        st.theme = Theme::Dark;
        st.first_run_done = true;
        s.save_settings(&st).unwrap();
        assert_eq!(s.load_settings(), st);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn exported_profiles_import_and_bad_files_are_explained() {
        let root = temp_root("export");
        let file = root.join("shared.json");
        Store::export_profile(&sample(), &file).unwrap();
        let back = Store::read_profile_file(&file).unwrap();
        assert_eq!(back.name, sample().name);
        assert_eq!(back.layouts.len(), sample().layouts.len());
        fs::write(root.join("junk.json"), "{ not json").unwrap();
        assert!(Store::read_profile_file(&root.join("junk.json")).unwrap_err().contains("not a Kiritori profile"));
        let mut newer = serde_json::to_value(sample()).unwrap();
        newer["schema"] = serde_json::json!(99);
        fs::write(root.join("newer.json"), newer.to_string()).unwrap();
        assert!(Store::read_profile_file(&root.join("newer.json")).unwrap_err().contains("newer Kiritori"));
        assert!(Store::read_profile_file(&root.join("missing.json")).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn a_deleted_profile_leaves_a_copy_in_backups() {
        let root = temp_root("deleted");
        let mut s = Store::open(&root);
        fs::write(s.profile_path("game"), "{\"schema\":1}").unwrap();
        s.delete_profile("game").unwrap();
        assert!(!s.profile_path("game").exists());
        assert_eq!(fs::read_to_string(root.join("backups").join("game.deleted.json")).unwrap(), "{\"schema\":1}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn the_run_marker_round_trips_and_goes_away() {
        let root = temp_root("marker");
        let s = Store::open(&root);
        assert!(s.read_marker().is_none());
        let m = RunMarker { pid: 42, frames: vec![FrameRecord { hwnd: 1234, style: 0x14CF0000, ex_style: 0x100, rect: [40, 40, 1986, 1191] }] };
        s.write_marker(&m);
        assert_eq!(s.read_marker(), Some(m));
        s.remove_marker();
        assert!(s.read_marker().is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn saved_numbers_are_rounded() {
        let root = temp_root("tidy");
        let mut s = Store::open(&root);
        let mut p = sample();
        p.layouts[0].cutouts[0].placement.rect.x = 1.0 / 3.0;
        s.save_profile("t", &p).unwrap();
        let text = fs::read_to_string(root.join("profiles").join("t.json")).unwrap();
        assert!(text.contains("0.33333"), "{text}");
        assert!(!text.contains("0.333333"));
        let _ = fs::remove_dir_all(&root);
    }
}
