//! Modèles de données purs du domaine de sauvegarde.
//!
//! Ce module contient les structures de configuration, de profil, de métadonnées
//! et d'historique, exemptes d'I/O direct conformément aux principes du domaine.

use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::exceptions::{DomainError, DomainResult};

/// Configuration principale de l'application chargée depuis un fichier YAML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Version du schéma de configuration (doit valoir 1).
    pub version: u32,

    /// Réglages globaux de l'application.
    pub settings: GlobalSettings,

    /// Liste des profils de sauvegarde configurés.
    pub profiles: Vec<BackupProfile>,
}

/// Réglages généraux partagés entre tous les profils.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalSettings {
    /// Niveau de log souhaité (ex: info, debug, warn).
    #[serde(default = "default_log_level")]
    pub log_level: String,

    /// Chemin du fichier de journalisation central.
    #[serde(default = "default_log_file")]
    pub log_file: PathBuf,

    /// Dossier racine où sont stockées les sauvegardes locales.
    #[serde(default = "default_backup_directory")]
    pub backup_directory: PathBuf,

    /// Chemin de base des fichiers de verrou : un verrou par profil est dérivé de ce chemin
    /// (ex: `/var/lock/backup-cli.lock` -> `/var/lock/backup-cli.solidtime.lock`).
    #[serde(default = "default_lock_file")]
    pub lock_file: PathBuf,

    /// Configuration optionnelle des notifications (Slack / Webhook).
    pub notifications: Option<NotificationConfig>,

    /// Configuration optionnelle du stockage distant global (ex: rclone / S3).
    pub storage: Option<RemoteStorageConfig>,

    /// Dossier racine des applications (ex: `/opt`) : l'application `authentik` y vit dans
    /// `/opt/authentik`. Sert à restaurer la configuration (compose, .env) d'une application.
    #[serde(default = "default_applications_root")]
    pub applications_root: PathBuf,
}

fn default_applications_root() -> PathBuf {
    PathBuf::from("/opt")
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> PathBuf {
    PathBuf::from("/backup/backup.log")
}

fn default_backup_directory() -> PathBuf {
    PathBuf::from("/backup")
}

fn default_lock_file() -> PathBuf {
    PathBuf::from("/var/lock/backup-cli.lock")
}

/// Paramètres de notification d'alerte et de suivi d'exécution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationConfig {
    /// Activation ou désactivation des notifications.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// URL directe du webhook Slack (déconseillée en clair dans le fichier).
    pub slack_webhook_url: Option<String>,

    /// Nom de la variable d'environnement contenant l'URL du webhook Slack.
    pub slack_webhook_env: Option<String>,
}

/// Paramètres de stockage distant (ex: S3 via rclone).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteStorageConfig {
    /// Chemin du fichier de configuration rclone.
    pub rclone_config: Option<PathBuf>,

    /// Cible distante rclone pour les sauvegardes de fichiers (ex: "s3backup:backups/s001/backup").
    pub remote: String,

    /// Cible distante rclone pour les sauvegardes de configuration (ex: "s3backup:backups/s001/backup-configuration").
    pub config_remote: Option<String>,

    /// Plafond de durée par transfert de fichier (ex: "6h").
    #[serde(default = "default_upload_duration")]
    pub upload_max_duration: String,

    /// Plafond de durée pour l'envoi de configuration (ex: "12h").
    #[serde(default = "default_config_upload_duration")]
    pub config_max_duration: String,

    /// Arguments supplémentaires transmis à rclone.
    pub extra_args: Option<Vec<String>>,

    /// Nom de CE serveur dans l'arborescence distante (ex: "s001"). Déduit par défaut de
    /// `remote` selon la convention `<racine>/<serveur>/<dossier>`.
    pub server_name: Option<String>,
}

impl RemoteStorageConfig {
    /// Arguments à passer à chaque appel rclone : `--config <rclone_config>` (si renseigné)
    /// suivi des `extra_args`.
    pub fn rclone_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(cfg) = &self.rclone_config {
            args.push("--config".to_string());
            args.push(cfg.to_string_lossy().to_string());
        }
        if let Some(extra) = &self.extra_args {
            args.extend(extra.iter().cloned());
        }
        args
    }
}

