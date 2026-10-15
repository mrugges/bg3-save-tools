use bg3_save_tools::{
    Backup, Divine, HonourRun, Paths, SaveSummary, backup_run, honour_runs, list_backups,
    resolve_session, restore_backup, revive,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "bg3-save-tools", about = "Baldur's Gate 3 Honour Mode save tools")]
struct Cli {
    /// Path to LSLib's Divine.exe (https://github.com/Norbyte/lslib/releases)
    #[arg(long, env = "DIVINE_EXE", global = true)]
    divine: Option<PathBuf>,

    /// Player profile directory (defaults to %LOCALAPPDATA%\Larian Studios\Baldur's Gate 3\PlayerProfiles\Public)
    #[arg(long, global = true)]
    profile_dir: Option<PathBuf>,

    /// Steam installation directory, used to also update Steam Cloud copies (detected by default)
    #[arg(long, global = true)]
    steam_dir: Option<PathBuf>,

    /// Where backups are kept (defaults to Documents\BG3 Save Tools)
    #[arg(long, global = true)]
    backup_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// List Honour Mode runs and whether Honour Mode was revoked
    List {
        /// Also show deleted runs that only remain in the profile
        #[arg(long)]
        all: bool,
    },
    /// Revive Honour Mode for runs that continued in Custom Mode after a party wipe
    Revive {
        /// Session IDs (or unique prefixes)
        #[arg(required = true)]
        session_ids: Vec<String>,
    },
    /// Back up Honour runs' saves
    Backup {
        /// Session IDs (or unique prefixes)
        #[arg(required_unless_present = "all")]
        session_ids: Vec<String>,
        /// Back up every run that has a save
        #[arg(long, conflicts_with = "session_ids")]
        all: bool,
    },
    /// List backups
    Backups {
        /// Only show backups for this session (or unique prefix)
        session_id: Option<String>,
    },
    /// Restore a run's save from a backup (the newest one unless --backup is given)
    Restore {
        session_id: String,
        /// Backup folder name, as shown by `backups`
        #[arg(long)]
        backup: Option<String>,
    },
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), String> {
    let mut paths = Paths::detect()?;
    if let Some(p) = cli.profile_dir {
        paths.profile_dir = p;
    }
    if let Some(s) = cli.steam_dir {
        paths.steam_dir = Some(s);
    }
    if let Some(b) = cli.backup_dir {
        paths.backup_root = b;
    }
    let divine = Divine::locate(cli.divine)?;

    match cli.command {
        Commands::List { all } => {
            for run in honour_runs(&divine, &paths)? {
                if all || run.lsv.is_some() || !run.backups.is_empty() {
                    print_run(&run);
                }
            }
        }
        Commands::Revive { session_ids } => {
            let runs = honour_runs(&divine, &paths)?;
            let ids = session_ids
                .iter()
                .map(|id| resolve_session(&runs, id).map(|r| r.session_id.clone()))
                .collect::<Result<Vec<_>, _>>()?;
            let report = revive(&divine, &paths, &ids)?;
            for id in &ids {
                println!("revived Honour Mode for {id}");
            }
            println!("profile backed up to {}", report.profile_backup.display());
            for p in &report.patched {
                println!("patched {}", p.display());
            }
            println!("if Steam reports a cloud sync conflict on launch, keep the local files");
        }
        Commands::Backup { session_ids, all } => {
            let runs = honour_runs(&divine, &paths)?;
            let targets: Vec<&HonourRun> = if all {
                runs.iter().filter(|r| r.lsv.is_some()).collect()
            } else {
                session_ids
                    .iter()
                    .map(|id| resolve_session(&runs, id))
                    .collect::<Result<_, _>>()?
            };
            for run in targets {
                let backup = backup_run(Some(&divine), &paths, &run.session_id, None)?;
                println!("backed up {} to {}", run.session_id, backup.dir.display());
            }
        }
        Commands::Backups { session_id } => {
            let session = match session_id {
                Some(id) => Some(resolve_session(&honour_runs(&divine, &paths)?, &id)?.session_id.clone()),
                None => None,
            };
            let backups = list_backups(&paths, session.as_deref());
            if backups.is_empty() {
                println!("no backups in {}", paths.backups_dir().display());
            }
            for b in &backups {
                print_backup(b);
            }
        }
        Commands::Restore { session_id, backup } => {
            let runs = honour_runs(&divine, &paths)?;
            let run = resolve_session(&runs, &session_id)?;
            let chosen = match &backup {
                Some(name) => run.backups.iter().find(|b| &b.name == name),
                None => run.backups.first(),
            }
            .ok_or_else(|| format!("no matching backup for {}", run.session_id))?;

            let report = restore_backup(&divine, &paths, chosen)?;
            if let Some(safety) = &report.safety_backup {
                println!("previous save backed up to {}", safety.dir.display());
            }
            for p in &report.written {
                println!("restored {} to {}", chosen.name, p.display());
            }
            if report.revived {
                println!("revived Honour Mode for {}", run.session_id);
            }
        }
    }
    Ok(())
}

fn print_run(run: &HonourRun) {
    let status = if run.failed { "REVOKED (Custom Mode)" } else { "Honour Mode" };
    let backups = match run.backups.len() {
        0 => String::new(),
        n => format!(", {n} backup(s)"),
    };
    println!("{}  [{status}{backups}]", run.session_id);
    match (run.best_summary(), &run.summary_error, &run.lsv) {
        (Some(s), _, lsv) => {
            if lsv.is_none() {
                println!("    save deleted; newest backup:");
            }
            print_summary(s);
        }
        (None, Some(e), _) => println!("    failed to read save: {e}"),
        (None, None, _) => println!("    save deleted"),
    }
    println!();
}

fn print_summary(s: &SaveSummary) {
    println!(
        "    {} ({}), {} played, leader {}, last saved {}",
        s.act(),
        s.level,
        s.playtime(),
        s.leader,
        s.saved
    );
    for m in s.members() {
        println!("    Lv{:<2} {:<12} {:<16} {}", m.level, m.name(), m.race, m.classes_text());
    }
    let summons: Vec<_> = s.summons().map(|m| m.race.as_str()).collect();
    if !summons.is_empty() {
        println!("    summons: {}", summons.join(", "));
    }
}

fn print_backup(b: &Backup) {
    match &b.summary {
        Some(s) => println!(
            "{}  {}  {} ({}), {} played, leader {}",
            b.session_id,
            b.name,
            s.act(),
            s.level,
            s.playtime(),
            s.leader
        ),
        None => println!("{}  {}", b.session_id, b.name),
    }
}
