//! Ports et interfaces abstraites du domaine.
//!
//! Ce module définit les contrats d'interaction avec le monde extérieur
//! (exécuteur de commandes, moteur de stockage, notification, verrouillage)
//! sans aucune dépendance envers les bibliothèques tierces d'implémentation.

use std::path::Path;
use super::exceptions::DomainResult;
use super::model::BackupMetadata;

/// Résultat d'exécution d'une commande système.
#[derive(Debug, Clone)]
pub struct ExecutionOutput {
    /// Sortie standard produite par le processus.
    pub stdout: String,
    /// Sortie d'erreur standard produite par le processus.
    pub stderr: String,
    /// Code de retour du processus.
    pub exit_code: i32,
    /// Vrai si le code de retour vaut zéro.
    pub success: bool,
}

/// Port d'exécution de commandes système et de vérifications de conteneurs.
pub trait CommandExecutor: Send + Sync {
    /// Exécute une commande avec ses arguments et son environnement.
    fn execute(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> DomainResult<ExecutionOutput>;

    /// Exécute une commande en redirigeant son flux de sortie standard vers un fichier.
    fn execute_to_file(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        output_file: &Path,
    ) -> DomainResult<u64>;

    /// Exécute une commande en lui fournissant un fichier sur son flux d'entrée standard (stdin).
    fn execute_with_stdin(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        stdin_file: &Path,
    ) -> DomainResult<ExecutionOutput>;

    /// Vérifie si un conteneur Docker donné est actuellement en cours d'exécution.
    fn is_container_running(&self, container: &str) -> DomainResult<bool>;
}

/// Port de gestion des métadonnées et du stockage physique des sauvegardes.
pub trait StorageEngine: Send + Sync {
    /// Enregistre les métadonnées d'une nouvelle sauvegarde dans l'index du répertoire.
    fn save_metadata(&self, base_dir: &Path, metadata: &BackupMetadata) -> DomainResult<()>;

    /// Liste toutes les sauvegardes présentes dans le répertoire, avec filtre optionnel par profil.
    fn list_backups(&self, base_dir: &Path, profile: Option<&str>) -> DomainResult<Vec<BackupMetadata>>;

    /// Recherche une sauvegarde par son identifiant ou par nom de fichier.
    fn find_backup(&self, base_dir: &Path, id: &str) -> DomainResult<Option<BackupMetadata>>;

    /// Supprime définitivement les fichiers associés à une sauvegarde (archive, .sha256).
    fn delete_backup(&self, base_dir: &Path, metadata: &BackupMetadata) -> DomainResult<()>;

    /// Synchronise un fichier ou dossier local vers une cible distante (rclone).
    fn upload_to_remote(
        &self,
        local_path: &Path,
        remote_target: &str,
        is_sync: bool,
        timeout: &str,
        extra_args: &[String],
    ) -> DomainResult<()>;
}

/// Port d'émission de notifications (Slack / webhook).
pub trait NotificationService: Send + Sync {
    /// Émet un message de notification à destination des canaux configurés.
    fn notify(&self, message: &str) -> DomainResult<()>;
}

/// Garde RAII garantissant la libération du verrou de concurrence à sa destruction.
pub trait LockGuard: Send + Sync {}

/// Port d'acquisition de verrous exclusifs pour la protection contre la concurrence.
pub trait LockService: Send + Sync {
    /// Tente d'acquérir un verrou exclusif sur le fichier spécifié.
    fn acquire_lock(&self, lock_path: &Path) -> DomainResult<Box<dyn LockGuard>>;
}
