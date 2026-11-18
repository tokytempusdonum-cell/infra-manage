//! Restauration d'applications depuis les sauvegardes d'un autre serveur (ou du même),
//! stockées sur S3, vers la machine courante.
//!
//! Cas d'usage : un serveur (ex: `s001`) tombe ; sur une machine de remplacement on lance
//! `infra-manage restore --server s001 --application authentik` (ou `all`).
//!
//! Pour chaque application :
//! 1. configuration (`docker-compose`, `.env`...) depuis le dépôt restic du serveur d'origine ;
//! 2. fichiers uploadés (profils `filesystem` de l'application) ;
//! 3. démarrage optionnel des conteneurs (`--start`) ;
//! 4. bases de données (postgres, mysql, mariadb, mongo) depuis le dernier dump.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};
use tracing::{info, warn};

use crate::actions::history_action::HistoryAction;
use crate::core::abstract_traits::{CommandExecutor, StorageEngine};
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{AppConfig, BackupProfile, ProfileType, RemoteStorageConfig};
use crate::providers::database_provider::DatabaseProvider;
use crate::providers::filesystem_provider::FilesystemProvider;
use crate::providers::restic_provider::ResticProvider;
use crate::providers::stream_compression::detect_compression;

/// Format d'horodatage des sauvegardes dans les noms de fichiers.
const TIMESTAMP_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";
/// Longueur du préfixe d'horodatage (`2026-11-17_01-00-02`).
const TIMESTAMP_LEN: usize = 19;
/// Attente maximale du démarrage d'un conteneur après `--start`.
const CONTAINER_WAIT_SECS: u64 = 90;

/// Options de la restauration depuis un serveur.
#[derive(Debug, Clone)]
pub struct ServerRestoreOptions {
    /// Serveur d'origine des sauvegardes (ex: `s001`).
    pub server: String,
    /// Application(s) : un nom, une liste séparée par des virgules, ou `all`.
    pub application: String,
    /// Restaure l'état le plus récent antérieur à cette date (sinon : le plus récent).
    pub before: Option<DateTime<Utc>>,
    /// Simulation : n'écrit rien, ne télécharge rien.
    pub dry_run: bool,
    /// Autorise l'écrasement d'une application déjà présente sur cette machine.
    pub force: bool,
    /// Lance `docker compose up -d` avant la restauration des bases.
    pub start: bool,
    /// Avec `all` : restaure aussi les fichiers système du snapshot (/etc, crontab...).
    pub include_system: bool,
    /// Dossier de téléchargement temporaire (défaut : `<backup_directory>/.restore-<serveur>`).
    pub work_dir: Option<PathBuf>,
    /// Conserve les fichiers téléchargés après la restauration.
    pub keep_downloads: bool,
}

/// État d'une étape de restauration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Done,
    Skipped,
    Failed,
    Planned,
}

/// Résultat d'une étape pour une application.
#[derive(Debug, Clone)]
pub struct StepReport {
    pub application: String,
    pub label: String,
    pub status: StepStatus,
    pub detail: String,
}

/// Bilan d'une restauration depuis un serveur.
#[derive(Debug, Clone, Default)]
pub struct ServerRestoreReport {
    pub server: String,
    pub dry_run: bool,
    pub steps: Vec<StepReport>,
}

impl ServerRestoreReport {
    fn push(&mut self, app: &str, label: &str, status: StepStatus, detail: impl Into<String>) {
        self.steps.push(StepReport {
            application: app.to_string(),
            label: label.to_string(),
            status,
            detail: detail.into(),
        });
    }

    fn count(&self, status: StepStatus) -> usize {
        self.steps.iter().filter(|s| s.status == status).count()
    }

    /// Nombre d'étapes en échec.
    pub fn failed(&self) -> usize {
        self.count(StepStatus::Failed)
    }

    /// Nombre d'étapes réussies.
    pub fn done(&self) -> usize {
        self.count(StepStatus::Done)
    }

