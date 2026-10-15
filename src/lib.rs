//! Baldur's Gate 3 Honour Mode save tools.
//!
//! When an Honour Mode party wipes and the run continues in Custom Mode, the save itself is
//! left untouched (meta.lsf / SaveInfo.json still report `RulesetHonour`). Instead, the game
//! appends the run's GameSessionID to `DisabledSingleSaveSessions` in the player profile
//! (`profile8.lsf`). Removing the ID from that list revives Honour Mode for the run.
//!
//! LSF/LSV handling goes through LSLib's Divine.exe.

use chrono::{DateTime, Local};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
    time::SystemTime,
};

pub type Result<T> = std::result::Result<T, String>;

pub const LSLIB_RELEASES_URL: &str = "https://github.com/Norbyte/lslib/releases";
const BG3_STEAM_APP_ID: &str = "1086940";
const PROFILE_FILE: &str = "profile8.lsf";
const HONOUR_SUFFIX: &str = "__HonourMode";
const SUMMARY_FILE: &str = "summary.json";
const APP_DIR: &str = "bg3-save-tools";

// ---------------------------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Paths {
    /// PlayerProfiles\Public
    pub profile_dir: PathBuf,
    pub steam_dir: Option<PathBuf>,
    pub backup_root: PathBuf,
}

impl Paths {
    pub fn detect() -> Result<Self> {
        let local = dirs::data_local_dir().ok_or("could not find %LOCALAPPDATA%")?;
        let documents = dirs::document_dir().ok_or("could not find the Documents folder")?;
        Ok(Self {
            profile_dir: local
                .join("Larian Studios")
                .join("Baldur's Gate 3")
                .join("PlayerProfiles")
                .join("Public"),
            steam_dir: steam_dir(),
            backup_root: documents.join("BG3 Save Tools"),
        })
    }

    pub fn story_dir(&self) -> PathBuf {
        self.profile_dir.join("Savegames").join("Story")
    }

    pub fn save_dir(&self, session_id: &str) -> PathBuf {
        self.story_dir().join(format!("{session_id}{HONOUR_SUFFIX}"))
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.backup_root.join("backups")
    }

    fn profile_file(&self) -> PathBuf {
        self.profile_dir.join(PROFILE_FILE)
    }

    /// `userdata\<account>\1086940\remote` for every Steam account that has BG3 cloud data.
    fn steam_remotes(&self) -> Vec<(String, PathBuf)> {
        let Some(Ok(users)) = self.steam_dir.as_ref().map(|s| fs::read_dir(s.join("userdata")))
        else {
            return vec![];
        };
        users
            .flatten()
            .map(|u| {
                let remote = u.path().join(BG3_STEAM_APP_ID).join("remote");
                (format!("steam_{}", u.file_name().to_string_lossy()), remote)
            })
            .filter(|(_, remote)| remote.is_dir())
            .collect()
    }
}

/// Steam install directory from the registry, falling back to the default location.
fn steam_dir() -> Option<PathBuf> {
    let from_registry = hidden_command("reg")
        .args(["query", r"HKCU\Software\Valve\Steam", "/v", "SteamPath"])
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|l| l.split("REG_SZ").nth(1).map(|p| PathBuf::from(p.trim())))
        });
    from_registry
        .into_iter()
        .chain([PathBuf::from(r"C:\Program Files (x86)\Steam")])
        .find(|p| p.join("userdata").is_dir())
}

// ---------------------------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------------------------

#[derive(Default, Serialize, Deserialize)]
pub struct Config {
    pub divine: Option<PathBuf>,
}

impl Config {
    fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join(APP_DIR).join("config.json"))
    }

    pub fn load() -> Self {
        Self::path()
            .and_then(|p| fs::read(p).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path().ok_or("could not find %APPDATA%")?;
        create_dir(path.parent().unwrap_or(&path))?;
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        write(&path, &json)
    }
}

// ---------------------------------------------------------------------------------------------
// Divine.exe
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Divine(PathBuf);

impl Divine {
    pub fn new(exe: PathBuf) -> Result<Self> {
        if exe.is_file() {
            Ok(Self(exe))
        } else {
            Err(format!("Divine.exe not found at {}", exe.display()))
        }
    }