fn default_upload_duration() -> String {
    "6h".to_string()
}

fn default_config_upload_duration() -> String {
    "12h".to_string()
}

/// Profil de sauvegarde autonome décrivant une source et ses règles.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupProfile {
    /// Nom unique du profil (ex: "authentik", "ghost", "configs").
    pub name: String,

    /// Active ou désactive le profil lors de l'exécution globale.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Type de sauvegarde (Postgres, Mysql, Mariadb, Mongo, Filesystem, Restic).
    #[serde(rename = "type")]
    pub profile_type: ProfileType,

    /// Expression cron de planification (ex: "0 2 * * *").
    pub schedule: Option<String>,

    /// Configuration de la source de données à sauvegarder.
    pub source: SourceConfig,

    /// Configuration de la destination locale ou distante.
    #[serde(default)]
    pub destination: DestinationConfig,

    /// Options de compression du fichier de sauvegarde.
    pub compression: Option<CompressionConfig>,

    /// Politique de rétention et de rotation des archives.
    pub retention: Option<RetentionPolicy>,

    /// Options de vérification d'intégrité.
    pub validation: Option<ValidationConfig>,

    /// Application à laquelle appartient ce profil (défaut : le nom du profil).
    ///
    /// Plusieurs profils peuvent désigner la même application : sa base de données
    /// (postgres, mysql, mariadb, mongo) et ses fichiers uploadés (filesystem).
    /// `restore --application <nom>` les restaure ensemble.
    #[serde(default)]
    pub application: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Types de profils de sauvegarde pris en charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProfileType {
    /// Base de données PostgreSQL (pg_dump -Fc).
    Postgres,
    /// Base de données MySQL (mysqldump / mysql).
    Mysql,
    /// Base de données MariaDB (mariadb-dump / mariadb).
    Mariadb,
    /// Base de données MongoDB (mongodump / mongorestore, archive gzip).
    #[serde(alias = "mongodb")]
    Mongo,
    /// Arborescence de fichiers système (tar).
    Filesystem,
    /// Dépôt de sauvegarde incrémentale dédupliquée Restic.
    Restic,
}

/// Paramètres de la source de données.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceConfig {
    /// Nom du conteneur Docker hébergeant le service (ex: "authentik-postgresql-1").
    pub container_name: Option<String>,

    /// Hôte réseau de la base de données (si hors Docker).
    pub host: Option<String>,

    /// Port réseau de la base de données.
    pub port: Option<u16>,

    /// Utilisateur de connexion à la base de données.
    pub username: Option<String>,

    /// Nom de la base de données à sauvegarder (MongoDB : toutes les bases si omis).
    pub database: Option<String>,

    /// Base d'authentification MongoDB (`--authenticationDatabase`, "admin" par défaut).
    pub auth_database: Option<String>,

    /// Nom de la variable d'environnement contenant le mot de passe.
    pub password_env: Option<String>,

    /// Chemin d'un fichier contenant le mot de passe.
    pub password_file: Option<PathBuf>,

    /// Arguments spécifiques supplémentaires pour pg_dump ou mysqldump.
    pub dump_options: Option<Vec<String>>,

    /// Chemins des répertoires ou fichiers à sauvegarder (filesystem ou restic).
    pub paths: Option<Vec<PathBuf>>,

    /// Motifs de fichiers ou dossiers à exclure (globs).
    pub exclude: Option<Vec<String>>,

    /// Commandes préparatoires exécutées avant la sauvegarde (ex: dpkg export, crontab dump).
    pub pre_commands: Option<Vec<String>>,

    /// Commandes exécutées après la sauvegarde avec succès.
    pub post_commands: Option<Vec<String>>,

    /// Chemin ou URL du dépôt Restic.
    pub restic_repository: Option<String>,

    /// Chemin du fichier de mot de passe Restic.
    pub restic_password_file: Option<PathBuf>,

    /// Variable d'environnement pour le mot de passe Restic.
    pub restic_password_env: Option<String>,

    /// Tags associés au snapshot Restic.
    pub tags: Option<Vec<String>>,
}

