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
