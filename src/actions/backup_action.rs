//! Action principale d'orchestration et d'exécution des sauvegardes.

use std::fs;
use std::time::Instant;
use chrono::Utc;
use tracing::{error, info, warn};

use crate::actions::history_action::HistoryAction;
use crate::actions::retention_action::RetentionAction;
use crate::actions::validation_action::VerifyBackupAction;
use crate::core::abstract_traits::{CommandExecutor, LockService, NotificationService, StorageEngine};
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{
    AppConfig, BackupMetadata, BackupProfile, BackupStatus, CompressionAlgorithm, ProfileType,
    BackupReport, ProfileReport,
};
use crate::providers::database_provider::DatabaseProvider;
use crate::providers::docker_provider::DockerProvider;
use crate::providers::filesystem_provider::FilesystemProvider;
use crate::providers::restic_provider::ResticProvider;
use crate::providers::stream_compression::compress_file;

/// Résultat d'exécution d'un profil, remonté dans le bilan de notification.
#[derive(Debug, Default)]
struct ProfileOutcome {
    size_bytes: Option<u64>,
    detail: Option<String>,
}

/// Action d'exécution d'un cycle de sauvegarde complet ou ciblé.
pub struct BackupAction<'a> {
    executor: &'a dyn CommandExecutor,
    storage: &'a dyn StorageEngine,
    lock_service: &'a dyn LockService,
    notifier: &'a dyn NotificationService,
}

impl<'a> BackupAction<'a> {
    /// Initialise l'action avec l'ensemble des dépendances injectées.
    pub fn new(
        executor: &'a dyn CommandExecutor,
        storage: &'a dyn StorageEngine,
        lock_service: &'a dyn LockService,
        notifier: &'a dyn NotificationService,
    ) -> Self {
        Self {
            executor,
            storage,
            lock_service,
            notifier,
        }
    }

