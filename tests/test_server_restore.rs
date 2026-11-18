//! Tests de la restauration d'applications depuis les sauvegardes d'un autre serveur.

mod common;
use common::TestEnvironment;

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use backup_cli::actions::server_restore_action::*;
use backup_cli::core::abstract_traits::{CommandExecutor, ExecutionOutput};
use backup_cli::core::exceptions::DomainResult;
use backup_cli::core::model::*;
use backup_cli::providers::filesystem_provider::FilesystemProvider;
use backup_cli::providers::storage_provider::DiskStorageEngine;

#[derive(Debug, Clone)]
struct Call {
    command: String,
    args: Vec<String>,
}

/// Exécuteur de test : « S3 » est un dossier local, restic / docker / clients SQL sont simulés.
#[derive(Clone)]
struct FakeExecutor {
    remote_root: PathBuf,
    calls: Arc<Mutex<Vec<Call>>>,
    stdin_seen: Arc<Mutex<Vec<(Vec<String>, Vec<u8>)>>>,
    container_running: Arc<Mutex<bool>>,
}

impl FakeExecutor {
    fn new(remote_root: PathBuf) -> Self {
        Self {
            remote_root,
            calls: Default::default(),
            stdin_seen: Default::default(),
            container_running: Arc::new(Mutex::new(true)),
        }
    }
    fn local(&self, remote: &str) -> PathBuf {
        self.remote_root.join(remote.split_once(':').map(|(_, p)| p).unwrap_or(remote))
    }
    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    fn find(&self, command: &str, first_arg: &str) -> Vec<Call> {
        self.calls().into_iter().filter(|c| c.command == command && c.args.first().map(|a| a == first_arg).unwrap_or(false)).collect()
    }
}

fn ok(stdout: &str) -> DomainResult<ExecutionOutput> {
    Ok(ExecutionOutput { stdout: stdout.to_string(), stderr: String::new(), exit_code: 0, success: true })
}

fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for e in fs::read_dir(src).unwrap() {
        let e = e.unwrap();
        let t = dst.join(e.file_name());
        if e.path().is_dir() { copy_dir(&e.path(), &t) } else { fs::copy(e.path(), t).unwrap(); }
    }
}

