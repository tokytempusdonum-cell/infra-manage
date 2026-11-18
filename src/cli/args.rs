//! Définition des commandes, sous-commandes et arguments en ligne de commande.

use std::path::PathBuf;
use clap::{Args, Parser, Subcommand};

/// infra-manage : Outil moderne, robuste et modulaire de gestion des sauvegardes et restaurations.
#[derive(Debug, Parser)]
#[command(name = "infra-manage", version, about, long_about = None)]
pub struct Cli {
    /// Fichier de configuration YAML par défaut.
    #[arg(short, long, global = true, default_value = "backup.yml")]
    pub config: PathBuf,

    /// Sous-commande à exécuter.
    #[command(subcommand)]
    pub command: Commands,
}

/// Commandes principales de l'application.
#[derive(Debug, Subcommand)]
pub enum Commands {
    /// Gestion et validation des configurations YAML.
    Config(ConfigArgs),

    /// Gestion de l'exécution et de l'inspection des sauvegardes.
    Backup(BackupCommandGroup),

    /// Restauration complète ou partielle de sauvegardes existantes.
    Restore(RestoreCommandGroup),

    /// Rétention et rotation des anciennes sauvegardes.
    Retention(RetentionCommandGroup),

    /// Consultation de l'historique des opérations.
    History(HistoryArgs),

    /// Planification et génération des entrées crontab.
    Scheduler(SchedulerCommandGroup),

    /// Envoi et test des notifications.
    Notify(NotifyCommandGroup),
}

/// Groupe de commandes pour `config`.
#[derive(Debug, Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigSubcommands,
}

#[derive(Debug, Subcommand)]
pub enum ConfigSubcommands {
    /// Valide un fichier de configuration YAML avant son utilisation.
    Validate {
        /// Chemin du fichier de configuration à valider.
        #[arg(short, long)]
        file: PathBuf,
    },
    /// Liste les profils configurés dans le fichier de configuration.
    List {
        /// Fichier alternatif de configuration.
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
}

/// Groupe de sous-commandes pour `backup`.
#[derive(Debug, Args)]
pub struct BackupCommandGroup {
    #[command(subcommand)]
    pub command: BackupSubcommands,
}

#[derive(Debug, Subcommand)]
pub enum BackupSubcommands {
    /// Exécute les sauvegardes définies dans la configuration.
    Run {
        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Profil spécifique à exécuter (exécute tous les profils actifs si omis).
        #[arg(short, long)]
        profile: Option<String>,

        /// Simule l'exécution sans modifier les fichiers ni lancer les processus lourds.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Liste toutes les sauvegardes disponibles dans le stockage.
    List {
        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Filtrer par nom de profil.
        #[arg(short, long)]
        profile: Option<String>,
    },
    /// Affiche les détails et métadonnées d'une sauvegarde.
    Info {
        /// Identifiant unique ou nom de fichier de la sauvegarde.
        backup_id: String,

        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
    /// Vérifie l'intégrité physique et la somme SHA-256 d'une sauvegarde.
    Verify {
        /// Identifiant de la sauvegarde à vérifier.
        backup_id: String,

        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,
    },
}

/// Groupe de commandes pour `restore`.
///
/// Sans sous-commande, restaure des applications d'un serveur depuis S3 vers cette machine :
/// `infra-manage restore --server s001 --application authentik` (ou `--application all`).
#[derive(Debug, Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct RestoreCommandGroup {
    #[command(subcommand)]
    pub command: Option<RestoreSubcommands>,

    /// Serveur d'origine des sauvegardes sur S3 (ex: s001, s002).
    #[arg(long)]
    pub server: Option<String>,

    /// Application à restaurer : un nom, une liste « a,b » ou `all` (toutes).
    #[arg(long, alias = "app")]
    pub application: Option<String>,

    /// Restaure l'état le plus récent antérieur à cette date UTC
    /// (YYYY-MM-DD = fin de journée, YYYY-MM-DD_HH-MM-SS ou RFC 3339).
    #[arg(long)]
    pub before: Option<String>,

    /// Simulation : affiche ce qui serait restauré sans rien télécharger ni modifier.
    #[arg(long, default_value_t = false)]
    pub dry_run: bool,

    /// Autorise l'écrasement d'une application déjà présente sur cette machine.
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Lance `docker compose up -d` pour l'application avant de restaurer ses bases de données.
    #[arg(long, default_value_t = false)]
    pub start: bool,

    /// Avec `--application all` : restaure aussi les fichiers système du snapshot (/etc, crontab...).
    #[arg(long, default_value_t = false)]
    pub include_system: bool,

    /// Dossier de téléchargement temporaire (défaut : <backup_directory>/.restore-<serveur>).
    #[arg(long)]
    pub work_dir: Option<PathBuf>,

    /// Conserve les fichiers téléchargés après la restauration.
    #[arg(long, default_value_t = false)]
    pub keep_downloads: bool,
}

#[derive(Debug, Subcommand)]
pub enum RestoreSubcommands {
    /// Liste, pour un serveur, les applications et la dernière sauvegarde disponible sur S3.
    List {
        /// Serveur d'origine des sauvegardes (ex: s001).
        #[arg(long)]
        server: String,
    },