    /// Exécute les sauvegardes pour les profils configurés.
    pub fn execute(
        &self,
        config: &AppConfig,
        profile_filter: Option<&str>,
        dry_run: bool,
    ) -> DomainResult<usize> {
        info!("[LOG ACTION] === Début du cycle de sauvegarde ===");

        let cycle_start = Instant::now();
        let started_at = Utc::now();
        let mut reports: Vec<ProfileReport> = Vec::new();
        let mut successful = 0;
        let mut failed = 0;

        let profiles_to_run: Vec<&BackupProfile> = config
            .profiles
            .iter()
            .filter(|p| {
                if let Some(target) = profile_filter {
                    p.name.eq_ignore_ascii_case(target)
                } else {
                    p.enabled
                }
            })
            .collect();

        if profiles_to_run.is_empty() {
            warn!("[LOG ACTION] Aucun profil actif à exécuter.");
            return Ok(0);
        }

        for profile in &profiles_to_run {
            info!("[LOG ACTION] Lancement du profil : '{}'", profile.name);
            let profile_start = Instant::now();

            // Verrou propre au profil : un même profil ne peut pas tourner deux fois en même temps,
            // mais des profils différents peuvent s'exécuter en parallèle (processus distincts).
            let _profile_guard = if dry_run {
                None
            } else {
                match self.lock_service.acquire_lock(&config.profile_lock_path(profile)) {
                    Ok(guard) => Some(guard),
                    Err(e) => {
                        error!(
                            "[LOG ACTION] ÉCHEC : Profil '{}' déjà en cours d'exécution : {}",
                            profile.name, e
                        );
                        failed += 1;
                        reports.push(ProfileReport {
                            name: profile.name.clone(),
                            profile_type: profile.profile_type,
                            status: BackupStatus::Failed,
                            size_bytes: None,
                            duration_seconds: profile_start.elapsed().as_secs_f64(),
                            detail: Some(format!(
                                "ce profil est déjà en cours d'exécution (verrou occupé) : {}",
                                e
                            )),
                        });
                        continue;
                    }
                }
            };

            match self.execute_profile(config, profile, dry_run) {
                Ok(outcome) => {
                    info!("[LOG ACTION] OK : Profil '{}' terminé avec succès", profile.name);
                    successful += 1;
                    reports.push(ProfileReport {
                        name: profile.name.clone(),
                        profile_type: profile.profile_type,
                        status: BackupStatus::Success,
                        size_bytes: outcome.size_bytes,
                        duration_seconds: profile_start.elapsed().as_secs_f64(),
                        detail: outcome.detail,
                    });
                }
                Err(e) => {
                    error!(
                        "[LOG ACTION] ÉCHEC : Profil '{}' a échoué : {}",
                        profile.name, e
                    );
                    failed += 1;
                    reports.push(ProfileReport {
                        name: profile.name.clone(),
                        profile_type: profile.profile_type,
                        status: BackupStatus::Failed,
                        size_bytes: None,
                        duration_seconds: profile_start.elapsed().as_secs_f64(),
                        detail: Some(e.to_string()),
                    });
                }
            }
        }

        let total = successful + failed;
        let summary = format!(
            "Cycle de sauvegarde terminé : {}/{} réussi(s), {} échec(s)",
            successful, total, failed
        );

        if failed == 0 {
            info!("[LOG ACTION] === {} ===", summary);
        } else {
            error!("[LOG ACTION] === {} ===", summary);
        }

        // Bilan détaillé (total, réussies, échecs, détail par profil) — pas en simulation.
        if !dry_run {
            let report = BackupReport {
                hostname: local_hostname(),
                started_at,
                duration_seconds: cycle_start.elapsed().as_secs_f64(),
                profiles: reports,
            };
            if let Err(e) = self.notifier.notify_report(&report) {
                warn!("[LOG ACTION] Notification non envoyée : {}", e);
            }
        }

        if failed > 0 {
            Err(DomainError::Execution {
                command: "backup cycle".to_string(),
                message: summary,
                exit_code: Some(1),
            })
        } else {
            Ok(successful)
        }
    }

