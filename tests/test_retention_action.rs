//! Tests unitaires de la politique de rétention et de rotation des archives.

use std::fs;
use std::path::PathBuf;
use chrono::{Duration, Utc};

mod common;
use common::TestEnvironment;

use backup_cli::actions::retention_action::RetentionAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::model::{BackupMetadata, BackupStatus, CompressionAlgorithm, ProfileType, RetentionPolicy};

#[test]
fn test_retention_purges_files_older_than_days() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let now = Utc::now();
    let old_date = now - Duration::days(20); // Plus vieux que les 14 jours de rétention
    let recent_date = now - Duration::days(2);

    let old_file = env.backup_dir.join("old_authentik_backup.dump");
    fs::write(&old_file, "dump ancien").unwrap();
    let old_meta = BackupMetadata {
        id: "old_authentik_backup".to_string(),
        profile_name: "authentik".to_string(),
        profile_type: ProfileType::Postgres,
        created_at: old_date,
        file_path: old_file.clone(),
        file_size: 10,
        checksum_sha256: "fakehash".to_string(),
        compression: CompressionAlgorithm::None,
        status: BackupStatus::Success,
        duration_seconds: 1.0,
    };
    storage.save_metadata(&env.backup_dir, &old_meta).unwrap();

    let recent_file = env.backup_dir.join("recent_authentik_backup.dump");
    fs::write(&recent_file, "dump récent").unwrap();
    let recent_meta = BackupMetadata {
        id: "recent_authentik_backup".to_string(),
        profile_name: "authentik".to_string(),
        profile_type: ProfileType::Postgres,
        created_at: recent_date,
        file_path: recent_file.clone(),
        file_size: 10,
        checksum_sha256: "fakehash2".to_string(),
        compression: CompressionAlgorithm::None,
        status: BackupStatus::Success,
        duration_seconds: 1.0,
    };
    storage.save_metadata(&env.backup_dir, &recent_meta).unwrap();

    let action = RetentionAction::new(&storage, None);
    let deleted_count = action
        .execute(&config, Some("authentik"), false)
        .expect("Rétention échouée");

    assert_eq!(deleted_count, 1, "Une seule sauvegarde doit être purgée");
    assert!(!old_file.exists(), "Le fichier ancien aurait dû être supprimé");
    assert!(recent_file.exists(), "Le fichier récent doit être conservé");

    let backups = storage.list_backups(&env.backup_dir, Some("authentik")).unwrap();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].id, "recent_authentik_backup");
}

#[test]
fn test_retention_dry_run_preserves_all_files() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let old_date = Utc::now() - Duration::days(25);
    let old_file = env.backup_dir.join("dryrun_old_backup.dump");
    fs::write(&old_file, "ancien dump").unwrap();
    let old_meta = BackupMetadata {
        id: "dryrun_old_backup".to_string(),
        profile_name: "authentik".to_string(),
        profile_type: ProfileType::Postgres,
        created_at: old_date,
        file_path: old_file.clone(),
        file_size: 10,
        checksum_sha256: "hash".to_string(),
        compression: CompressionAlgorithm::None,
        status: BackupStatus::Success,
        duration_seconds: 1.0,
    };
    storage.save_metadata(&env.backup_dir, &old_meta).unwrap();

    let action = RetentionAction::new(&storage, None);
    let deleted_count = action
        .execute(&config, Some("authentik"), true) // dry_run = true
        .expect("Rétention dry-run échouée");

    assert_eq!(deleted_count, 1, "Une purge aurait dû être identifiée");
    assert!(old_file.exists(), "Le fichier physique ne doit PAS être supprimé en mode dry-run");
}

#[test]
fn test_retention_gfs_daily_weekly_monthly() {
    let policy = RetentionPolicy {
        days: None,
        daily: Some(3),
        weekly: Some(2),
        monthly: Some(1),
    };

    let now = Utc::now();
    let mut backups = Vec::new();

    // Génération de 10 sauvegardes étalées sur les 10 derniers jours
    for i in 0..10 {
        let b = BackupMetadata {
            id: format!("b_{}", i),
            profile_name: "test".to_string(),
            profile_type: ProfileType::Filesystem,
            created_at: now - Duration::days(i),
            file_path: PathBuf::from(format!("/backup/b_{}.tar", i)),
            file_size: 100,
            checksum_sha256: "h".to_string(),
            compression: CompressionAlgorithm::None,
            status: BackupStatus::Success,
            duration_seconds: 1.0,
        };
        backups.push(b);
    }

    let expired = RetentionAction::identify_expired_backups(&backups, &policy);
    // Au moins les sauvegardes les plus anciennes au-delà des quotas GFS doivent être marquées expirées
    assert!(!expired.is_empty());
    assert!(expired.iter().any(|b| b.id == "b_9"));
}