    /// Finds Divine.exe: the explicit path, `DIVINE_EXE`, the saved config, next to this
    /// executable, then the copy fetched by [`download_lslib`].
    pub fn locate(explicit: Option<PathBuf>) -> Result<Self> {
        if let Some(exe) = explicit {
            return Self::new(exe);
        }
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf));
        let near_exe = exe_dir.into_iter().flat_map(|d| {
            [
                d.join("Divine.exe"),
                d.join("Tools").join("Divine.exe"),
                d.join("lslib").join("Packed").join("Tools").join("Divine.exe"),
            ]
        });
        std::env::var_os("DIVINE_EXE")
            .map(PathBuf::from)
            .into_iter()
            .chain(Config::load().divine)
            .chain(near_exe)
            .chain(lslib_download_dir().map(|d| d.join("Packed").join("Tools").join("Divine.exe")))
            .find(|p| p.is_file())
            .map(Self)
            .ok_or_else(|| {
                format!(
                    "Divine.exe not found: pass --divine, set DIVINE_EXE, or download LSLib's \
                     ExportTool from {LSLIB_RELEASES_URL}"
                )
            })
    }

    pub fn exe(&self) -> &Path {
        &self.0
    }

    fn run(&self, action: &str, src: &Path, dst: &Path, extra: &[&str]) -> Result<()> {
        let out = hidden_command(&self.0)
            .args(["-g", "bg3", "-a", action, "-s"])
            .arg(src)
            .arg("-d")
            .arg(dst)
            .args(extra)
            .output()
            .map_err(|e| format!("failed to run Divine: {e}"))?;
        if !out.status.success() || !dst.exists() {
            return Err(format!(
                "Divine {action} failed for {}: {}",
                src.display(),
                String::from_utf8_lossy(&out.stdout)
            ));
        }
        Ok(())
    }

    fn convert(&self, src: &Path, dst: &Path) -> Result<()> {
        self.run("convert-resource", src, dst, &[])
    }

    /// Extracts the package files whose path matches `regex` into `dst`.
    fn extract(&self, package: &Path, dst: &Path, regex: &str) -> Result<()> {
        self.run("extract-package", package, dst, &["-x", regex, "--use-regex"])
    }

    fn to_lsx(&self, lsf: &Path, work: &WorkDir) -> Result<String> {
        let stem = lsf.file_stem().unwrap_or_default().to_string_lossy();
        let dst = work.0.join(format!("{stem}.lsx"));
        let _ = fs::remove_file(&dst);
        self.convert(lsf, &dst)?;
        let text = fs::read_to_string(&dst).map_err(|e| format!("failed to read lsx: {e}"))?;
        Ok(text.trim_start_matches('\u{feff}').to_string())
    }
}

fn lslib_download_dir() -> Option<PathBuf> {
    dirs::data_local_dir().map(|d| d.join(APP_DIR).join("lslib"))
}

/// Downloads the latest LSLib ExportTool release from GitHub and returns its Divine.exe.
pub fn download_lslib() -> Result<Divine> {
    let dir = lslib_download_dir().ok_or("could not find %LOCALAPPDATA%")?;
    let release: serde_json::Value =
        ureq::get("https://api.github.com/repos/Norbyte/lslib/releases/latest")
            .set("User-Agent", APP_DIR)
            .call()
            .map_err(|e| format!("failed to query LSLib releases: {e}"))?
            .into_json()
            .map_err(|e| format!("failed to parse LSLib release: {e}"))?;
    let url = release["assets"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.starts_with("ExportTool") && n.ends_with(".zip"))
        })
        .and_then(|a| a["browser_download_url"].as_str())
        .ok_or("no ExportTool download in the latest LSLib release")?;

    let mut zip_bytes = vec![];
    ureq::get(url)
        .set("User-Agent", APP_DIR)
        .call()
        .map_err(|e| format!("failed to download LSLib: {e}"))?
        .into_reader()
        .read_to_end(&mut zip_bytes)
        .map_err(|e| format!("failed to download LSLib: {e}"))?;

    let _ = fs::remove_dir_all(&dir);
    zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .and_then(|mut z| z.extract(&dir))
        .map_err(|e| format!("failed to unpack LSLib: {e}"))?;
    Divine::new(dir.join("Packed").join("Tools").join("Divine.exe"))
}


