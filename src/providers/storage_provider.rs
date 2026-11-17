//! Adaptateur de stockage local et distant (manifeste, hash, rclone/S3).

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use tracing::{debug, info};

use crate::core::abstract_traits::{CommandExecutor, StorageEngine};
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{BackupMetadata, BackupStatus, CompressionAlgorithm, ProfileType};

/// Nom conventionnel du fichier d'index des métadonnées de sauvegarde.
pub const MANIFEST_FILENAME: &str = "manifest.json";

/// Implémentation du moteur de stockage avec gestion d'index local et synchronisation distante.
pub struct DiskStorageEngine<'a> {
    executor: &'a dyn CommandExecutor,
}

impl<'a> DiskStorageEngine<'a> {
    /// Crée un nouveau moteur de stockage lié à un exécuteur de commandes pour rclone.
    pub fn new(executor: &'a dyn CommandExecutor) -> Self {
        Self { executor }
    }

    /// Charge ou initialise l'index de métadonnées situé dans le dossier de sauvegarde.
    fn load_manifest(&self, base_dir: &Path) -> DomainResult<Vec<BackupMetadata>> {
        let manifest_path = base_dir.join(MANIFEST_FILENAME);
        if !manifest_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&manifest_path)?;
        let reader = BufReader::new(file);
        let list: Vec<BackupMetadata> = serde_json::from_reader(reader).unwrap_or_default();
        Ok(list)
    }

    /// Écrit l'index de métadonnées sur le disque de façon atomique.
    fn write_manifest(&self, base_dir: &Path, list: &[BackupMetadata]) -> DomainResult<()> {
        let manifest_path = base_dir.join(MANIFEST_FILENAME);
        let temp_path = base_dir.join(format!("{}.part", MANIFEST_FILENAME));

        let file = File::create(&temp_path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, list)?;

        fs::rename(temp_path, manifest_path)?;
        Ok(())
    }
}

impl<'a> StorageEngine for DiskStorageEngine<'a> {
    fn save_metadata(&self, base_dir: &Path, metadata: &BackupMetadata) -> DomainResult<()> {
        debug!(
            "[LOG STORAGE] Enregistrement des métadonnées de la sauvegarde '{}'",
            metadata.id
        );

        fs::create_dir_all(base_dir).map_err(|e| {
            DomainError::Storage(format!(
                "Impossible de créer le dossier {:?} : {}",
                base_dir, e
            ))
        })?;

        let mut list = self.load_manifest(base_dir)?;

        // Remplacement ou ajout
        if let Some(pos) = list.iter().position(|m| m.id == metadata.id) {
            list[pos] = metadata.clone();
        } else {
            list.push(metadata.clone());
        }

        self.write_manifest(base_dir, &list)?;

        // Écriture du fichier compagnon .sha256 à côté de l'archive (standard Unix)
        let sha_path = PathBuf::from(format!("{}.sha256", metadata.file_path.display()));
        if let Some(file_name) = metadata.file_path.file_name() {
            let content = format!("{}  {}\n", metadata.checksum_sha256, file_name.to_string_lossy());
            let mut sha_file = File::create(&sha_path)?;
            sha_file.write_all(content.as_bytes())?;
            debug!(
                "[LOG STORAGE] Fichier d'empreinte généré : {:?}",
                sha_path
            );
        }

        Ok(())
    }