impl CommandExecutor for FakeExecutor {
    fn execute(&self, command: &str, args: &[&str], _envs: &[(&str, &str)]) -> DomainResult<ExecutionOutput> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.calls.lock().unwrap().push(Call { command: command.to_string(), args: args.clone() });
        match command {
            "timeout" => {
                let i = args.iter().position(|a| a == "rclone").unwrap();
                let (sub, src, dst) = (&args[i + 1], &args[i + 2], &args[i + 3]);
                let src_local = self.local(src);
                match sub.as_str() {
                    "copyto" => {
                        fs::create_dir_all(Path::new(dst).parent().unwrap()).unwrap();
                        fs::copy(&src_local, dst).expect("fichier distant absent");
                    }
                    "copy" => copy_dir(&src_local, Path::new(dst)),
                    other => panic!("sous-commande rclone inattendue : {}", other),
                }
                ok("")
            }
            "rclone" if args[0] == "lsf" => {
                let dir = self.local(&args[1]);
                if !dir.exists() {
                    return Ok(ExecutionOutput { stdout: String::new(), stderr: "directory not found".into(), exit_code: 3, success: false });
                }
                let mut names: Vec<String> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok())
                    .filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
                names.sort();
                ok(&names.join("\n"))
            }
            "restic" if args[0] == "snapshots" => ok(
                r#"[{"id":"cccc2222dddd9999","time":"2026-11-17T01:00:00.123456789+00:00"},{"id":"aaaa1111bbbb8888","time":"2026-11-10T01:00:00+00:00"}]"#),
            "restic" if args[0] == "ls" => {
                let root = args[2].trim_end_matches('/').to_string();
                let lines: Vec<String> = ["authentik", "ghost", "extra"].iter()
                    .flat_map(|d| vec![format!(r#"{{"type":"dir","path":"{}/{}"}}"#, root, d), format!(r#"{{"type":"file","path":"{}/{}/.env"}}"#, root, d)])
                    .chain(std::iter::once(format!(r#"{{"type":"dir","path":"{}/ghost/data"}}"#, root)))
                    .collect();
                ok(&lines.join("\n"))
            }
            _ => ok(""),
        }
    }
    fn execute_to_file(&self, _c: &str, _a: &[&str], _e: &[(&str, &str)], _o: &Path) -> DomainResult<u64> { Ok(0) }
    fn execute_with_stdin(&self, command: &str, args: &[&str], _e: &[(&str, &str)], stdin: &Path) -> DomainResult<ExecutionOutput> {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        self.calls.lock().unwrap().push(Call { command: command.to_string(), args: args.clone() });
        self.stdin_seen.lock().unwrap().push((args, fs::read(stdin)?));
        ok("")
    }
    fn is_container_running(&self, _c: &str) -> DomainResult<bool> { Ok(*self.container_running.lock().unwrap()) }
}

fn gz(data: &str) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data.as_bytes()).unwrap();
    e.finish().unwrap()
}

fn put(root: &Path, dir: &str, name: &str, bytes: &[u8]) {
    let d = root.join(dir);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join(name), bytes).unwrap();
}

struct Fixture {
    env: TestEnvironment,
    exec: FakeExecutor,
    config: AppConfig,
    apps_root: PathBuf,
    uploads: PathBuf,
}

fn restic_profile() -> BackupProfile {
    BackupProfile {
        name: "configs".into(), enabled: true, profile_type: ProfileType::Restic, schedule: None,
        source: SourceConfig { restic_repository: Some("/backup-configuration".into()), ..Default::default() },
        destination: DestinationConfig { remote: Some("s3backup:backups/s001/backup-configuration".into()), ..Default::default() },
        compression: None, retention: None, validation: None, application: None,
    }
}

fn fixture() -> Fixture {
    let env = TestEnvironment::new();
    let remote_root = env.temp_dir.path().join("fake-s3");
    let apps_root = env.temp_dir.path().join("opt");
    let uploads = env.temp_dir.path().join("ghost-content");
    fs::create_dir_all(&apps_root).unwrap();

    // Archive d'uploads du serveur s002 (chemins absolus), puis suppression de la source.
    fs::create_dir_all(uploads.join("images")).unwrap();
    fs::write(uploads.join("images/logo.png"), b"PNG-DATA").unwrap();
    let archive = env.temp_dir.path().join("2026-11-17_02-00-00_ghost-files_backup.tar.gz");
    FilesystemProvider::new().create_archive(&[uploads.clone()], &archive, &[],
        Some(&CompressionConfig { enabled: true, algorithm: CompressionAlgorithm::Gzip, level: None })).unwrap();
    fs::remove_dir_all(&uploads).unwrap();

    let data = "backups/s002/backup";
    put(&remote_root, data, "2026-11-10_01-00-00_authentik_backup.dump", b"PGDMP-old");
    put(&remote_root, data, "2026-11-17_01-00-00_authentik_backup.dump", b"PGDMP-new");
    put(&remote_root, data, "2026-11-17_01-00-00_ghost_backup.sql.gz", &gz("CREATE TABLE posts (id INT);"));
    put(&remote_root, data, "2026-11-17_02-00-00_ghost-files_backup.tar.gz", &fs::read(&archive).unwrap());
    put(&remote_root, data, "manifest.json", b"[]");
    put(&remote_root, data, "2026-11-17_01-00-00_authentik_backup.dump.sha256", b"abc");
    put(&remote_root, "backups/s002/backup-configuration", "config", b"restic-config");

    let mut config = env.build_sample_config();
    config.settings.applications_root = apps_root.clone();
    config.settings.storage = Some(RemoteStorageConfig {
        rclone_config: None,
        remote: "s3backup:backups/s001/backup".into(),
        config_remote: Some("s3backup:backups/s001/backup-configuration".into()),
        upload_max_duration: "6h".into(),
        config_max_duration: "12h".into(),
        extra_args: None,
        server_name: None,
    });
    let mut ghost_files = config.profiles[1].clone();
    ghost_files.name = "ghost-files".into();
    ghost_files.profile_type = ProfileType::Filesystem;
    ghost_files.application = Some("ghost".into());
    ghost_files.source = SourceConfig { paths: Some(vec![uploads.clone()]), ..Default::default() };
    config.profiles.truncate(2);
    config.profiles.push(ghost_files);
    config.profiles.push(restic_profile());

    let exec = FakeExecutor::new(remote_root);
    Fixture { env, exec, config, apps_root, uploads }
}

fn opts(app: &str) -> ServerRestoreOptions {
    ServerRestoreOptions {
        server: "s002".into(), application: app.into(), before: None, dry_run: false, force: false,
        start: false, include_system: false, work_dir: None, keep_downloads: false,
    }
}

fn run(fx: &Fixture, o: &ServerRestoreOptions) -> ServerRestoreReport {
    let storage = DiskStorageEngine::new(&fx.exec);
    ServerRestoreAction::new(&fx.exec, &storage).execute(&fx.config, o).expect("restauration")
}

fn step<'a>(r: &'a ServerRestoreReport, app: &str, label_part: &str) -> &'a StepReport {
    r.steps.iter().find(|s| s.application == app && s.label.contains(label_part))
        .unwrap_or_else(|| panic!("étape '{}' de '{}' absente : {:#?}", label_part, app, r.steps))
}