// ---------------------------------------------------------------------------------------------
// Save summaries
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SaveInfo {
    #[serde(rename = "Active Party")]
    active_party: ActiveParty,
    #[serde(rename = "Current Level")]
    current_level: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ActiveParty {
    #[serde(rename = "Characters")]
    characters: Vec<PartyMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PartyMember {
    #[serde(rename = "Classes", default)]
    pub classes: Vec<ClassInfo>,
    #[serde(rename = "Level")]
    pub level: u32,
    #[serde(rename = "Origin")]
    pub origin: String,
    #[serde(rename = "Race")]
    pub race: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ClassInfo {
    #[serde(rename = "Main")]
    pub main: String,
    #[serde(rename = "Sub")]
    pub sub: String,
}

impl PartyMember {
    /// Summons share the party list; they're the only members without classes besides the Emperor.
    pub fn is_summon(&self) -> bool {
        self.classes.is_empty() && self.origin != "S_GLO_Emperor"
    }

    pub fn name(&self) -> &str {
        match self.origin.as_str() {
            // custom characters' names live in the undecoded NewAge blob
            "Generic" => "(custom)",
            "S_GLO_Emperor" => "Emperor",
            o => o,
        }
    }

    pub fn classes_text(&self) -> String {
        self.classes
            .iter()
            .map(|c| match c.sub.as_str() {
                "" => c.main.clone(),
                sub => format!("{} ({sub})", c.main),
            })
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SaveSummary {
    pub level: String,
    pub leader: String,
    /// Total playtime in seconds (meta.lsf `TimeStamp`).
    pub playtime_secs: u64,
    pub saved: String,
    pub party: Vec<PartyMember>,
}

impl SaveSummary {
    pub fn read(divine: &Divine, lsv: &Path) -> Result<Self> {
        let work = WorkDir::new()?;
        let save_dir = work.0.join("save");
        divine.extract(lsv, &save_dir, r"^(SaveInfo\.json|meta\.lsf)$")?;

        let info_json = fs::read(save_dir.join("SaveInfo.json"))
            .map_err(|e| format!("failed to read SaveInfo.json: {e}"))?;
        let info: SaveInfo = serde_json::from_slice(&info_json)
            .map_err(|e| format!("failed to parse SaveInfo.json: {e}"))?;
        let meta = divine.to_lsx(&save_dir.join("meta.lsf"), &work)?;

        Ok(Self {
            level: info.current_level,
            leader: lsx_attribute(&meta, "LeaderName").unwrap_or_default(),
            playtime_secs: lsx_attribute(&meta, "TimeStamp")
                .and_then(|t| t.parse().ok())
                .unwrap_or(0),
            saved: modified_time(lsv),
            party: info.active_party.characters,
        })
    }

    pub fn act(&self) -> &'static str {
        match self.level.split('_').next().unwrap_or_default() {
            "TUT" => "Prologue",
            "WLD" | "CRE" => "Act 1",
            "SCL" | "INT" => "Act 2",
            "BGO" | "CTY" | "IRN" => "Act 3",
            "END" => "Finale",
            _ => "Unknown act",
        }
    }

    pub fn playtime(&self) -> String {
        format!("{}h{:02}m", self.playtime_secs / 3600, self.playtime_secs % 3600 / 60)
    }

    pub fn members(&self) -> impl Iterator<Item = &PartyMember> {
        self.party.iter().filter(|m| !m.is_summon())
    }

    pub fn summons(&self) -> impl Iterator<Item = &PartyMember> {
        self.party.iter().filter(|m| m.is_summon())
    }
}

fn lsx_attribute(lsx: &str, id: &str) -> Option<String> {
    let re = Regex::new(&format!(r#"<attribute id="{id}" type="[^"]*" value="([^"]*)""#)).ok()?;
    re.captures(lsx).map(|c| c[1].to_string())
}

// ---------------------------------------------------------------------------------------------
// Honour runs
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct HonourRun {
    pub session_id: String,
    /// The run's save file, if its folder still exists.
    pub lsv: Option<PathBuf>,
    /// Honour Mode was revoked after a party wipe.
    pub failed: bool,
    pub summary: Option<SaveSummary>,
    pub summary_error: Option<String>,
    /// Newest first.
    pub backups: Vec<Backup>,
}

impl HonourRun {
    /// Summary of the current save, or of the newest backup if the save is gone.
    pub fn best_summary(&self) -> Option<&SaveSummary> {
        self.summary
            .as_ref()
            .or_else(|| self.backups.iter().find_map(|b| b.summary.as_ref()))
    }
}

/// Every Honour run that has a save folder, a backup, or a revoked-Honour entry, newest first.
pub fn honour_runs(divine: &Divine, paths: &Paths) -> Result<Vec<HonourRun>> {
    let failed = failed_sessions(divine, paths)?;
    let mut backups = list_backups(paths, None);

    let mut ids: Vec<String> = fs::read_dir(paths.story_dir())
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(HONOUR_SUFFIX).map(str::to_lowercase)
                })
                .collect()
        })
        .unwrap_or_default();
    ids.extend(failed.iter().cloned());
    ids.extend(backups.iter().map(|b| b.session_id.clone()));
    ids.sort();
    ids.dedup();

    let mut runs: Vec<HonourRun> = ids
        .into_iter()
        .map(|id| {
            let lsv = newest_lsv(&paths.save_dir(&id));
            let (summary, summary_error) = match lsv.as_ref().map(|l| SaveSummary::read(divine, l)) {
                Some(Ok(s)) => (Some(s), None),
                Some(Err(e)) => (None, Some(e)),
                None => (None, None),
            };
            let (run_backups, rest) = backups.drain(..).partition(|b| b.session_id == id);
            backups = rest;
            HonourRun {
                failed: failed.contains(&id),
                session_id: id,
                lsv,
                summary,
                summary_error,
                backups: run_backups,
            }
        })
        .collect();

    // newest activity first: current save, else newest backup
    runs.sort_by_cached_key(|r| {
        std::cmp::Reverse(
            r.lsv
                .as_ref()
                .and_then(|l| fs::metadata(l).and_then(|m| m.modified()).ok())
                .or_else(|| r.backups.first().map(|b| b.created)),
        )
    });
    Ok(runs)
}

/// Resolves a full or abbreviated session ID against the known runs.
pub fn resolve_session<'a>(runs: &'a [HonourRun], id: &str) -> Result<&'a HonourRun> {
    let id = id.to_lowercase();
    let matches: Vec<_> = runs.iter().filter(|r| r.session_id.starts_with(&id)).collect();
    match matches.as_slice() {
        [run] => Ok(run),
        [] => Err(format!("no Honour run matches {id}")),
        _ => Err(format!("{id} matches several runs; use more characters")),
    }
}

