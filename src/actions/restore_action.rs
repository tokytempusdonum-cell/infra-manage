//! Action de restauration sécurisée et vérifiée de sauvegardes existantes.

use std::fs;
use std::path::{Path, PathBuf};
use tracing::{error, info};

use crate::actions::history_action::HistoryAction;
use crate::actions::validation_action::VerifyBackupAction;
use crate::core::abstract_traits::{CommandExecutor, StorageEngine};
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{AppConfig, BackupMetadata, ProfileType};
use crate::providers::database_provider::DatabaseProvider;
use crate::providers::filesystem_provider::FilesystemProvider;
use crate::providers::restic_provider::ResticProvider;

/// Action d'exécution d'une restauration contrôlée.
pub struct RestoreAction<'a> {
    executor: &'a dyn CommandExecutor,
    storage: &'a dyn StorageEngine,
}

impl<'a> RestoreAction<'a> {
    /// Initialise l'action de restauration.
    pub fn new(executor: &'a dyn CommandExecutor, storage: &'a dyn StorageEngine) -> Self {
        Self { executor, storage }
    }

    /// Exécute la restauration d'une sauvegarde identifiée.
    pub fn execute(
        &self,
        config: &AppConfig,
        backup_id: &str,
        target_override: Option<&Path>,
        dest_db_override: Option<&str>,
        partial_filter: Option<&str>,
        force: bool,
    ) -> DomainResult<()> {
        info!(
            "[LOG ACTION] Début de la restauration de la sauvegarde '{}'",
            backup_id
        );

        // 1. Recherche de la sauvegarde dans le stockage
        let mut found: Option<(PathBuf, BackupMetadata)> = None;

        // Recherche d'abord dans le répertoire racine puis dans les profils
        let mut dirs_to_search = vec![config.settings.backup_directory.clone()];
        for p in &config.profiles {
            if let Some(dest) = &p.destination.path {
                if !dirs_to_search.contains(dest) {
                    dirs_to_search.push(dest.clone());
                }
            }
        }

        for dir in dirs_to_search {
            if let Ok(Some(meta)) = self.storage.find_backup(&dir, backup_id) {
                found = Some((dir, meta));
                break;
            }
        }

        let (base_dir, metadata) = match found {
            Some(res) => res,
            None => {
                return Err(DomainError::Storage(format!(
                    "Sauvegarde introuvable pour l'identifiant '{}'",
                    backup_id
                )));
            }
        };

        // 2. Recherche du profil associé
        let profile = config
            .profiles
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(&metadata.profile_name))
            .ok_or_else(|| {
                DomainError::Config(format!(
                    "Aucun profil configuré ne correspond à la sauvegarde '{}' (profil : '{}')",
                    backup_id, metadata.profile_name
                ))
            })?;

        // 3. Vérification préalable de l'intégrité
        info!("[LOG RESTORE] Contrôle d'intégrité avant restauration...");
        let verify_action = VerifyBackupAction::new(self.storage);
        verify_action.execute(&base_dir, backup_id)?;

        // 4. Exécution selon le type avec vérification anti-écrasement
        let res = match metadata.profile_type {
            ProfileType::Postgres => {
                let db_provider = DatabaseProvider::new(self.executor);
                db_provider.restore_postgres(profile, &metadata.file_path, dest_db_override)
            }
            ProfileType::Mysql => {
                let db_provider = DatabaseProvider::new(self.executor);
                db_provider.restore_mysql(profile, &metadata.file_path, dest_db_override)
            }
            ProfileType::Mariadb => {
                let db_provider = DatabaseProvider::new(self.executor);
                db_provider.restore_mariadb(profile, &metadata.file_path, dest_db_override)
            }
            ProfileType::Mongo => {
                let db_provider = DatabaseProvider::new(self.executor);
                db_provider.restore_mongo(profile, &metadata.file_path, dest_db_override)
            }
            ProfileType::Filesystem => {
                let default_target = profile
                    .source
                    .paths
                    .as_ref()
                    .and_then(|p| p.first())
                    .cloned()
                    .unwrap_or_else(|| PathBuf::from("/tmp/restore"));

                let target_dir = target_override.unwrap_or(&default_target);

                // Sécurité anti-écrasement : si le dossier existe et n'est pas vide
                if target_dir.exists() && !force {
                    if let Ok(mut entries) = fs::read_dir(target_dir) {
                        if entries.next().is_some() {
                            return Err(DomainError::RestoreSafety {
                                target: target_dir.display().to_string(),
                            });
                        }
                    }
                }

                let fs_provider = FilesystemProvider::new();
                fs_provider
                    .extract_archive(
                        &metadata.file_path,
                        target_dir,
                        metadata.compression,
                        partial_filter,
                    )
                    .map(|_| ())
            }
            ProfileType::Restic => {
                let default_target = PathBuf::from("/tmp/restore-restic");
                let target_dir = target_override.unwrap_or(&default_target);
                let restic_provider = ResticProvider::new(self.executor);
                restic_provider.restore(profile, backup_id, target_dir)
            }
        };

        if let Err(ref e) = res {
            error!("[LOG RESTORE] Échec de la restauration : {}", e);
            HistoryAction::record(
                &base_dir,
                "restore",
                Some(&profile.name),
                Some(backup_id),
                "FAILURE",
                &e.to_string(),
            )?;
            return res;
        }

        info!(
            "[LOG RESTORE] Restauration terminée avec succès pour '{}'",
            backup_id
        );
        HistoryAction::record(
            &base_dir,
            "restore",
            Some(&profile.name),
            Some(backup_id),
            "SUCCESS",
            "Restauration effectuée",
        )?;

        Ok(())
    }
}