    /// Texte lisible du bilan, groupé par application.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let title = if self.dry_run {
            format!("Simulation de restauration depuis le serveur '{}' (rien n'est modifié)", self.server)
        } else {
            format!("Restauration depuis le serveur '{}' vers cette machine", self.server)
        };
        out.push_str(&title);
        out.push('\n');

        let mut by_app: BTreeMap<&str, Vec<&StepReport>> = BTreeMap::new();
        for s in &self.steps {
            by_app.entry(s.application.as_str()).or_default().push(s);
        }
        for (app, steps) in by_app {
            out.push_str(&format!("\n{}\n", app));
            for s in steps {
                let icon = match s.status {
                    StepStatus::Done => "✅",
                    StepStatus::Failed => "❌",
                    StepStatus::Skipped => "⏭️ ",
                    StepStatus::Planned => "📝",
                };
                out.push_str(&format!("  {} {} — {}\n", icon, s.label, s.detail));
            }
        }
        out.push_str(&format!(
            "\nRésumé : {} étape(s) — {} réussie(s), {} échec(s), {} ignorée(s), {} prévue(s)\n",
            self.steps.len(),
            self.done(),
            self.failed(),
            self.count(StepStatus::Skipped),
            self.count(StepStatus::Planned)
        ));
        out
    }
}

/// Ligne de listing : dernier artefact disponible pour un profil.
#[derive(Debug, Clone)]
pub struct ListItem {
    pub profile: String,
    pub profile_type: ProfileType,
    pub latest: Option<String>,
}

/// Contenu restaurable d'un serveur, par application.
#[derive(Debug, Clone, Default)]
pub struct ServerListing {
    pub server: String,
    pub applications: BTreeMap<String, Vec<ListItem>>,
    pub config_profiles: Vec<String>,
}

impl ServerListing {
    /// Texte lisible du listing.
    pub fn render(&self) -> String {
        let mut out = format!("Sauvegardes disponibles pour le serveur '{}'\n", self.server);
        for (app, items) in &self.applications {
            out.push_str(&format!("\n{}\n", app));
            for it in items {
                match &it.latest {
                    Some(f) => out.push_str(&format!("  ✅ {} ({}) — dernière : {}\n", it.profile, it.profile_type.label(), f)),
                    None => out.push_str(&format!("  ❌ {} ({}) — aucune sauvegarde trouvée\n", it.profile, it.profile_type.label())),
                }
            }
        }
        if !self.config_profiles.is_empty() {
            out.push_str(&format!(
                "\nConfiguration (dépôt restic) : {}\n",
                self.config_profiles.join(", ")
            ));
        }
        out
    }
}

/// Interprète une date `--before` : `YYYY-MM-DD` (fin de journée), `YYYY-MM-DD_HH-MM-SS`
/// ou RFC 3339. Toutes en UTC, comme l'horodatage des sauvegardes.
pub fn parse_before(value: &str) -> DomainResult<DateTime<Utc>> {
    let v = value.trim();
    if let Ok(d) = NaiveDate::parse_from_str(v, "%Y-%m-%d") {
        let dt = d.and_hms_opt(23, 59, 59).expect("heure valide");
        return Ok(Utc.from_utc_datetime(&dt));
    }
    if let Ok(dt) = NaiveDateTime::parse_from_str(v, TIMESTAMP_FORMAT) {
        return Ok(Utc.from_utc_datetime(&dt));
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(v) {
        return Ok(dt.with_timezone(&Utc));
    }
    Err(DomainError::Config(format!(
        "Date '{}' invalide : formats acceptés YYYY-MM-DD, YYYY-MM-DD_HH-MM-SS ou RFC 3339 (UTC)",
        value
    )))
}

/// Extensions d'artefact attendues selon le type de profil.
fn artifact_extensions(kind: ProfileType) -> &'static [&'static str] {
    match kind {
        ProfileType::Postgres => &["dump"],
        ProfileType::Mysql | ProfileType::Mariadb => &["sql", "sql.gz", "sql.zst"],
        ProfileType::Mongo => &["archive", "archive.gz"],
        ProfileType::Filesystem => &["tar", "tar.gz", "tar.zst"],
        ProfileType::Restic => &[],
    }
}