fn newest_lsv(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|f| f.path().extension().is_some_and(|e| e.eq_ignore_ascii_case("lsv")))
        .max_by_key(|f| f.metadata().and_then(|m| m.modified()).ok())
        .map(|f| f.path())
}

// ---------------------------------------------------------------------------------------------
// Profile: revoked Honour sessions
// ---------------------------------------------------------------------------------------------

fn session_entry_regex(id: &str) -> Regex {
    // entries are child nodes holding a single guid; the container node shares their name but
    // starts with <children>, so it never matches
    #[allow(clippy::unwrap_used)]
    Regex::new(&format!(
        r#"\r?\n[ \t]*<node id="DisabledSingleSaveSessions">\s*<attribute id="Object" type="guid" value="({id})" />\s*</node>"#
    ))
    .unwrap()
}

fn sessions_in(lsx: &str) -> Vec<String> {
    session_entry_regex("[0-9a-fA-F-]{36}")
        .captures_iter(lsx)
        .map(|c| c[1].to_lowercase())
        .collect()
}

/// Session IDs that had Honour Mode revoked.
pub fn failed_sessions(divine: &Divine, paths: &Paths) -> Result<Vec<String>> {
    let work = WorkDir::new()?;
    Ok(sessions_in(&divine.to_lsx(&paths.profile_file(), &work)?))
}