#[test]
fn test_server_names_and_remote_mapping() {
    let fx = fixture();
    assert_eq!(fx.config.current_server_name().as_deref(), Some("s001"));
    assert_eq!(fx.config.remote_for_server("s3backup:backups/s001/backup", "s002").unwrap(), "s3backup:backups/s002/backup");
    assert_eq!(fx.config.remote_for_server("s3backup:backups/s001/backup-configuration", "s003").unwrap(), "s3backup:backups/s003/backup-configuration");
    assert!(fx.config.remote_for_server("s3backup:other/place", "s002").is_err());

    let mut c = fx.config.clone();
    c.settings.storage.as_mut().unwrap().server_name = Some("prod-a".into());
    assert!(c.remote_for_server("s3backup:backups/s001/backup", "s002").is_err(), "le nom explicite remplace la déduction");
}

#[test]
fn test_applications_grouping_uses_application_tag() {
    let fx = fixture();
    let apps = fx.config.applications();
    assert_eq!(apps.keys().cloned().collect::<Vec<_>>(), vec!["authentik", "ghost"]);
    assert_eq!(apps["ghost"].len(), 2); // base mysql + fichiers uploadés
    assert_eq!(fx.config.config_profiles().len(), 1);
}

#[test]
fn test_parse_before_formats() {
    assert_eq!(parse_before("2026-11-12").unwrap().to_rfc3339(), "2026-11-12T23:59:59+00:00");
    assert_eq!(parse_before("2026-11-12_08-30-00").unwrap().to_rfc3339(), "2026-11-12T08:30:00+00:00");
    assert!(parse_before("2026-11-12T08:30:00+03:00").is_ok());
    assert!(parse_before("hier").is_err());
}

#[test]
fn test_list_shows_latest_backup_per_profile() {
    let fx = fixture();
    let storage = DiskStorageEngine::new(&fx.exec);
    let listing = ServerRestoreAction::new(&fx.exec, &storage).list(&fx.config, "s002").unwrap();
    let text = listing.render();
    assert!(text.contains("2026-11-17_01-00-00_authentik_backup.dump"), "{}", text);
    assert!(!text.contains("2026-11-10"), "seul le dernier doit être listé : {}", text);
    // 'ghost' ne doit pas confondre ses archives avec celles de 'ghost-files'
    assert!(text.contains("ghost (mysql) — dernière : 2026-11-17_01-00-00_ghost_backup.sql.gz"), "{}", text);
    assert!(text.contains("ghost-files (filesystem) — dernière : 2026-11-17_02-00-00_ghost-files_backup.tar.gz"), "{}", text);
}