/// Paramètres de destination de sauvegarde.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DestinationConfig {
    /// Dossier local spécifique où écrire la sauvegarde (hérite de settings si absent).
    pub path: Option<PathBuf>,

    /// Cible distante spécifique pour ce profil.
    pub remote: Option<String>,

    /// Si vrai, synchronise (sync) au lieu de copier (copyto).
    pub remote_sync: Option<bool>,
}

/// Algorithmes de compression pris en charge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompressionAlgorithm {
    Gzip,
    Zstd,
    None,
}

impl Default for CompressionAlgorithm {
    fn default() -> Self {
        Self::None
    }
}

/// Options de compression du profil.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionConfig {
    /// Indique si la compression est activée.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Algorithme employé (gzip, zstd, none).
    #[serde(default)]
    pub algorithm: CompressionAlgorithm,

    /// Niveau de compression optionnel.
    pub level: Option<i32>,
}

/// Politique de rétention et de rotation des sauvegardes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Nombre de jours de conservation simple (style -mtime +N).
    pub days: Option<u32>,

    /// Nombre de sauvegardes quotidiennes conservées (schéma GFS).
    pub daily: Option<u32>,

    /// Nombre de sauvegardes hebdomadaires conservées.
    pub weekly: Option<u32>,

    /// Nombre de sauvegardes mensuelles conservées.
    pub monthly: Option<u32>,
}

/// Options de validation d'intégrité.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationConfig {
    /// Algorithme de hachage (par défaut sha256).
    #[serde(default = "default_checksum")]
    pub checksum: String,

    /// Si vrai, recalcule et valide immédiatement le hash après création.
    #[serde(default = "default_true")]
    pub verify_after_backup: bool,
}

fn default_checksum() -> String {
    "sha256".to_string()
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            checksum: default_checksum(),
            verify_after_backup: true,
        }
    }
}

/// Statut d'une opération de sauvegarde.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum BackupStatus {
    Success,
    Failed,
    Partial,
}

/// Métadonnées détaillées d'une sauvegarde persistée.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupMetadata {
    /// Identifiant unique de la sauvegarde (ex: "2026-11-17_12-30-00_authentik").
    pub id: String,

    /// Nom du profil ayant produit cette sauvegarde.
    pub profile_name: String,

    /// Type de profil d'origine.
    pub profile_type: ProfileType,

    /// Horodatage UTC précis de création.
    pub created_at: DateTime<Utc>,

    /// Chemin absolu du fichier de sauvegarde.
    pub file_path: PathBuf,

    /// Taille du fichier en octets.
    pub file_size: u64,

    /// Empreinte SHA-256 du fichier complet.
    pub checksum_sha256: String,

    /// Algorithme de compression utilisé.
    pub compression: CompressionAlgorithm,

    /// Statut final de l'opération.
    pub status: BackupStatus,

    /// Durée de l'opération en secondes.
    pub duration_seconds: f64,
}

/// Entrée du journal d'historique des opérations de sauvegarde et de restauration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    /// Horodatage UTC de l'événement.
    pub timestamp: DateTime<Utc>,

    /// Type d'opération ("backup", "restore", "retention", "verify").
    pub operation: String,

    /// Nom du profil concerné le cas échéant.
    pub profile: Option<String>,

    /// Identifiant de sauvegarde associé.
    pub backup_id: Option<String>,

    /// Résultat statut ("SUCCESS", "FAILURE", "WARNING").
    pub status: String,

    /// Message descriptif de l'opération.
    pub details: String,
}