pub struct ReviveReport {
    pub profile_backup: PathBuf,
    pub patched: Vec<PathBuf>,
}

/// Removes the sessions from `DisabledSingleSaveSessions` in the local profile and every Steam
/// Cloud copy, after backing them up.
pub fn revive(divine: &Divine, paths: &Paths, session_ids: &[String]) -> Result<ReviveReport> {
    ensure_game_closed()?;
    let work = WorkDir::new()?;
    let lsx = divine.to_lsx(&paths.profile_file(), &work)?;

    let mut patched = lsx;
    for id in session_ids {
        let re = session_entry_regex(&regex::escape(&id.to_lowercase()));
        if !re.is_match(&patched) {
            return Err(format!("Honour Mode is not revoked for {id}"));
        }
        patched = re.replace_all(&patched, "").into_owned();
    }

    // round-trip the patched profile and make sure only the intended entries changed
    let patched_lsx = work.0.join("patched.lsx");
    let patched_lsf = work.0.join("patched.lsf");
    write(&patched_lsx, patched.as_bytes())?;
    divine.convert(&patched_lsx, &patched_lsf)?;
    if sessions_in(&divine.to_lsx(&patched_lsf, &work)?) != sessions_in(&patched) {
        return Err("patched profile failed verification, nothing written".to_string());
    }

    let mut targets = vec![(String::from("local"), paths.profile_file())];
    targets.extend(paths.steam_remotes().into_iter().filter_map(|(label, remote)| {
        let profile = remote.join("_PROFILE_Public").join(PROFILE_FILE);
        profile.is_file().then_some((label, profile))
    }));

    let profile_backup = paths.backup_root.join("profile-backups").join(timestamp());
    for (label, target) in &targets {
        create_dir(&profile_backup.join(label))?;
        fs::copy(target, profile_backup.join(label).join(PROFILE_FILE))
            .map_err(|e| format!("failed to back up {label} profile: {e}"))?;
    }

    let new_profile = fs::read(&patched_lsf).map_err(|e| format!("read patched profile: {e}"))?;
    for (_, target) in &targets {
        write(target, &new_profile)?;
    }
    Ok(ReviveReport {
        profile_backup,
        patched: targets.into_iter().map(|(_, t)| t).collect(),
    })
}

// ---------------------------------------------------------------------------------------------
// Backups
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Backup {
    pub session_id: String,
    /// Folder name: local timestamp plus an optional label.
    pub name: String,
    pub dir: PathBuf,
    pub created: SystemTime,
    pub summary: Option<SaveSummary>,
}

/// Copies a run's save folder into `<backup root>\backups\<session>\<timestamp>[_label]`.
pub fn backup_run(
    divine: Option<&Divine>,
    paths: &Paths,
    session_id: &str,
    label: Option<&str>,
) -> Result<Backup> {
    let src = paths.save_dir(session_id);
    let lsv = newest_lsv(&src).ok_or_else(|| format!("no save found for {session_id}"))?;

    let name = match label {
        Some(l) => format!("{}_{l}", timestamp()),
        None => timestamp(),
    };
    let dir = paths.backups_dir().join(session_id).join(&name);
    copy_files(&src, &dir)?;

    // the summary lets backups be listed without Divine; a backup is still useful without it
    let summary = divine.and_then(|d| SaveSummary::read(d, &lsv).ok());
    if let Some(s) = &summary {
        let json = serde_json::to_vec_pretty(s).map_err(|e| e.to_string())?;
        write(&dir.join(SUMMARY_FILE), &json)?;
    }
    Ok(Backup {
        session_id: session_id.to_string(),
        name,
        dir,
        created: SystemTime::now(),
        summary,
    })
}