    /// Restaure une sauvegarde locale (complète ou partielle) identifiée par son ID.
    Run {
        /// Identifiant de la sauvegarde à restaurer.
        backup_id: String,

        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Répertoire cible de restauration (pour profil filesystem).
        #[arg(short, long)]
        target: Option<PathBuf>,

        /// Nom de la base de données de destination (pour profil Postgres/MySQL/MariaDB/MongoDB).
        #[arg(long)]
        dest_db: Option<String>,

        /// Chemin partiel ou sous-dossier spécifique à extraire.
        #[arg(long)]
        partial: Option<String>,

        /// Force l'écrasement de données existantes sans confirmation.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

/// Groupe de sous-commandes pour `retention`.
#[derive(Debug, Args)]
pub struct RetentionCommandGroup {
    #[command(subcommand)]
    pub command: RetentionSubcommands,
}

#[derive(Debug, Subcommand)]
pub enum RetentionSubcommands {
    /// Applique les politiques de rétention et purge les anciennes archives.
    Run {
        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Profil spécifique sur lequel appliquer la rétention.
        #[arg(short, long)]
        profile: Option<String>,

        /// Affiche les suppressions qui auraient lieu sans supprimer aucun fichier.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

/// Arguments pour `history`.
#[derive(Debug, Args)]
pub struct HistoryArgs {
    /// Fichier de configuration YAML.
    #[arg(short, long)]
    pub config: Option<PathBuf>,

    /// Limite du nombre d'entrées affichées.
    #[arg(short, long, default_value_t = 20)]
    pub limit: usize,
}

/// Groupe de sous-commandes pour `scheduler`.
#[derive(Debug, Args)]
pub struct SchedulerCommandGroup {
    #[command(subcommand)]
    pub command: SchedulerSubcommands,
}

#[derive(Debug, Subcommand)]
pub enum SchedulerSubcommands {
    /// Génère le contenu d'un fichier crontab système à partir des profils.
    Crontab {
        /// Fichier de configuration YAML.
        #[arg(short, long)]
        config: Option<PathBuf>,

        /// Chemin absolu du binaire infra-manage utilisé dans la crontab.
        #[arg(long)]
        binary_path: Option<String>,
    },
}

/// Groupe de sous-commandes pour `notify`.
#[derive(Debug, Args)]
pub struct NotifyCommandGroup {
    #[command(subcommand)]
    pub command: NotifySubcommands,
}

#[derive(Debug, Subcommand)]
pub enum NotifySubcommands {
    /// Envoie une notification de test via le webhook configuré.
    Test {
        /// Message à envoyer.
        #[arg(short, long, default_value = "Test de notification infra-manage")]
        message: String,
    },
}
