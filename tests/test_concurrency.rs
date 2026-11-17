//! Tests de concurrence : verrou par profil et index partagé.

mod common;
use common::TestEnvironment;

use std::path::PathBuf;
use std::thread;

use backup_cli::actions::history_action::HistoryAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::model::{BackupMetadata, BackupStatus, CompressionAlgorithm, ProfileType};
use backup_cli::providers::storage_provider::DiskStorageEngine;
use chrono::Utc;

#[test]
fn test_profile_lock_paths_are_distinct_per_profile() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.settings.lock_file = PathBuf::from("/var/lock/backup-cli.lock");

    let a = config.profile_lock_path(&config.profiles[0]);
    let b = config.profile_lock_path(&config.profiles[1]);

    assert_ne!(a, b);
    assert_eq!(a, PathBuf::from("/var/lock/backup-cli.authentik.lock"));
    assert_eq!(b, PathBuf::from("/var/lock/backup-cli.ghost.lock"));
}

#[test]
fn test_profile_lock_path_sanitizes_name() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.settings.lock_file = PathBuf::from("/var/lock/backup-cli.lock");
    config.profiles[0].name = "a/b c".to_string();

    assert_eq!(
        config.profile_lock_path(&config.profiles[0]),
        PathBuf::from("/var/lock/backup-cli.a_b_c.lock")
    );
}

#[test]
fn test_concurrent_writes_to_shared_index_lose_nothing() {
    let env = TestEnvironment::new();
    let dir = env.backup_dir.clone();
    let n = 16;

    let handles: Vec<_> = (0..n)
        .map(|i| {
            let dir = dir.clone();
            thread::spawn(move || {
                let executor = backup_cli::providers::command_executor::MockCommandExecutor::new();
                let storage = DiskStorageEngine::new(&executor);
                let file = dir.join(format!("p{}_backup.dump", i));
                std::fs::write(&file, b"x").unwrap();
                let meta = BackupMetadata {
                    id: format!("p{}_backup", i),
                    profile_name: format!("p{}", i),
                    profile_type: ProfileType::Postgres,
                    created_at: Utc::now(),
                    file_path: file,
                    file_size: 1,
                    checksum_sha256: "00".to_string(),
                    compression: CompressionAlgorithm::None,
                    status: BackupStatus::Success,
                    duration_seconds: 0.0,
                };
                storage.save_metadata(&dir, &meta).unwrap();
                HistoryAction::record(&dir, "backup", Some(&meta.profile_name), Some(&meta.id), "SUCCESS", "ok").unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let executor = backup_cli::providers::command_executor::MockCommandExecutor::new();
    let storage = DiskStorageEngine::new(&executor);
    assert_eq!(storage.list_backups(&dir, None).unwrap().len(), n);
    assert_eq!(HistoryAction::list(&dir).unwrap().len(), n);
}

#[test]
fn test_real_lock_blocks_same_profile_but_not_another() {
    use backup_cli::core::abstract_traits::LockService;
    use backup_cli::providers::lock_provider::SystemLockService;

    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let svc = SystemLockService::new();

    let lock_a = config.profile_lock_path(&config.profiles[0]);
    let lock_b = config.profile_lock_path(&config.profiles[1]);

    let guard_a = svc.acquire_lock(&lock_a).expect("premier verrou du profil A");
    // Même profil : refusé tant que le premier tourne
    assert!(svc.acquire_lock(&lock_a).is_err());
    // Autre profil : autorisé en parallèle
    let _guard_b = svc.acquire_lock(&lock_b).expect("le profil B doit pouvoir tourner");

    // Après libération, le profil A peut repartir
    drop(guard_a);
    assert!(svc.acquire_lock(&lock_a).is_ok());
}
