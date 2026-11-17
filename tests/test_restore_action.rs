//! Tests unitaires avec mocks de l'action de restauration.

use std::fs;

mod common;
use common::TestEnvironment;

use backup_cli::actions::backup_action::BackupAction;
use backup_cli::actions::restore_action::RestoreAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::exceptions::DomainError;

#[test]
fn test_restore_postgres_nominal() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    // 1. Création préalable d'une sauvegarde
    let backup_action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );
    backup_action
        .execute(&config, Some("authentik"), false)
        .unwrap();

    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .unwrap();
    let backup_id = &backups[0].id;

    // 2. Restauration
    let restore_action = RestoreAction::new(&env.executor, &storage);
    restore_action
        .execute(&config, backup_id, None, None, None, false)
        .expect("Restauration PostgreSQL échouée");

    // 3. Vérification des appels reçus par le mock
    let calls = env.executor.get_calls();
    let has_pg_restore = calls
        .iter()
        .any(|c| c.args.contains(&"pg_restore".to_string()));
    assert!(has_pg_restore, "pg_restore aurait dû être exécuté via docker exec");
}

#[test]
fn test_restore_filesystem_nominal() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    let src_dir = env.populate_dummy_files();
    config.profiles[2].source.paths = Some(vec![src_dir]);

    let storage = env.storage();
    let backup_action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );
    backup_action
        .execute(&config, Some("configs"), false)
        .unwrap();

    let backups = storage
        .list_backups(&env.backup_dir, Some("configs"))
        .unwrap();
    let backup_id = &backups[0].id;

    let restore_target = env.temp_dir.path().join("restored_files");

    let restore_action = RestoreAction::new(&env.executor, &storage);
    restore_action
        .execute(
            &config,
            backup_id,
            Some(&restore_target),
            None,
            None,
            false,
        )
        .expect("Restauration filesystem échouée");

    // Vérification des fichiers restaurés
    assert!(restore_target.exists());
    let restored_file = restore_target.join(env.temp_dir.path().strip_prefix("/").unwrap()).join("source_files/data.txt");
    assert!(restored_file.exists(), "Le fichier data.txt aurait dû être extrait");
}

#[test]
fn test_restore_fails_when_target_not_empty_without_force() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    let src_dir = env.populate_dummy_files();
    config.profiles[2].source.paths = Some(vec![src_dir]);

    let storage = env.storage();
    let backup_action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );
    backup_action
        .execute(&config, Some("configs"), false)
        .unwrap();

    let backups = storage
        .list_backups(&env.backup_dir, Some("configs"))
        .unwrap();
    let backup_id = &backups[0].id;

    let target_dir = env.temp_dir.path().join("existing_target");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("critical_data.txt"), "Données existantes importantes").unwrap();

    let restore_action = RestoreAction::new(&env.executor, &storage);
    let result = restore_action.execute(
        &config,
        backup_id,
        Some(&target_dir),
        None,
        None,
        false, // force = false
    );

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::RestoreSafety { target } => {
            assert!(target.contains("existing_target"));
        }
        err => panic!("Type d'erreur inattendu : {:?}", err),
    }
}

#[test]
fn test_restore_succeeds_when_force_is_true() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    let src_dir = env.populate_dummy_files();
    config.profiles[2].source.paths = Some(vec![src_dir]);

    let storage = env.storage();
    let backup_action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );
    backup_action
        .execute(&config, Some("configs"), false)
        .unwrap();

    let backups = storage
        .list_backups(&env.backup_dir, Some("configs"))
        .unwrap();
    let backup_id = &backups[0].id;

    let target_dir = env.temp_dir.path().join("existing_target_force");
    fs::create_dir_all(&target_dir).unwrap();
    fs::write(target_dir.join("old.txt"), "Données").unwrap();

    let restore_action = RestoreAction::new(&env.executor, &storage);
    let result = restore_action.execute(
        &config,
        backup_id,
        Some(&target_dir),
        None,
        None,
        true, // force = true
    );

    assert!(result.is_ok(), "La restauration forcée aurait dû réussir");
}

#[test]
fn test_restore_fails_when_checksum_mismatched() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let backup_action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );
    backup_action
        .execute(&config, Some("authentik"), false)
        .unwrap();

    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .unwrap();
    let backup_id = &backups[0].id;
    let file_path = &backups[0].file_path;

    // Altération malveillante du fichier sur le disque
    fs::write(file_path, "FICHIER CORROMPU VOLONTAIREMENT").unwrap();

    let restore_action = RestoreAction::new(&env.executor, &storage);
    let result = restore_action.execute(&config, backup_id, None, None, None, false);

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::Integrity { .. } => {
            // Intégrité corrompue détectée avec succès avant toute restauration !
        }
        err => panic!("L'erreur d'intégrité aurait dû être levée : {:?}", err),
    }
}
