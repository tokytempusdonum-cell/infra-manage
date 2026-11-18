//! Tests unitaires avec mocks de l'action de sauvegarde.

mod common;
use common::TestEnvironment;

use backup_cli::actions::backup_action::BackupAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::model::RemoteStorageConfig;

#[test]
fn test_backup_postgres_nominal_flow() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let count = action
        .execute(&config, Some("authentik"), false)
        .expect("Échec sauvegarde postgres");
    assert_eq!(count, 1);

    // Vérification de la création de la sauvegarde dans le stockage
    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .expect("Échec listing");
    assert_eq!(backups.len(), 1);

    let backup = &backups[0];
    assert!(backup.file_path.exists());
    assert!(backup.file_size > 0);
    assert!(!backup.checksum_sha256.is_empty());

    // Vérification de l'existence du fichier compagnon .sha256
    let sha_path = std::path::PathBuf::from(format!("{}.sha256", backup.file_path.display()));
    assert!(sha_path.exists());

    // Vérification des notifications
    let notifs = env.notifier.get_messages();
    assert_eq!(notifs.len(), 1);
    assert!(notifs[0].contains("1/1 profil sauvegardé"), "message inattendu : {}", notifs[0]);
    assert!(notifs[0].contains("Total : 1 · Réussies : 1 · Échecs : 0"));
}

#[test]
fn test_backup_mysql_nominal_flow() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let count = action
        .execute(&config, Some("ghost"), false)
        .expect("Échec sauvegarde mysql");
    assert_eq!(count, 1);

    let backups = storage
        .list_backups(&env.backup_dir, Some("ghost"))
        .expect("Échec listing");
    assert_eq!(backups.len(), 1);
    assert!(backups[0].file_path.to_string_lossy().ends_with(".sql.gz"));
}

#[test]
fn test_backup_filesystem_nominal_flow() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    let src_dir = env.populate_dummy_files();
    config.profiles[2].source.paths = Some(vec![src_dir]);

    let storage = env.storage();
    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let count = action
        .execute(&config, Some("configs"), false)
        .expect("Échec sauvegarde filesystem");
    assert_eq!(count, 1);

    let backups = storage
        .list_backups(&env.backup_dir, Some("configs"))
        .expect("Échec listing");
    assert_eq!(backups.len(), 1);
    assert!(backups[0].file_path.to_string_lossy().ends_with(".tar.gz"));
    assert!(backups[0].file_size > 0);
}

#[test]
fn test_backup_aborts_when_docker_container_stopped() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    // Simulation conteneur arrêté
    env.executor
        .set_container_running("authentik-postgresql-1", false);

    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let result = action.execute(&config, Some("authentik"), false);
    assert!(result.is_err(), "La sauvegarde aurait dû échouer car le conteneur est arrêté");

    // Aucune archive valide ne doit être créée
    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .unwrap();
    assert_eq!(backups.len(), 0);
}

#[test]
fn test_backup_pre_and_post_commands_executed() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    let marker_file = env.temp_dir.path().join("pre_cmd_marker.txt");
    let marker_str = marker_file.to_string_lossy().to_string();

    config.profiles[0].source.pre_commands = Some(vec![format!("touch {}", marker_str)]);

    let storage = env.storage();
    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let count = action
        .execute(&config, Some("authentik"), false)
        .expect("Échec sauvegarde avec pre_commands");
    assert_eq!(count, 1);

    // Vérifie que l'exécuteur a bien reçu l'appel sh -c
    let calls = env.executor.get_calls();
    let found_pre_cmd = calls
        .iter()
        .any(|c| c.command == "sh" && c.args.contains(&format!("touch {}", marker_str)));
    assert!(found_pre_cmd, "La pré-commande aurait dû être exécutée");
}

#[test]
fn test_single_file_upload_does_not_receive_rclone_filters() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.settings.storage = Some(RemoteStorageConfig {
        rclone_config: Some("/root/.config/rclone/rclone.conf".into()),
        remote: "s3backup:backups/s001/backup".into(),
        config_remote: None,
        upload_max_duration: "6h".into(),
        config_max_duration: "12h".into(),
        // Filtre volontairement présent dans les réglages globaux : il ne doit pas
        // être transmis au `copyto` d'un fichier unique.
        extra_args: Some(vec!["--exclude".into(), "/locks/**".into()]),
        server_name: None,
    });

    let storage = env.storage();
    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    action
        .execute(&config, Some("authentik"), false)
        .expect("Échec sauvegarde postgres");

    let calls = env.executor.get_calls();
    let upload = calls
        .iter()
        .find(|c| c.command == "timeout" && c.args.iter().any(|a| a == "copyto"))
        .expect("aucun envoi rclone copyto enregistré");

    assert!(
        !upload.args.iter().any(|a| a == "--exclude" || a == "/locks/**"),
        "un filtre rclone a été transmis au copyto : {:?}",
        upload.args
    );
    assert!(
        upload.args.windows(2).any(|w| w[0] == "--config" && w[1] == "/root/.config/rclone/rclone.conf"),
        "--config manquant dans : {:?}",
        upload.args
    );
}

#[test]
fn test_backup_dry_run_does_not_create_files() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();

    let action = BackupAction::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let count = action
        .execute(&config, Some("authentik"), true)
        .expect("Échec dry-run");
    assert_eq!(count, 1);

    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .unwrap();
    assert_eq!(backups.len(), 0, "Aucun fichier ne doit être créé en mode simulation (dry-run)");
}