impl AppConfig {
    /// Valide la cohérence logique et la sécurité de la configuration.
    ///
    /// Vérifie notamment :
    /// - Le numéro de version du schéma
    /// - La présence d'au moins un profil
    /// - L'unicité des noms de profil
    /// - L'interdiction formelle de mots de passe en clair
    /// - Les prérequis spécifiques selon le type (Postgres, Mysql, Filesystem, Restic)
    /// - La validité syntaxique des motifs d'exclusion
    pub fn validate(&self) -> DomainResult<()> {
        if self.version != 1 {
            return Err(DomainError::Config(format!(
                "Version de schéma de configuration non supportée : {} (attendu : 1)",
                self.version
            )));
        }

        if self.profiles.is_empty() {
            return Err(DomainError::Config(
                "Le fichier de configuration ne définit aucun profil de sauvegarde".to_string(),
            ));
        }

        let mut seen_names = std::collections::HashSet::new();

        for profile in &self.profiles {
            if profile.name.trim().is_empty() {
                return Err(DomainError::Config(
                    "Un nom de profil ne peut pas être vide".to_string(),
                ));
            }

            if !seen_names.insert(profile.name.to_lowercase()) {
                return Err(DomainError::Config(format!(
                    "Nom de profil dupliqué : '{}'",
                    profile.name
                )));
            }

            self.validate_profile(profile)?;
        }

        Ok(())
    }

    /// Valide un profil individuel selon son type.
    fn validate_profile(&self, profile: &BackupProfile) -> DomainResult<()> {
        let src = &profile.source;

        if profile.profile_type != ProfileType::Restic {
            let app = profile.application_name();
            let valid_chars = app
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
            if let Some(explicit) = &profile.application {
                if explicit.is_empty() || !valid_chars {
                    return Err(DomainError::Config(format!(
                        "Profil '{}' : 'application' invalide ('{}'), caractères autorisés : lettres, chiffres, - _ .",
                        profile.name, explicit
                    )));
                }
            }
            if app.eq_ignore_ascii_case("all") {
                return Err(DomainError::Config(format!(
                    "Profil '{}' : le nom d'application 'all' est réservé (restore --application all)",
                    profile.name
                )));
            }
        }

        match profile.profile_type {
            ProfileType::Postgres => {
                if src.container_name.is_none() && src.host.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil Postgres '{}' : spécifier 'container_name' ou 'host'",
                        profile.name
                    )));
                }
                if src.username.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil Postgres '{}' : 'username' requis",
                        profile.name
                    )));
                }
                if src.database.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil Postgres '{}' : 'database' requis",
                        profile.name
                    )));
                }
            }
            ProfileType::Mysql | ProfileType::Mariadb => {
                let label = if profile.profile_type == ProfileType::Mariadb {
                    "MariaDB"
                } else {
                    "MySQL"
                };
                if src.container_name.is_none() && src.host.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil {} '{}' : spécifier 'container_name' ou 'host'",
                        label, profile.name
                    )));
                }
                if src.username.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil {} '{}' : 'username' requis",
                        label, profile.name
                    )));
                }
                if src.database.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil {} '{}' : 'database' requis",
                        label, profile.name
                    )));
                }
            }
            ProfileType::Mongo => {
                if src.container_name.is_none() && src.host.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil MongoDB '{}' : spécifier 'container_name' ou 'host'",
                        profile.name
                    )));
                }
                let has_password = src.password_env.is_some() || src.password_file.is_some();
                if has_password && src.username.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil MongoDB '{}' : 'username' requis lorsqu'un mot de passe est fourni",
                        profile.name
                    )));
                }
                if let Some(c) = &profile.compression {
                    if c.enabled && c.algorithm == CompressionAlgorithm::Zstd {
                        return Err(DomainError::Config(format!(
                            "Profil MongoDB '{}' : mongodump ne supporte que la compression gzip",
                            profile.name
                        )));
                    }
                }
            }
            ProfileType::Filesystem => {
                match &src.paths {
                    Some(paths) if !paths.is_empty() => {}
                    _ => {
                        return Err(DomainError::Config(format!(
                            "Profil Filesystem '{}' : au moins un chemin dans 'paths' est requis",
                            profile.name
                        )));
                    }
                }
            }
            ProfileType::Restic => {
                if src.restic_repository.is_none() && profile.destination.path.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil Restic '{}' : spécifier 'restic_repository' ou 'destination.path'",
                        profile.name
                    )));
                }
                if src.restic_password_file.is_none() && src.restic_password_env.is_none() {
                    return Err(DomainError::Config(format!(
                        "Profil Restic '{}' : spécifier 'restic_password_file' ou 'restic_password_env'",
                        profile.name
                    )));
                }
                match &src.paths {
                    Some(paths) if !paths.is_empty() => {}
                    _ => {
                        return Err(DomainError::Config(format!(
                            "Profil Restic '{}' : au moins un chemin dans 'paths' est requis",
                            profile.name
                        )));
                    }
                }
            }
        }

        // Validation des motifs d'exclusion
        if let Some(excludes) = &src.exclude {
            for pattern in excludes {
                if glob::Pattern::new(pattern).is_err() {
                    return Err(DomainError::Config(format!(
                        "Profil '{}' : motif d'exclusion invalide '{}'",
                        profile.name, pattern
                    )));
                }
            }
        }

        Ok(())
    }

    /// Chemin du fichier de verrou propre à un profil.
    ///
    /// Empêche deux exécutions simultanées du même profil tout en autorisant
    /// l'exécution en parallèle de profils différents.
    pub fn profile_lock_path(&self, profile: &BackupProfile) -> PathBuf {
        let base = &self.settings.lock_file;
        let safe_name: String = profile
            .name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
            .collect();
        let stem = base
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "backup-cli".to_string());
        let ext = base
            .extension()
            .map(|e| e.to_string_lossy().to_string())
            .unwrap_or_else(|| "lock".to_string());
        base.with_file_name(format!("{}.{}.{}", stem, safe_name, ext))
    }

    /// Récupère le dossier effectif de sauvegarde pour un profil donné.
    pub fn resolve_backup_dir<'a>(&'a self, profile: &'a BackupProfile) -> &'a Path {
        if let Some(dest_path) = &profile.destination.path {
            dest_path.as_path()
        } else {
            self.settings.backup_directory.as_path()
        }
    }
}