/// Date d'une sauvegarde si `file_name` est un artefact de ce profil (`<horodatage>_<profil>_backup.<ext>`).
fn artifact_timestamp(file_name: &str, profile: &BackupProfile) -> Option<NaiveDateTime> {
    if file_name.len() <= TIMESTAMP_LEN + 1 || !file_name.is_char_boundary(TIMESTAMP_LEN) {
        return None;
    }
    let (stamp, rest) = file_name.split_at(TIMESTAMP_LEN);
    let ts = NaiveDateTime::parse_from_str(stamp, TIMESTAMP_FORMAT).ok()?;
    let rest = rest.strip_prefix('_')?;
    let rest = rest.strip_prefix(profile.name.as_str())?;
    let ext = rest.strip_prefix("_backup.")?;
    artifact_extensions(profile.profile_type)
        .contains(&ext)
        .then_some(ts)
}

/// Dernier artefact d'un profil parmi `files`, éventuellement antérieur à `before`.
fn pick_latest(files: &[String], profile: &BackupProfile, before: Option<DateTime<Utc>>) -> Option<String> {
    files
        .iter()
        .filter_map(|f| artifact_timestamp(f, profile).map(|ts| (ts, f)))
        .filter(|(ts, _)| before.map(|b| Utc.from_utc_datetime(ts) <= b).unwrap_or(true))
        .max_by_key(|(ts, _)| *ts)
        .map(|(_, f)| f.clone())
}

/// Vrai si le chemin existe et n'est pas un dossier vide.
fn exists_non_empty(path: &Path) -> bool {
    match fs::metadata(path) {
        Ok(m) if m.is_dir() => fs::read_dir(path).map(|mut d| d.next().is_some()).unwrap_or(false),
        Ok(_) => true,
        Err(_) => false,
    }
}

fn is_database(kind: ProfileType) -> bool {
    matches!(
        kind,
        ProfileType::Postgres | ProfileType::Mysql | ProfileType::Mariadb | ProfileType::Mongo
    )
}

/// Source de configuration (dépôt restic téléchargé) prête à être restaurée.
struct ConfigSource {
    profile: BackupProfile,
    snapshot: String,
    discovered: Vec<String>,
}

/// Action de restauration d'applications depuis les sauvegardes d'un serveur.
pub struct ServerRestoreAction<'a> {
    executor: &'a dyn CommandExecutor,
    storage: &'a dyn StorageEngine,
}

impl<'a> ServerRestoreAction<'a> {
    /// Initialise l'action avec l'exécuteur et le moteur de stockage injectés.
    pub fn new(executor: &'a dyn CommandExecutor, storage: &'a dyn StorageEngine) -> Self {
        Self { executor, storage }
    }