/// Backups for one session, or all of them, newest first.
pub fn list_backups(paths: &Paths, session_id: Option<&str>) -> Vec<Backup> {
    let Ok(sessions) = fs::read_dir(paths.backups_dir()) else {
        return vec![];
    };
    let mut backups: Vec<Backup> = sessions
        .flatten()
        .filter(|s| session_id.is_none_or(|id| s.file_name().to_string_lossy() == id))
        .flat_map(|s| {
            let session = s.file_name().to_string_lossy().into_owned();
            fs::read_dir(s.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter(|b| newest_lsv(&b.path()).is_some())
                .map(move |b| Backup {
                    session_id: session.clone(),
                    name: b.file_name().to_string_lossy().into_owned(),
                    created: b.metadata().and_then(|m| m.created()).unwrap_or(SystemTime::UNIX_EPOCH),
                    summary: fs::read(b.path().join(SUMMARY_FILE))
                        .ok()
                        .and_then(|j| serde_json::from_slice(&j).ok()),
                    dir: b.path(),
                })
        })
        .collect();
    backups.sort_by(|a, b| b.name.cmp(&a.name));
    backups
}

pub struct RestoreReport {
    /// Backup of the save that was replaced, if there was one.
    pub safety_backup: Option<Backup>,
    pub written: Vec<PathBuf>,
    pub revived: bool,
}

/// Puts a backup back in place as the run's save (locally and in Steam Cloud), revives Honour
/// Mode if it was revoked, and backs up the save it replaces first.
pub fn restore_backup(divine: &Divine, paths: &Paths, backup: &Backup) -> Result<RestoreReport> {
    ensure_game_closed()?;
    let id = &backup.session_id;
    let save_dir = paths.save_dir(id);

    let safety_backup = match newest_lsv(&save_dir) {
        Some(_) => Some(backup_run(Some(divine), paths, id, Some("before-restore"))?),
        None => None,
    };

    let folder = format!("{id}{HONOUR_SUFFIX}");
    let mut targets = vec![save_dir];
    targets.extend(paths.steam_remotes().into_iter().filter_map(|(_, remote)| {
        let story = remote.join("_SAVE_Public").join("Savegames").join("Story");
        story.is_dir().then(|| story.join(&folder))
    }));
    for target in &targets {
        if target.is_dir() {
            fs::remove_dir_all(target)
                .map_err(|e| format!("failed to clear {}: {e}", target.display()))?;
        }
        copy_files(&backup.dir, target)?;
        let _ = fs::remove_file(target.join(SUMMARY_FILE));
    }

    let revived = failed_sessions(divine, paths)?.contains(id);
    if revived {
        revive(divine, paths, std::slice::from_ref(id))?;
    }
    Ok(RestoreReport {
        safety_backup,
        written: targets,
        revived,
    })
}

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

pub fn game_running() -> bool {
    hidden_command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .map(str::to_lowercase)
                .any(|l| l.starts_with("\"bg3.exe\"") || l.starts_with("\"bg3_dx11.exe\""))
        })
        .unwrap_or(false)
}

fn ensure_game_closed() -> Result<()> {
    if game_running() {
        Err("Baldur's Gate 3 is running; close it first".to_string())
    } else {
        Ok(())
    }
}

/// A Command that doesn't flash a console window when spawned from the GUI.
fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Scratch directory, removed on drop.
struct WorkDir(PathBuf);

impl WorkDir {
    fn new() -> Result<Self> {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("{APP_DIR}_{}_{n}", std::process::id()));
        create_dir(&dir)?;
        Ok(Self(dir))
    }
}

impl Drop for WorkDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_files(src: &Path, dst: &Path) -> Result<()> {
    create_dir(dst)?;
    let entries = fs::read_dir(src).map_err(|e| format!("failed to read {}: {e}", src.display()))?;
    for entry in entries.flatten().filter(|e| e.path().is_file()) {
        fs::copy(entry.path(), dst.join(entry.file_name()))
            .map_err(|e| format!("failed to copy {}: {e}", entry.path().display()))?;
    }
    Ok(())
}

fn create_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::write(path, bytes).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn timestamp() -> String {
    Local::now().format("%Y-%m-%d_%H-%M-%S").to_string()
}

fn modified_time(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| DateTime::<Local>::from(t).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