    fn list_backups(&self, base_dir: &Path, profile: Option<&str>) -> DomainResult<Vec<BackupMetadata>> {
        if !base_dir.exists() {
            return Ok(Vec::new());
        }

        let mut list = self.load_manifest(base_dir)?;

        // Si l'index est vide ou inexistant, on reconstruit dynamiquement depuis le dossier
        if list.is_empty() {
            debug!("[LOG STORAGE] Index manifest vide, analyse directe du dossier");
            if let Ok(entries) = fs::read_dir(base_dir) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    let file_name = path
                        .file_name()
                        .map(|f| f.to_string_lossy().to_string())
                        .unwrap_or_default();

                    if file_name.ends_with(".part")
                        || file_name.ends_with(".sha256")
                        || file_name == MANIFEST_FILENAME
                    {
                        continue;
                    }

                    if path.is_file() {
                        let id = file_name
                            .split('.')
                            .next()
                            .unwrap_or(&file_name)
                            .to_string();

                        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                        let created_at: DateTime<Utc> = entry
                            .metadata()
                            .and_then(|m| m.created().or_else(|_| m.modified()))
                            .map(DateTime::from)
                            .unwrap_or_else(|_| Utc::now());

                        // Lecture du .sha256 compagnon si disponible
                        let sha_companion = PathBuf::from(format!("{}.sha256", path.display()));
                        let hash = if sha_companion.exists() {
                            fs::read_to_string(&sha_companion)
                                .ok()
                                .and_then(|c| c.split_whitespace().next().map(|s| s.to_string()))
                                .unwrap_or_default()
                        } else {
                            String::new()
                        };

                        let guessed_type = if file_name.ends_with(".dump") {
                            ProfileType::Postgres
                        } else if file_name.contains(".sql") {
                            ProfileType::Mysql
                        } else {
                            ProfileType::Filesystem
                        };

                        let item = BackupMetadata {
                            id: id.clone(),
                            profile_name: id,
                            profile_type: guessed_type,
                            created_at,
                            file_path: path,
                            file_size: size,
                            checksum_sha256: hash,
                            compression: CompressionAlgorithm::None,
                            status: BackupStatus::Success,
                            duration_seconds: 0.0,
                        };
                        list.push(item);
                    }
                }
            }
        }

        if let Some(prof) = profile {
            list.retain(|m| m.profile_name.eq_ignore_ascii_case(prof));
        }

        list.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(list)
    }

    fn find_backup(&self, base_dir: &Path, id: &str) -> DomainResult<Option<BackupMetadata>> {
        let all = self.list_backups(base_dir, None)?;
        for item in all {
            if item.id == id
                || item
                    .file_path
                    .file_name()
                    .map(|f| f.to_string_lossy() == id)
                    .unwrap_or(false)
            {
                return Ok(Some(item));
            }
        }
        Ok(None)
    }

    fn delete_backup(&self, base_dir: &Path, metadata: &BackupMetadata) -> DomainResult<()> {
        info!(
            "[LOG STORAGE] Suppression physique de la sauvegarde '{}' ({:?})",
            metadata.id, metadata.file_path
        );

        if metadata.file_path.exists() {
            let _ = fs::remove_file(&metadata.file_path);
        }

        let sha_path = PathBuf::from(format!("{}.sha256", metadata.file_path.display()));
        if sha_path.exists() {
            let _ = fs::remove_file(sha_path);
        }

        let mut list = self.load_manifest(base_dir)?;
        list.retain(|m| m.id != metadata.id);
        self.write_manifest(base_dir, &list)?;

        Ok(())
    }

    fn upload_to_remote(
        &self,
        local_path: &Path,
        remote_target: &str,
        is_sync: bool,
        timeout: &str,
        extra_args: &[String],
    ) -> DomainResult<()> {
        let local_str = local_path.to_string_lossy().to_string();
        info!(
            "[LOG STORAGE] Envoi distant rclone vers '{}' (mode: {}, timeout: {})",
            remote_target,
            if is_sync { "sync" } else { "copyto" },
            timeout
        );

        let cmd_mode = if is_sync { "sync" } else { "copyto" };

        let mut args = vec![
            "-k",
            "60",
            timeout,
            "rclone",
            cmd_mode,
            &local_str,
            remote_target,
            "--contimeout",
            "10s",
            "--timeout",
            "120s",
            "--retries",
            "3",
            "--low-level-retries",
            "10",
            "--s3-upload-concurrency",
            "4",
            "--s3-chunk-size",
            "32M",
        ];

        for extra in extra_args {
            args.push(extra.as_str());
        }

        let output = self.executor.execute("timeout", &args, &[])?;

        if !output.success {
            let err_msg = if output.exit_code == 124 {
                format!(
                    "Dépassement du plafond de durée ({}) lors de l'envoi rclone",
                    timeout
                )
            } else {
                format!("Échec rclone (code {}) : {}", output.exit_code, output.stderr)
            };
            return Err(DomainError::Storage(err_msg));
        }

        info!("[LOG STORAGE] Envoi distant rclone terminé avec succès.");
        Ok(())
    }
}