#[test]
fn test_restore_single_application_files_uploads_and_database() {
    let fx = fixture();
    let report = run(&fx, &opts("ghost"));
    assert_eq!(report.failed(), 0, "{}", report.render());

    // uploads restaurés à leur emplacement d'origine
    assert_eq!(fs::read(fx.uploads.join("images/logo.png")).unwrap(), b"PNG-DATA");

    // restic : snapshot le plus récent, cible /, uniquement l'application demandée
    let restic = fx.exec.find("restic", "restore");
    assert_eq!(restic.len(), 1);
    let a = &restic[0].args;
    assert_eq!(a[1], "latest");
    assert!(a.windows(2).any(|w| w[0] == "--target" && w[1] == "/"));
    let includes: Vec<&String> = a.windows(2).filter(|w| w[0] == "--include").map(|w| &w[1]).collect();
    assert_eq!(includes, vec![&fx.apps_root.join("ghost").display().to_string()]);

    // base MySQL : le client reçoit du SQL décompressé
    let seen = fx.exec.stdin_seen.lock().unwrap();
    assert_eq!(seen.len(), 1);
    assert_eq!(String::from_utf8(seen[0].1.clone()).unwrap(), "CREATE TABLE posts (id INT);");
    assert!(seen[0].0.contains(&"mysql".to_string()));
    // rien d'autre n'est touché : l'autre application n'est pas restaurée
    assert!(report.steps.iter().all(|s| s.application == "ghost" || s.application == "(configuration)"));
    // le dossier de téléchargement est nettoyé
    assert!(!fx.env.backup_dir.join(".restore-s002").exists());
}

#[test]
fn test_restore_all_includes_every_known_and_discovered_application() {
    let fx = fixture();
    let report = run(&fx, &opts("all"));
    assert_eq!(report.failed(), 0, "{}", report.render());

    let a = &fx.exec.find("restic", "restore")[0].args;
    let includes: Vec<String> = a.windows(2).filter(|w| w[0] == "--include").map(|w| w[1].clone()).collect();
    for app in ["authentik", "ghost", "extra"] {
        assert!(includes.contains(&fx.apps_root.join(app).display().to_string()), "{} manquant : {:?}", app, includes);
    }
    assert_eq!(includes.len(), 3, "pas de restauration système implicite");
    step(&report, "authentik", "base 'authentik' (postgres)");
    step(&report, "ghost", "base 'ghost' (mysql)");
}

#[test]
fn test_existing_application_is_protected_without_force() {
    let fx = fixture();
    fs::create_dir_all(fx.apps_root.join("ghost")).unwrap();
    fs::write(fx.apps_root.join("ghost/.env"), b"LOCAL=1").unwrap();

    let report = run(&fx, &opts("ghost"));
    assert_eq!(step(&report, "ghost", "protection").status, StepStatus::Failed);
    assert!(step(&report, "ghost", "protection").detail.contains("--force"));
    assert!(fx.exec.find("restic", "restore").is_empty(), "aucune restauration sans --force");
    assert!(fx.exec.stdin_seen.lock().unwrap().is_empty());
    assert!(!fx.uploads.exists());

    let mut forced = opts("ghost");
    forced.force = true;
    let report = run(&fx, &forced);
    assert_eq!(report.failed(), 0, "{}", report.render());
    assert_eq!(fx.exec.find("restic", "restore").len(), 1);
}

#[test]
fn test_database_step_fails_with_hint_when_container_is_stopped_and_start_fixes_it() {
    let fx = fixture();
    *fx.exec.container_running.lock().unwrap() = false;
    let report = run(&fx, &opts("authentik"));
    let db = step(&report, "authentik", "base 'authentik'");
    assert_eq!(db.status, StepStatus::Failed);
    assert!(db.detail.contains("docker compose --project-directory") && db.detail.contains("--start"), "{}", db.detail);
    assert!(fx.exec.stdin_seen.lock().unwrap().is_empty());

    // avec --start : docker compose up -d est lancé (l'attente de conteneur est simulée par un faux « running »)
    *fx.exec.container_running.lock().unwrap() = true;
    let mut o = opts("authentik");
    o.start = true;
    let report = run(&fx, &o);
    assert_eq!(report.failed(), 0, "{}", report.render());
    let up = fx.exec.find("docker", "compose");
    assert_eq!(up.len(), 1);
    assert_eq!(up[0].args[1..], ["--project-directory", &fx.apps_root.join("authentik").display().to_string(), "up", "-d"]);
    assert_eq!(fx.exec.stdin_seen.lock().unwrap()[0].1, b"PGDMP-new");
}