    fn storage_cfg<'c>(&self, config: &'c AppConfig) -> DomainResult<&'c RemoteStorageConfig> {
        config.settings.storage.as_ref().ok_or_else(|| {
            DomainError::Config(
                "Stockage distant non configuré ('settings.storage') : restauration depuis S3 impossible".to_string(),
            )
        })
    }

    /// Dossier distant d'un profil, transposé vers le serveur demandé.
    fn remote_dir_for(&self, config: &AppConfig, profile: &BackupProfile, server: &str) -> DomainResult<String> {
        let remote = config.remote_of_profile(profile).ok_or_else(|| {
            DomainError::Config(format!(
                "Profil '{}' : aucune cible distante (ni 'destination.remote' ni 'settings.storage.remote')",
                profile.name
            ))
        })?;
        config.remote_for_server(&remote, server)
    }

    /// Dossier distant global (`settings.storage.remote`) transposé vers le serveur demandé.
    fn remote_dir_for_global(&self, config: &AppConfig, server: &str) -> DomainResult<String> {
        let cfg = self.storage_cfg(config)?;
        config.remote_for_server(&cfg.remote, server)
    }

    /// Liste ce qui est restaurable pour un serveur (dernier artefact par profil).
    pub fn list(&self, config: &AppConfig, server: &str) -> DomainResult<ServerListing> {
        let cfg = self.storage_cfg(config)?;
        let extra = cfg.rclone_args();
        let mut cache: HashMap<String, Vec<String>> = HashMap::new();
        let mut listing = ServerListing {
            server: server.to_string(),
            ..Default::default()
        };

        for (app, profiles) in config.applications() {
            let mut items = Vec::new();
            for p in profiles {
                let dir = self.remote_dir_for(config, p, server)?;
                if !cache.contains_key(&dir) {
                    let files = self.storage.list_remote_files(&dir, &extra)?;
                    cache.insert(dir.clone(), files);
                }
                items.push(ListItem {
                    profile: p.name.clone(),
                    profile_type: p.profile_type,
                    latest: pick_latest(&cache[&dir], p, None),
                });
            }
            listing.applications.insert(app, items);
        }
        for rp in config.config_profiles() {
            listing.config_profiles.push(rp.name.clone());
        }
        Ok(listing)
    }

    /// Exécute la restauration demandée et retourne le bilan détaillé.
    pub fn execute(&self, config: &AppConfig, opts: &ServerRestoreOptions) -> DomainResult<ServerRestoreReport> {
        let cfg = self.storage_cfg(config)?;
        let extra = cfg.rclone_args();
        config.current_server_name().ok_or_else(|| {
            DomainError::Config(
                "Nom du serveur courant introuvable : renseigner 'settings.storage.server_name' ou une cible 'remote' de la forme <racine>/<serveur>/<dossier>".to_string(),
            )
        })?;

        let requested: Vec<String> = opts
            .application
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if requested.is_empty() {
            return Err(DomainError::Config("Aucune application indiquée (--application <nom|all>)".to_string()));
        }
        let all = requested.iter().any(|a| a.eq_ignore_ascii_case("all"));
        if all && requested.len() > 1 {
            return Err(DomainError::Config("'all' ne se combine pas avec d'autres applications".to_string()));
        }
        if opts.include_system && !all {
            return Err(DomainError::Config(
                "--include-system n'est utilisable qu'avec --application all".to_string(),
            ));
        }
        if opts.include_system && !opts.force && !opts.dry_run {
            return Err(DomainError::Config(
                "--include-system écrase des fichiers système (/etc, crontab...) : ajouter --force pour confirmer".to_string(),
            ));
        }

        // Contrôle préalable : le serveur demandé existe-t-il sur le stockage distant ?
        // (évite plusieurs minutes de tentatives rclone sur un nom de serveur erroné)
        let probe_remote = self.remote_dir_for_global(config, &opts.server)?;
        self.storage
            .list_remote_files(&probe_remote, &extra)
            .map_err(|e| {
                DomainError::Storage(format!(
                    "Serveur '{}' introuvable ou inaccessible sur le stockage distant ({}) : {}",
                    opts.server, probe_remote, e
                ))
            })?;

        let known = config.applications();
        let mut report = ServerRestoreReport {
            server: opts.server.clone(),
            dry_run: opts.dry_run,
            steps: Vec::new(),
        };

        let work_dir = opts
            .work_dir
            .clone()
            .unwrap_or_else(|| config.settings.backup_directory.join(format!(".restore-{}", opts.server)));

        // 1. Sources de configuration (dépôts restic) : téléchargement et découverte des applications.
        let mut config_sources: Vec<ConfigSource> = Vec::new();
        let mut names: BTreeSet<String> = if all {
            known.keys().cloned().collect()
        } else {
            requested.iter().cloned().collect()
        };

        if config.config_profiles().is_empty() {
            report.push("(configuration)", "dépôt restic", StepStatus::Skipped,
                "aucun profil restic : la configuration (compose, .env) n'est pas restaurée");
        } else if opts.dry_run {
            for rp in config.config_profiles() {
                match self.remote_dir_for(config, rp, &opts.server) {
                    Ok(dir) => report.push("(configuration)", &format!("dépôt restic '{}'", rp.name), StepStatus::Planned,
                        format!("téléchargement de {} puis restic restore", dir)),
                    Err(e) => report.push("(configuration)", &format!("dépôt restic '{}'", rp.name), StepStatus::Failed, e.to_string()),
                }
            }
        } else {
            for rp in config.config_profiles() {
                let label = format!("dépôt restic '{}'", rp.name);
                match self.prepare_config_source(config, rp, opts, &work_dir, cfg, &extra) {
                    Ok(src) => {
                        if all {
                            names.extend(src.discovered.iter().cloned());
                        }
                        config_sources.push(src);
                    }
                    Err(e) => report.push("(configuration)", &label, StepStatus::Failed, e.to_string()),
                }
            }
        }

        // 2. Contrôle des applications demandées et protection contre l'écrasement.
        let apps_root = config.settings.applications_root.clone();
        let mut approved: Vec<String> = Vec::new();
        for app in &names {
            let in_profiles = known.contains_key(app);
            let in_snapshot = config_sources
                .iter()
                .any(|s| s.discovered.iter().any(|d| d == app));
            if !in_profiles && !in_snapshot && !opts.dry_run {
                report.push(app, "application", StepStatus::Failed,
                    format!("introuvable : aucun profil '{}' et aucun dossier {}/{} dans la configuration sauvegardée de '{}'",
                        app, apps_root.display(), app, opts.server));
                continue;
            }
            let mut present: Vec<PathBuf> = Vec::new();
            let app_dir = apps_root.join(app);
            if exists_non_empty(&app_dir) {
                present.push(app_dir);
            }
            for p in known.get(app).map(|v| v.as_slice()).unwrap_or(&[]) {
                if p.profile_type == ProfileType::Filesystem {
                    for path in p.source.paths.iter().flatten() {
                        if exists_non_empty(path) {
                            present.push(path.clone());
                        }
                    }
                }
            }
            if !present.is_empty() && !opts.force && !opts.dry_run {
                let list: Vec<String> = present.iter().map(|p| p.display().to_string()).collect();
                report.push(app, "protection", StepStatus::Failed,
                    format!("déjà présente sur cette machine ({}) : ajouter --force pour écraser", list.join(", ")));
                continue;
            }
            if !present.is_empty() && opts.dry_run && !opts.force {
                report.push(app, "protection", StepStatus::Planned,
                    "déjà présente sur cette machine : --force serait nécessaire pour l'écraser");
            }
            approved.push(app.clone());
        }

        // 3. Configuration : un seul restic restore par dépôt, limité aux applications approuvées.
        for src in &config_sources {
            let label = format!("configuration ({})", src.profile.name);
            let includes: Vec<String> = if opts.include_system {
                Vec::new()
            } else {
                approved
                    .iter()
                    .filter(|a| src.discovered.iter().any(|d| d == *a))
                    .map(|a| apps_root.join(a).display().to_string())
                    .collect()
            };
            if !opts.include_system && includes.is_empty() {
                continue;
            }
            let provider = ResticProvider::new(self.executor);
            let res = provider.restore_filtered(&src.profile, &src.snapshot, Path::new("/"), &includes);
            let scope = if opts.include_system { "snapshot complet".to_string() } else { includes.join(", ") };
            match res {
                Ok(()) => {
                    for app in &approved {
                        if opts.include_system || src.discovered.iter().any(|d| d == app) {
                            report.push(app, &label, StepStatus::Done, format!("snapshot {} restauré ({})", short(&src.snapshot), scope));
                        }
                    }
                }
                Err(e) => {
                    for app in &approved {
                        report.push(app, &label, StepStatus::Failed, e.to_string());
                    }
                }
            }
        }

        // 4. Fichiers uploadés, démarrage, bases de données — application par application.
        let mut remote_cache: HashMap<String, Vec<String>> = HashMap::new();
        for app in &approved {
            let profiles: Vec<&BackupProfile> = known.get(app).cloned().unwrap_or_default();

            for p in profiles.iter().filter(|p| p.profile_type == ProfileType::Filesystem) {
                self.restore_uploads(config, p, app, opts, &work_dir, cfg, &extra, &mut remote_cache, &mut report);
            }

            let dbs: Vec<&&BackupProfile> = profiles.iter().filter(|p| is_database(p.profile_type)).collect();
            if opts.start && !dbs.is_empty() {
                let dir = apps_root.join(app);
                self.start_application(app, &dir, &dbs, opts, &mut report);
            }
            for p in dbs {
                self.restore_database(config, p, app, opts, &work_dir, cfg, &extra, &mut remote_cache, &mut report);
            }
        }

        // 5. Historique et nettoyage.
        if !opts.dry_run {
            let apps_with_steps: BTreeSet<&str> = report.steps.iter().map(|s| s.application.as_str()).collect();
            for app in apps_with_steps {
                let failed = report.steps.iter().any(|s| s.application == app && s.status == StepStatus::Failed);
                let _ = HistoryAction::record(
                    &config.settings.backup_directory,
                    "restore-server",
                    Some(app),
                    Some(&opts.server),
                    if failed { "FAILURE" } else { "SUCCESS" },
                    &format!("Restauration depuis le serveur '{}'", opts.server),
                );
            }
            if !opts.keep_downloads {
                let _ = fs::remove_dir_all(&work_dir);
            }
        }

        Ok(report)
    }

    /// Télécharge le dépôt restic d'un serveur et détermine le snapshot et les applications présentes.
    fn prepare_config_source(
        &self,
        config: &AppConfig,
        rp: &BackupProfile,
        opts: &ServerRestoreOptions,
        work_dir: &Path,
        cfg: &RemoteStorageConfig,
        extra: &[String],
    ) -> DomainResult<ConfigSource> {
        let remote = self.remote_dir_for(config, rp, &opts.server)?;
        let local_repo = work_dir.join(format!("restic-{}", rp.name));
        self.storage
            .download_dir(&remote, &local_repo, &cfg.config_max_duration, extra)?;

        let mut profile = rp.clone();
        profile.source.restic_repository = Some(local_repo.to_string_lossy().to_string());

        let provider = ResticProvider::new(self.executor);
        let snapshot = provider.pick_snapshot(&profile, opts.before)?;
        let root = config.settings.applications_root.display().to_string();
        let discovered = provider
            .list_child_dirs(&profile, &snapshot, &root)?
            .into_iter()
            .filter_map(|p| p.rsplit('/').next().map(|s| s.to_string()))
            .collect();
        Ok(ConfigSource { profile, snapshot, discovered })
    }

    /// Dossier distant et liste des fichiers (mise en cache) pour un profil.
    fn remote_files(
        &self,
        config: &AppConfig,
        profile: &BackupProfile,
        server: &str,
        extra: &[String],
        cache: &mut HashMap<String, Vec<String>>,
    ) -> DomainResult<(String, Vec<String>)> {
        let dir = self.remote_dir_for(config, profile, server)?;
        if !cache.contains_key(&dir) {
            let files = self.storage.list_remote_files(&dir, extra)?;
            cache.insert(dir.clone(), files);
        }
        Ok((dir.clone(), cache[&dir].clone()))
    }

    #[allow(clippy::too_many_arguments)]
    fn restore_uploads(
        &self,
        config: &AppConfig,
        profile: &BackupProfile,
        app: &str,
        opts: &ServerRestoreOptions,
        work_dir: &Path,
        cfg: &RemoteStorageConfig,
        extra: &[String],
        cache: &mut HashMap<String, Vec<String>>,
        report: &mut ServerRestoreReport,
    ) {
        let label = format!("fichiers uploadés '{}'", profile.name);
        let result: DomainResult<(String, usize)> = (|| {
            let (dir, files) = self.remote_files(config, profile, &opts.server, extra, cache)?;
            let file = pick_latest(&files, profile, opts.before).ok_or_else(|| {
                DomainError::Storage(format!("aucune archive de '{}' trouvée dans {}", profile.name, dir))
            })?;
            if opts.dry_run {
                return Ok((file, 0));
            }
            // Téléchargement d'un fichier unique (`copyto`) : pas de filtres rclone.
            let local = self.storage.download_file(&dir, &file, &work_dir.join(&profile.name), &cfg.upload_max_duration, &cfg.rclone_base_args())?;
            let algo = detect_compression(&local)?;
            // Les chemins sont stockés absolus (sans « / » initial) : extraction à la racine.
            let count = FilesystemProvider::new().extract_archive(&local, Path::new("/"), algo, None)?;
            Ok((file, count))
        })();
        match result {
            Ok((file, _)) if opts.dry_run => report.push(app, &label, StepStatus::Planned, format!("restaurerait {}", file)),
            Ok((file, count)) => report.push(app, &label, StepStatus::Done, format!("{} ({} élément(s))", file, count)),
            Err(e) => report.push(app, &label, StepStatus::Failed, e.to_string()),
        }
    }

    fn start_application(
        &self,
        app: &str,
        dir: &Path,
        dbs: &[&&BackupProfile],
        opts: &ServerRestoreOptions,
        report: &mut ServerRestoreReport,
    ) {
        let label = "démarrage (docker compose up -d)";
        if opts.dry_run {
            report.push(app, label, StepStatus::Planned, format!("docker compose --project-directory {} up -d", dir.display()));
            return;
        }
        let dir_str = dir.to_string_lossy().to_string();
        let res = self
            .executor
            .execute("docker", &["compose", "--project-directory", &dir_str, "up", "-d"], &[])
            .and_then(|out| {
                if out.success {
                    Ok(())
                } else {
                    Err(DomainError::Execution {
                        command: "docker compose up".to_string(),
                        message: out.stderr.trim().to_string(),
                        exit_code: Some(out.exit_code),
                    })
                }
            });
        match res {
            Ok(()) => {
                for p in dbs {
                    if let Some(c) = &p.source.container_name {
                        self.wait_for_container(c);
                    }
                }
                report.push(app, label, StepStatus::Done, dir_str);
            }
            Err(e) => report.push(app, label, StepStatus::Failed, e.to_string()),
        }
    }

    fn wait_for_container(&self, container: &str) {
        let mut waited = 0;
        while waited < CONTAINER_WAIT_SECS {
            if self.executor.is_container_running(container).unwrap_or(false) {
                return;
            }
            thread::sleep(Duration::from_secs(2));
            waited += 2;
        }
        warn!("[LOG RESTORE] Le conteneur '{}' n'a pas démarré après {} s", container, CONTAINER_WAIT_SECS);
    }

    #[allow(clippy::too_many_arguments)]
    fn restore_database(
        &self,
        config: &AppConfig,
        profile: &BackupProfile,
        app: &str,
        opts: &ServerRestoreOptions,
        work_dir: &Path,
        cfg: &RemoteStorageConfig,
        extra: &[String],
        cache: &mut HashMap<String, Vec<String>>,
        report: &mut ServerRestoreReport,
    ) {
        let label = format!("base '{}' ({})", profile.name, profile.profile_type.label());
        let apps_root = &config.settings.applications_root;
        let result: DomainResult<String> = (|| {
            let (dir, files) = self.remote_files(config, profile, &opts.server, extra, cache)?;
            let file = pick_latest(&files, profile, opts.before).ok_or_else(|| {
                DomainError::Storage(format!("aucune sauvegarde de '{}' trouvée dans {}", profile.name, dir))
            })?;
            if opts.dry_run {
                return Ok(file);
            }

            if let Some(container) = &profile.source.container_name {
                if !self.executor.is_container_running(container)? {
                    return Err(DomainError::Config(format!(
                        "le conteneur '{}' n'est pas démarré : lancer `docker compose --project-directory {} up -d` puis relancer la restauration, ou utiliser --start",
                        container,
                        apps_root.join(app).display()
                    )));
                }
            }

            // Téléchargement d'un fichier unique (`copyto`) : pas de filtres rclone.
            let local = self.storage.download_file(&dir, &file, &work_dir.join(&profile.name), &cfg.upload_max_duration, &cfg.rclone_base_args())?;
            let db = DatabaseProvider::new(self.executor);
            match profile.profile_type {
                ProfileType::Postgres => db.restore_postgres(profile, &local, None)?,
                ProfileType::Mysql => db.restore_mysql(profile, &local, None)?,
                ProfileType::Mariadb => db.restore_mariadb(profile, &local, None)?,
                ProfileType::Mongo => db.restore_mongo(profile, &local, None)?,
                _ => unreachable!("seuls les profils de base de données arrivent ici"),
            }
            Ok(file)
        })();
        match result {
            Ok(file) if opts.dry_run => report.push(app, &label, StepStatus::Planned, format!("restaurerait {}", file)),
            Ok(file) => {
                info!("[LOG RESTORE] Base '{}' restaurée depuis {}", profile.name, file);
                report.push(app, &label, StepStatus::Done, file)
            }
            Err(e) => report.push(app, &label, StepStatus::Failed, e.to_string()),
        }
    }
}

/// Raccourcit un identifiant de snapshot pour l'affichage.
fn short(snapshot: &str) -> &str {
    if snapshot.len() > 8 { &snapshot[..8] } else { snapshot }
}