impl BackupProfile {
    /// Algorithme de compression appliqué aux dumps MySQL / MariaDB.
    ///
    /// - `compression` absent : gzip (comportement historique du nommage `.sql.gz`)
    /// - `enabled: false` : aucune compression
    /// - `enabled: true` sans `algorithm` (ou `none`) : gzip
    pub fn sql_compression(&self) -> CompressionAlgorithm {
        match &self.compression {
            None => CompressionAlgorithm::Gzip,
            Some(c) if !c.enabled => CompressionAlgorithm::None,
            Some(c) => match c.algorithm {
                CompressionAlgorithm::None => CompressionAlgorithm::Gzip,
                other => other,
            },
        }
    }

    /// Indique si l'archive MongoDB est compressée en gzip (`mongodump --gzip`).
    ///
    /// Activé par défaut ; désactivé uniquement par `compression: { enabled: false }`.
    pub fn mongo_uses_gzip(&self) -> bool {
        self.compression.as_ref().map(|c| c.enabled).unwrap_or(true)
    }
}

impl ProfileType {
    /// Libellé court et stable du type de profil (affichage, notifications).
    pub fn label(&self) -> &'static str {
        match self {
            ProfileType::Postgres => "postgres",
            ProfileType::Mysql => "mysql",
            ProfileType::Mariadb => "mariadb",
            ProfileType::Mongo => "mongo",
            ProfileType::Filesystem => "filesystem",
            ProfileType::Restic => "restic",
        }
    }
}

/// Résultat d'un profil au sein d'un cycle de sauvegarde (utilisé par les notifications).
#[derive(Debug, Clone)]
pub struct ProfileReport {
    /// Nom du profil.
    pub name: String,
    /// Type du profil.
    pub profile_type: ProfileType,
    /// `Success` ou `Failed`.
    pub status: BackupStatus,
    /// Taille du fichier produit, si connue.
    pub size_bytes: Option<u64>,
    /// Durée d'exécution du profil en secondes.
    pub duration_seconds: f64,
    /// Message d'erreur (échec) ou précision (ex: « snapshot restic créé »).
    pub detail: Option<String>,
}