#[test]
fn test_before_selects_older_dump_and_older_snapshot() {
    let fx = fixture();
    let mut o = opts("authentik");
    o.before = Some(parse_before("2026-11-12").unwrap());
    let report = run(&fx, &o);
    assert_eq!(report.failed(), 0, "{}", report.render());
    assert_eq!(fx.exec.stdin_seen.lock().unwrap()[0].1, b"PGDMP-old");
    assert_eq!(fx.exec.find("restic", "restore")[0].args[1], "aaaa1111bbbb8888");

    let mut too_old = opts("authentik");
    too_old.before = Some(parse_before("2020-01-01").unwrap());
    let storage = DiskStorageEngine::new(&fx.exec);
    let report = ServerRestoreAction::new(&fx.exec, &storage).execute(&fx.config, &too_old).unwrap();
    assert!(report.failed() > 0);
}

#[test]
fn test_dry_run_downloads_and_modifies_nothing() {
    let fx = fixture();
    let mut o = opts("all");
    o.dry_run = true;
    let report = run(&fx, &o);
    assert_eq!(report.failed(), 0, "{}", report.render());
    assert!(report.steps.iter().all(|s| s.status == StepStatus::Planned), "{}", report.render());
    assert!(fx.exec.find("timeout", "-k").is_empty(), "aucun téléchargement en simulation");
    assert!(fx.exec.find("restic", "restore").is_empty());
    assert!(fx.exec.stdin_seen.lock().unwrap().is_empty());
    assert!(!fx.uploads.exists());
    assert!(report.render().contains("Simulation"));
}

#[test]
fn test_unknown_application_is_reported() {
    let fx = fixture();
    let report = run(&fx, &opts("inconnue"));
    let s = step(&report, "inconnue", "application");
    assert_eq!(s.status, StepStatus::Failed);
    assert!(s.detail.contains("introuvable"), "{}", s.detail);
}

#[test]
fn test_config_only_application_found_in_snapshot_is_restored() {
    let fx = fixture();
    let report = run(&fx, &opts("extra")); // pas de profil, mais dossier présent dans le snapshot
    assert_eq!(report.failed(), 0, "{}", report.render());
    let a = &fx.exec.find("restic", "restore")[0].args;
    assert!(a.contains(&fx.apps_root.join("extra").display().to_string()));
}

#[test]
fn test_include_system_requires_all_and_force() {
    let fx = fixture();
    let storage = DiskStorageEngine::new(&fx.exec);
    let action = ServerRestoreAction::new(&fx.exec, &storage);

    let mut o = opts("ghost");
    o.include_system = true;
    assert!(action.execute(&fx.config, &o).is_err(), "--include-system sans 'all'");

    let mut o = opts("all");
    o.include_system = true;
    assert!(action.execute(&fx.config, &o).is_err(), "--include-system sans --force");

    o.force = true;
    let report = action.execute(&fx.config, &o).unwrap();
    assert_eq!(report.failed(), 0, "{}", report.render());
    let a = &fx.exec.find("restic", "restore")[0].args;
    assert!(!a.contains(&"--include".to_string()), "snapshot complet attendu : {:?}", a);
}

#[test]
fn test_missing_remote_storage_config_is_an_error() {
    let mut fx = fixture();
    fx.config.settings.storage = None;
    let storage = DiskStorageEngine::new(&fx.exec);
    assert!(ServerRestoreAction::new(&fx.exec, &storage).execute(&fx.config, &opts("ghost")).is_err());
}

#[test]
fn test_download_is_verified_with_rclone_check() {
    let fx = fixture();
    run(&fx, &opts("authentik"));
    let checks = fx.exec.find("rclone", "check");
    assert!(checks.iter().any(|c| c.args.contains(&"--include".to_string())), "vérification du fichier : {:?}", checks);
    assert!(checks.iter().any(|c| !c.args.contains(&"--include".to_string())), "vérification du dépôt restic : {:?}", checks);
    assert!(checks.iter().all(|c| c.args.contains(&"--one-way".to_string())));
}