    /// Exécute la sauvegarde d'un profil individuel.
    fn execute_profile(
        &self,
        config: &AppConfig,
        profile: &BackupProfile,
        dry_run: bool,
    ) -> DomainResult<ProfileOutcome> {
        let start_time = Instant::now();
        let base_dir = config.resolve_backup_dir(profile);
        fs::create_dir_all(base_dir)?;

        let timestamp_str = Utc::now().format("%Y-%m-%d_%H-%M-%S").to_string();
        let backup_id = format!("{}_{}_backup", timestamp_str, profile.name);

        if dry_run {
            info!(
                "[LOG ACTION] [SIMULATION] Sauvegarde simulée pour '{}' (id: {})",
                profile.name, backup_id
            );
            return Ok(ProfileOutcome::default());
        }

        // 1. Exécution des pré-commandes éventuelles
        if let Some(pre_cmds) = &profile.source.pre_commands {
            for cmd_str in pre_cmds {
                info!(
                    "[LOG ACTION] Exécution de la pré-commande : '{}'",
                    cmd_str
                );
                let output = self.executor.execute("sh", &["-c", cmd_str], &[])?;
                if !output.success {
                    return Err(DomainError::Execution {
                        command: cmd_str.clone(),
                        message: format!("Échec de la pré-commande : {}", output.stderr),
                        exit_code: Some(output.exit_code),
                    });
                }
            }
        }

        // 2. Vérification de l'état du conteneur Docker si applicable
        if let Some(container) = &profile.source.container_name {
            let docker = DockerProvider::new(self.executor);
            docker.ensure_container_running(container)?;
        }

        // 3. Détermination de l'extension et des chemins de fichiers
        let (extension, compression_algo) = match profile.profile_type {
            ProfileType::Postgres => ("dump", CompressionAlgorithm::None),
            ProfileType::Mysql | ProfileType::Mariadb => {
                let algo = profile.sql_compression();
                let ext = match algo {
                    CompressionAlgorithm::Gzip => "sql.gz",
                    CompressionAlgorithm::Zstd => "sql.zst",
                    CompressionAlgorithm::None => "sql",
                };
                (ext, algo)
            }
            // mongodump --archive --gzip produit nativement une archive gzip.
            ProfileType::Mongo => {
                if profile.mongo_uses_gzip() {
                    ("archive.gz", CompressionAlgorithm::Gzip)
                } else {
                    ("archive", CompressionAlgorithm::None)
                }
            }
            ProfileType::Filesystem => {
                let algo = profile
                    .compression
                    .as_ref()
                    .filter(|c| c.enabled)
                    .map(|c| c.algorithm)
                    .unwrap_or(CompressionAlgorithm::None);
                let ext = match algo {
                    CompressionAlgorithm::Gzip => "tar.gz",
                    CompressionAlgorithm::Zstd => "tar.zst",
                    CompressionAlgorithm::None => "tar",
                };
                (ext, algo)
            }
            ProfileType::Restic => ("restic", CompressionAlgorithm::None),
        };

        // Cas particulier Restic
        if profile.profile_type == ProfileType::Restic {
            let restic = ResticProvider::new(self.executor);
            restic.backup(profile)?;

            HistoryAction::record(
                base_dir,
                "backup",
                Some(&profile.name),
                Some(&backup_id),
                "SUCCESS",
                "Snapshot Restic créé",
            )?;

            // Synchronisation distante du dépôt restic si remote configuré
            if let Some(remote) = &profile.destination.remote {
                let timeout = config
                    .settings
                    .storage
                    .as_ref()
                    .map(|s| s.config_max_duration.as_str())
                    .unwrap_or("12h");
                let is_sync = profile.destination.remote_sync.unwrap_or(true);
                let mut extra = config
                    .settings
                    .storage
                    .as_ref()
                    .map(|s| s.rclone_args())
                    .unwrap_or_default();
                // Les filtres propres au profil (ex: exclusion des verrous restic)
                // ne s'appliquent qu'au `sync` récursif, jamais au `copyto`.
                if is_sync {
                    if let Some(profile_extra) = &profile.destination.extra_args {
                        extra.extend(profile_extra.iter().cloned());
                    }
                }
                self.storage
                    .upload_to_remote(base_dir, remote, is_sync, timeout, &extra)?;
            }

            return Ok(ProfileOutcome {
                size_bytes: None,
                detail: Some("snapshot restic créé".to_string()),
            });
        }

        let file_name = format!("{}.{}", backup_id, extension);
        let final_path = base_dir.join(&file_name);
        let tmp_path = base_dir.join(format!("{}.part", file_name));

        // 4. Génération du flux de sauvegarde vers le fichier temporaire
        let db_provider = DatabaseProvider::new(self.executor);
        let fs_provider = FilesystemProvider::new();

        let res = match profile.profile_type {
            ProfileType::Postgres => db_provider.dump_postgres(profile, &tmp_path),
            ProfileType::Mysql | ProfileType::Mariadb => {
                let dump = |out: &std::path::Path| {
                    if profile.profile_type == ProfileType::Mariadb {
                        db_provider.dump_mariadb(profile, out)
                    } else {
                        db_provider.dump_mysql(profile, out)
                    }
                };
                if compression_algo == CompressionAlgorithm::None {
                    dump(&tmp_path)
                } else {
                    // Le dump est écrit en clair puis réellement compressé (gzip / zstd).
                    let raw_path = base_dir.join(format!("{}.raw.part", file_name));
                    let level = profile.compression.as_ref().and_then(|c| c.level);
                    let result = dump(&raw_path).and_then(|_| {
                        compress_file(&raw_path, &tmp_path, compression_algo, level)
                    });
                    let _ = fs::remove_file(&raw_path);
                    result
                }
            }
            ProfileType::Mongo => db_provider.dump_mongo(profile, &tmp_path),
            ProfileType::Filesystem => {
                let paths = profile.source.paths.as_deref().unwrap_or(&[]);
                let excludes = profile.source.exclude.as_deref().unwrap_or(&[]);
                fs_provider.create_archive(
                    paths,
                    &tmp_path,
                    excludes,
                    profile.compression.as_ref(),
                )
            }
            ProfileType::Restic => unreachable!(),
        };

        if let Err(e) = res {
            let _ = fs::remove_file(&tmp_path);
            HistoryAction::record(
                base_dir,
                "backup",
                Some(&profile.name),
                Some(&backup_id),
                "FAILURE",
                &e.to_string(),
            )?;
            return Err(e);
        }

        // 5. Renommage atomique
        fs::rename(&tmp_path, &final_path)?;

        let file_size = fs::metadata(&final_path)?.len();
        if file_size == 0 {
            let _ = fs::remove_file(&final_path);
            return Err(DomainError::Integrity {
                resource: backup_id,
                message: "Fichier de sauvegarde généré de taille nulle".to_string(),
            });
        }

        // 6. Calcul de l'empreinte SHA-256
        let hash = VerifyBackupAction::compute_sha256(&final_path)?;
        info!("[LOG ACTION] Empreinte SHA-256 : {}", hash);

        // 7. Enregistrement des métadonnées
        let duration = start_time.elapsed().as_secs_f64();
        let metadata = BackupMetadata {
            id: backup_id.clone(),
            profile_name: profile.name.clone(),
            profile_type: profile.profile_type,
            created_at: Utc::now(),
            file_path: final_path.clone(),
            file_size,
            checksum_sha256: hash,
            compression: compression_algo,
            status: BackupStatus::Success,
            duration_seconds: duration,
        };

        self.storage.save_metadata(base_dir, &metadata)?;

        HistoryAction::record(
            base_dir,
            "backup",
            Some(&profile.name),
            Some(&backup_id),
            "SUCCESS",
            &format!("Sauvegarde créée ({} octets, {:.2}s)", file_size, duration),
        )?;

        // 8. Rotation locale si rétention configurée
        if profile.retention.is_some() {
            let retention_action = RetentionAction::new(self.storage, None);
            let _ = retention_action.execute(config, Some(&profile.name), false);
        }

        // 9. Envoi vers le stockage distant si configuré
        if let Some(ref storage_cfg) = config.settings.storage {
            let remote_dest = profile
                .destination
                .remote
                .as_deref()
                .unwrap_or(&storage_cfg.remote);

            let remote_file = format!("{}/{}", remote_dest, file_name);
            let timeout = &storage_cfg.upload_max_duration;
            // `copyto` porte sur un fichier unique : rclone interdit d'y ajouter des
            // filtres (`--exclude`...). On ne transmet donc que les arguments de base.
            let extra = storage_cfg.rclone_base_args();

            if let Err(e) = self
                .storage
                .upload_to_remote(&final_path, &remote_file, false, timeout, &extra)
            {
                warn!(
                    "[LOG ACTION] Avertissement : échec de l'envoi distant rclone : {}",
                    e
                );
            }
        }

        // 10. Exécution des post-commandes éventuelles
        if let Some(post_cmds) = &profile.source.post_commands {
            for cmd_str in post_cmds {
                let _ = self.executor.execute("sh", &["-c", cmd_str], &[]);
            }
        }

        Ok(ProfileOutcome {
            size_bytes: Some(file_size),
            detail: None,
        })
    }
}

/// Nom de la machine locale (pour identifier le serveur dans les notifications).
fn local_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok().filter(|h| !h.is_empty()))
        .unwrap_or_else(|| "serveur inconnu".to_string())
}