impl ProfileReport {
    /// Vrai si le profil a réussi.
    pub fn is_success(&self) -> bool {
        self.status == BackupStatus::Success
    }

    /// Description courte du résultat, sans icône ni nom (ex: « 12,3 Mo en 4 s »).
    pub fn summary(&self) -> String {
        if self.is_success() {
            let what = match (self.size_bytes, &self.detail) {
                (Some(size), _) => format_size(size),
                (None, Some(d)) => d.clone(),
                (None, None) => "terminé".to_string(),
            };
            format!("{} en {}", what, format_duration(self.duration_seconds))
        } else {
            format!(
                "échec : {}",
                short_error(self.detail.as_deref().unwrap_or("erreur inconnue"))
            )
        }
    }
}

/// Bilan complet d'un cycle de sauvegarde, destiné aux notifications.
#[derive(Debug, Clone)]
pub struct BackupReport {
    /// Nom de la machine ayant exécuté les sauvegardes.
    pub hostname: String,
    /// Début du cycle (UTC).
    pub started_at: DateTime<Utc>,
    /// Durée totale du cycle en secondes.
    pub duration_seconds: f64,
    /// Résultat de chaque profil exécuté.
    pub profiles: Vec<ProfileReport>,
}

impl BackupReport {
    /// Nombre total de profils exécutés.
    pub fn total(&self) -> usize {
        self.profiles.len()
    }

    /// Nombre de profils sauvegardés avec succès.
    pub fn succeeded(&self) -> usize {
        self.profiles.iter().filter(|p| p.is_success()).count()
    }

    /// Nombre de profils en échec.
    pub fn failed(&self) -> usize {
        self.total() - self.succeeded()
    }

    /// Profils triés pour l'affichage : échecs d'abord, puis ordre d'exécution.
    pub fn sorted_profiles(&self) -> Vec<&ProfileReport> {
        let mut list: Vec<&ProfileReport> = self.profiles.iter().collect();
        list.sort_by_key(|p| p.is_success());
        list
    }

    /// Titre en une ligne : état global et décompte.
    pub fn headline(&self) -> String {
        let (ok, ko, total) = (self.succeeded(), self.failed(), self.total());
        let profils = if total > 1 { "profils" } else { "profil" };
        if ko == 0 {
            format!(
                "✅ Sauvegardes réussies — {}/{} {} sauvegardé{}",
                ok,
                total,
                profils,
                if total > 1 { "s" } else { "" }
            )
        } else if ok == 0 {
            format!(
                "🚨 Échec des sauvegardes — {} sur {} {} en échec, aucune réussie",
                ko, total, profils
            )
        } else {
            format!(
                "⚠️ Sauvegardes partielles — {} réussie{}, {} échec{} sur {} profils",
                ok,
                if ok > 1 { "s" } else { "" },
                ko,
                if ko > 1 { "s" } else { "" },
                total
            )
        }
    }

    /// Ligne de décompte : total, réussies, échecs, durée.
    pub fn counts_line(&self) -> String {
        format!(
            "Total : {} · Réussies : {} · Échecs : {} · Durée : {}",
            self.total(),
            self.succeeded(),
            self.failed(),
            format_duration(self.duration_seconds)
        )
    }

    /// Version texte brut complète (repli pour tout canal sans mise en forme riche).
    pub fn to_plain_text(&self) -> String {
        let mut lines = vec![
            format!("{} (serveur {})", self.headline(), self.hostname),
            self.counts_line(),
        ];
        for p in self.sorted_profiles() {
            lines.push(format!(
                "{} {} ({}) — {}",
                if p.is_success() { "✅" } else { "❌" },
                p.name,
                p.profile_type.label(),
                p.summary()
            ));
        }
        lines.join("\n")
    }
}

/// Formate une taille en octets de façon lisible (« 12,3 Mo »).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["o", "Ko", "Mo", "Go", "To"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} o", bytes)
    } else {
        format!("{:.1} {}", value, UNITS[unit]).replace('.', ",")
    }
}