#[test]
fn test_tar_entries_with_parent_dir_are_rejected() {
    let env = TestEnvironment::new();
    let archive = env.temp_dir.path().join("evil.tar");
    {
        let mut builder = tar::Builder::new(fs::File::create(&archive).unwrap());
        let mut header = tar::Header::new_gnu();
        let name = b"../evil.txt";
        header.as_gnu_mut().unwrap().name[..name.len()].copy_from_slice(name);
        header.set_size(4);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append(&header, &b"evil"[..]).unwrap();
        builder.finish().unwrap();
    }
    let target = env.temp_dir.path().join("target");
    let err = FilesystemProvider::new().extract_archive(&archive, &target, CompressionAlgorithm::None, None);
    assert!(err.is_err(), "l'entrée '../evil.txt' doit être refusée");
    assert!(!env.temp_dir.path().join("evil.txt").exists());
}

#[test]
fn test_cli_parsing_of_restore_forms() {
    use backup_cli::cli::args::{Cli, Commands, RestoreSubcommands};
    use clap::Parser;

    let cli = Cli::try_parse_from(["backup-cli", "restore", "--server", "s001", "--application", "authentik", "--before", "2026-11-12", "--start", "--force"]).unwrap();
    let Commands::Restore(g) = cli.command else { panic!() };
    assert!(g.command.is_none());
    assert_eq!((g.server.as_deref(), g.application.as_deref(), g.start, g.force), (Some("s001"), Some("authentik"), true, true));
    assert_eq!(g.before.as_deref(), Some("2026-11-12"));

    let cli = Cli::try_parse_from(["backup-cli", "restore", "--server", "s001", "--app", "all", "--dry-run"]).unwrap();
    let Commands::Restore(g) = cli.command else { panic!() };
    assert_eq!((g.application.as_deref(), g.dry_run), (Some("all"), true));

    let cli = Cli::try_parse_from(["backup-cli", "restore", "list", "--server", "s002"]).unwrap();
    let Commands::Restore(g) = cli.command else { panic!() };
    assert!(matches!(g.command, Some(RestoreSubcommands::List { ref server }) if server == "s002"));

    let cli = Cli::try_parse_from(["backup-cli", "restore", "run", "2026-11-17_01-00-00_x_backup"]).unwrap();
    let Commands::Restore(g) = cli.command else { panic!() };
    assert!(matches!(g.command, Some(RestoreSubcommands::Run { .. })));
}

#[test]
fn test_unknown_server_fails_fast_with_clear_message() {
    let fx = fixture();
    let storage = DiskStorageEngine::new(&fx.exec);
    let mut o = opts("ghost");
    o.server = "s999".into();
    let err = ServerRestoreAction::new(&fx.exec, &storage).execute(&fx.config, &o).unwrap_err().to_string();
    assert!(err.contains("s999") && err.contains("introuvable"), "{}", err);
    assert!(fx.exec.find("timeout", "-k").is_empty(), "aucun téléchargement tenté");
}

#[test]
fn test_rclone_config_path_is_passed_to_every_rclone_call() {
    let mut fx = fixture();
    {
        let st = fx.config.settings.storage.as_mut().unwrap();
        st.rclone_config = Some("/root/.config/rclone/rclone.conf".into());
        st.extra_args = Some(vec!["--bwlimit".into(), "5M".into()]);
        assert_eq!(st.rclone_args(), vec!["--config", "/root/.config/rclone/rclone.conf", "--bwlimit", "5M"]);
    }
    run(&fx, &opts("authentik"));
    let rclone_calls: Vec<Call> = fx.exec.calls().into_iter()
        .filter(|c| c.command == "rclone" || (c.command == "timeout" && c.args.contains(&"rclone".to_string()))).collect();
    assert!(!rclone_calls.is_empty());
    for c in rclone_calls {
        assert!(c.args.windows(2).any(|w| w[0] == "--config" && w[1] == "/root/.config/rclone/rclone.conf"),
            "--config manquant dans : {:?}", c);
    }
}