/// Formate une durée en secondes (« 4 s », « 1 min 12 s », « 1 h 02 min »).
pub fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    if total < 60 {
        format!("{} s", total)
    } else if total < 3600 {
        format!("{} min {:02} s", total / 60, total % 60)
    } else {
        format!("{} h {:02} min", total / 3600, (total % 3600) / 60)
    }
}

/// Réduit un message d'erreur à une ligne d'au plus 220 caractères.
pub fn short_error(message: &str) -> String {
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 220 {
        let cut: String = flat.chars().take(219).collect();
        format!("{}…", cut)
    } else {
        flat
    }
}

impl BackupProfile {
    /// Nom de l'application de ce profil (champ `application`, sinon le nom du profil).
    pub fn application_name(&self) -> &str {
        self.application.as_deref().unwrap_or(&self.name)
    }
}

impl AppConfig {
    /// Regroupe les profils (hors restic) par application, triés par nom d'application.
    pub fn applications(&self) -> std::collections::BTreeMap<String, Vec<&BackupProfile>> {
        let mut map: std::collections::BTreeMap<String, Vec<&BackupProfile>> =
            std::collections::BTreeMap::new();
        for p in self
            .profiles
            .iter()
            .filter(|p| p.profile_type != ProfileType::Restic)
        {
            map.entry(p.application_name().to_string()).or_default().push(p);
        }
        map
    }

    /// Profils restic : ils portent les fichiers de configuration des applications.
    pub fn config_profiles(&self) -> Vec<&BackupProfile> {
        self.profiles
            .iter()
            .filter(|p| p.profile_type == ProfileType::Restic)
            .collect()
    }

    /// Nom de ce serveur dans l'arborescence distante (`storage.server_name`, sinon déduit de
    /// `storage.remote` : `s3backup:backups/s001/backup` -> `s001`).
    pub fn current_server_name(&self) -> Option<String> {
        let storage = self.settings.storage.as_ref()?;
        if let Some(name) = storage.server_name.as_ref().filter(|n| !n.is_empty()) {
            return Some(name.clone());
        }
        let path = storage.remote.split_once(':').map(|(_, p)| p).unwrap_or(&storage.remote);
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.len() >= 2 {
            Some(segments[segments.len() - 2].to_string())
        } else {
            None
        }
    }

    /// Transpose une cible distante de CE serveur vers celle d'un autre serveur.
    ///
    /// `s3backup:backups/s001/backup` avec `s002` -> `s3backup:backups/s002/backup`.
    pub fn remote_for_server(&self, remote: &str, server: &str) -> DomainResult<String> {
        let current = self.current_server_name().ok_or_else(|| {
            DomainError::Config(
                "Impossible de déterminer le nom du serveur courant : renseigner \
                 'settings.storage.server_name' ou utiliser une cible 'remote' de la forme \
                 <racine>/<serveur>/<dossier>"
                    .to_string(),
            )
        })?;
        let (prefix, path) = match remote.split_once(':') {
            Some((p, rest)) => (format!("{}:", p), rest),
            None => (String::new(), remote),
        };
        let mut replaced = false;
        let segments: Vec<String> = path
            .split('/')
            .map(|seg| {
                if !replaced && seg == current {
                    replaced = true;
                    server.to_string()
                } else {
                    seg.to_string()
                }
            })
            .collect();
        if !replaced {
            return Err(DomainError::Config(format!(
                "La cible distante '{}' ne contient pas le nom du serveur courant '{}'",
                remote, current
            )));
        }
        Ok(format!("{}{}", prefix, segments.join("/")))
    }

    /// Cible distante (CE serveur) où un profil envoie ses sauvegardes de fichiers/dumps.
    pub fn remote_of_profile(&self, profile: &BackupProfile) -> Option<String> {
        profile
            .destination
            .remote
            .clone()
            .or_else(|| self.settings.storage.as_ref().map(|s| s.remote.clone()))
    }
}
