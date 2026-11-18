//! Tests des profils MongoDB et MariaDB (sauvegarde, restauration, validation).

mod common;
use common::TestEnvironment;

use backup_cli::actions::backup_action::BackupAction;
use backup_cli::actions::restore_action::RestoreAction;
use backup_cli::actions::validation_action::ValidateConfigAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::exceptions::DomainError;
use backup_cli::core::model::{
    AppConfig, BackupProfile, CompressionAlgorithm, CompressionConfig, ProfileType, SourceConfig,
    ValidationConfig,
};

fn profile(name: &str, kind: ProfileType, source: SourceConfig) -> BackupProfile {
    BackupProfile {
        name: name.to_string(),
        enabled: true,
        profile_type: kind,
        schedule: None,
        source,
        destination: Default::default(),
        compression: None,
        retention: None,
        validation: Some(ValidationConfig::default()),
        application: None,
    }
}

fn config_with(env: &TestEnvironment, p: BackupProfile) -> AppConfig {
    let mut config = env.build_sample_config();
    config.profiles = vec![p];
    config
}

fn run_backup(env: &TestEnvironment, config: &AppConfig, name: &str) -> BackupMetadataLite {
    let storage = env.storage();
    let action = BackupAction::new(&env.executor, &storage, &env.lock_service, &env.notifier);
    action.execute(config, Some(name), false).expect("sauvegarde échouée");
    let b = storage.list_backups(&env.backup_dir, Some(name)).unwrap();
    assert_eq!(b.len(), 1);
    BackupMetadataLite {
        id: b[0].id.clone(),
        path: b[0].file_path.to_string_lossy().to_string(),
        compression: b[0].compression,
        profile_type: b[0].profile_type,
    }
}

struct BackupMetadataLite {
    id: String,
    path: String,
    compression: CompressionAlgorithm,
    profile_type: ProfileType,
}

#[test]
fn test_mongo_backup_container_with_password_keeps_secret_out_of_args() {
    std::env::set_var("TEST_MONGO_PWD_A", "s3cr3t-pwd");
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "mongo-app",
            ProfileType::Mongo,
            SourceConfig {
                container_name: Some("mongo-app-1".to_string()),
                username: Some("root".to_string()),
                database: Some("appdb".to_string()),
                password_env: Some("TEST_MONGO_PWD_A".to_string()),
                ..Default::default()
            },
        ),
    );

    let b = run_backup(&env, &config, "mongo-app");
    assert!(b.path.ends_with(".archive.gz"), "extension inattendue : {}", b.path);
    assert_eq!(b.compression, CompressionAlgorithm::Gzip);
    assert_eq!(b.profile_type, ProfileType::Mongo);

    let call = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.args.iter().any(|a| a.contains("mongodump")))
        .expect("mongodump non appelé");
    assert_eq!(call.command, "docker");
    for expected in ["--archive", "--gzip", "--quiet", "--db", "appdb", "--username", "root",
                     "--authenticationDatabase", "admin", "mongo-app-1"] {
        assert!(call.args.contains(&expected.to_string()), "argument manquant : {}", expected);
    }
    // Le mot de passe ne doit JAMAIS figurer dans les arguments
    assert!(call.args.iter().all(|a| !a.contains("s3cr3t-pwd")));
    assert!(call.envs.contains(&("MONGO_PWD".to_string(), "s3cr3t-pwd".to_string())));
    // `docker exec -e MONGO_PWD` (valeur lue depuis l'environnement du client)
    let pos = call.args.iter().position(|a| a == "-e").unwrap();
    assert_eq!(call.args[pos + 1], "MONGO_PWD");
}

#[test]
fn test_mongo_backup_host_mode_without_auth_calls_mongodump_directly() {
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "mongo-host",
            ProfileType::Mongo,
            SourceConfig {
                host: Some("10.0.0.5".to_string()),
                port: Some(27018),
                ..Default::default()
            },
        ),
    );
    run_backup(&env, &config, "mongo-host");

    let call = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.command == "mongodump")
        .expect("mongodump direct attendu");
    assert!(call.args.windows(2).any(|w| w[0] == "--host" && w[1] == "10.0.0.5:27018"));
    // Pas de --db : toutes les bases
    assert!(!call.args.contains(&"--db".to_string()));
    assert!(call.envs.is_empty());
}

#[test]
fn test_mongo_without_gzip_produces_plain_archive() {
    let env = TestEnvironment::new();
    let mut p = profile(
        "mongo-plain",
        ProfileType::Mongo,
        SourceConfig {
            container_name: Some("m".to_string()),
            ..Default::default()
        },
    );
    p.compression = Some(CompressionConfig {
        enabled: false,
        algorithm: CompressionAlgorithm::None,
        level: None,
    });
    let config = config_with(&env, p);

    let b = run_backup(&env, &config, "mongo-plain");
    assert!(b.path.ends_with(".archive"));
    assert_eq!(b.compression, CompressionAlgorithm::None);
    let call = env.executor.get_calls().into_iter().find(|c| c.args.contains(&"mongodump".to_string())).unwrap();
    assert!(!call.args.contains(&"--gzip".to_string()));
}

#[test]
fn test_mongo_restore_uses_mongorestore_with_stdin_and_gzip() {
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "mongo-r",
            ProfileType::Mongo,
            SourceConfig {
                container_name: Some("mongo-r-1".to_string()),
                database: Some("appdb".to_string()),
                ..Default::default()
            },
        ),
    );
    let b = run_backup(&env, &config, "mongo-r");

    let storage = env.storage();
    RestoreAction::new(&env.executor, &storage)
        .execute(&config, &b.id, None, None, None, false)
        .expect("restauration mongo échouée");

    let call = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.args.contains(&"mongorestore".to_string()))
        .expect("mongorestore non appelé");
    assert_eq!(call.command, "docker");
    for expected in ["exec", "-i", "--archive", "--drop", "--gzip", "mongo-r-1"] {
        assert!(call.args.contains(&expected.to_string()), "argument manquant : {}", expected);
    }
}

#[test]
fn test_mongo_restore_to_other_database_renames_namespace() {
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "mongo-ns",
            ProfileType::Mongo,
            SourceConfig {
                container_name: Some("m".to_string()),
                database: Some("prod".to_string()),
                ..Default::default()
            },
        ),
    );
    let b = run_backup(&env, &config, "mongo-ns");

    let storage = env.storage();
    RestoreAction::new(&env.executor, &storage)
        .execute(&config, &b.id, None, Some("staging"), None, false)
        .expect("restauration mongo échouée");

    let call = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.args.contains(&"mongorestore".to_string()))
        .unwrap();
    assert!(call.args.windows(2).any(|w| w[0] == "--nsFrom" && w[1] == "prod.*"));
    assert!(call.args.windows(2).any(|w| w[0] == "--nsTo" && w[1] == "staging.*"));
}

#[test]
fn test_mongo_restore_dest_db_requires_source_database() {
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "mongo-all",
            ProfileType::Mongo,
            SourceConfig {
                container_name: Some("m".to_string()),
                ..Default::default()
            },
        ),
    );
    let b = run_backup(&env, &config, "mongo-all");

    let storage = env.storage();
    let err = RestoreAction::new(&env.executor, &storage)
        .execute(&config, &b.id, None, Some("other"), None, false)
        .unwrap_err();
    assert!(matches!(err, DomainError::Config(_)), "erreur inattendue : {:?}", err);
}

#[test]
fn test_mariadb_backup_uses_mariadb_dump_and_restore_uses_mariadb_client() {
    let env = TestEnvironment::new();
    let config = config_with(
        &env,
        profile(
            "maria",
            ProfileType::Mariadb,
            SourceConfig {
                container_name: Some("maria-db-1".to_string()),
                username: Some("root".to_string()),
                database: Some("shop".to_string()),
                ..Default::default()
            },
        ),
    );
    let b = run_backup(&env, &config, "maria");
    assert!(b.path.ends_with(".sql.gz"), "extension inattendue : {}", b.path);
    assert_eq!(b.profile_type, ProfileType::Mariadb);

    let dump = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.args.contains(&"mariadb-dump".to_string()))
        .expect("mariadb-dump non appelé");
    assert!(!dump.args.contains(&"mysqldump".to_string()));
    for expected in ["--single-transaction", "--routines", "--triggers", "--events", "shop"] {
        assert!(dump.args.contains(&expected.to_string()));
    }

    let storage = env.storage();
    RestoreAction::new(&env.executor, &storage)
        .execute(&config, &b.id, None, None, None, false)
        .expect("restauration mariadb échouée");
    let restore = env
        .executor
        .get_calls()
        .into_iter()
        .find(|c| c.args.contains(&"mariadb".to_string()) && c.args.contains(&"-i".to_string()))
        .expect("client mariadb non appelé pour la restauration");
    assert!(restore.args.contains(&"shop".to_string()));
}

#[test]
fn test_mysql_profile_still_uses_mysqldump() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();
    BackupAction::new(&env.executor, &storage, &env.lock_service, &env.notifier)
        .execute(&config, Some("ghost"), false)
        .unwrap();
    assert!(env.executor.get_calls().iter().any(|c| c.args.contains(&"mysqldump".to_string())));
}

#[test]
fn test_yaml_accepts_mongo_mongodb_alias_and_mariadb() {
    let env = TestEnvironment::new();
    let yaml = r#"
version: 1
settings:
  backup_directory: /tmp/b
profiles:
  - name: m1
    type: mongodb
    source: { container_name: mongo-1, database: app, username: root, password_env: X, auth_database: admin }
  - name: m2
    type: mongo
    source: { host: 127.0.0.1 }
  - name: maria
    type: mariadb
    source: { container_name: db-1, username: root, database: shop }
"#;
    std::fs::write(&env.config_file, yaml).unwrap();
    let cfg = ValidateConfigAction::execute(&env.config_file).expect("YAML valide attendu");
    assert_eq!(cfg.profiles[0].profile_type, ProfileType::Mongo);
    assert_eq!(cfg.profiles[1].profile_type, ProfileType::Mongo);
    assert_eq!(cfg.profiles[2].profile_type, ProfileType::Mariadb);
}

fn validation_error(env: &TestEnvironment, p: BackupProfile) -> String {
    let config = config_with(env, p);
    env.write_config_yaml(&config);
    match ValidateConfigAction::execute(&env.config_file).unwrap_err() {
        DomainError::Config(m) => m,
        other => panic!("erreur inattendue : {:?}", other),
    }
}

#[test]
fn test_mongo_validation_rules() {
    let env = TestEnvironment::new();

    let msg = validation_error(&env, profile("m", ProfileType::Mongo, SourceConfig::default()));
    assert!(msg.contains("container_name"));

    let msg = validation_error(
        &env,
        profile(
            "m",
            ProfileType::Mongo,
            SourceConfig {
                container_name: Some("c".to_string()),
                password_env: Some("X".to_string()),
                ..Default::default()
            },
        ),
    );
    assert!(msg.contains("username"));

    let mut zstd = profile(
        "m",
        ProfileType::Mongo,
        SourceConfig { container_name: Some("c".to_string()), ..Default::default() },
    );
    zstd.compression = Some(CompressionConfig {
        enabled: true,
        algorithm: CompressionAlgorithm::Zstd,
        level: None,
    });
    assert!(validation_error(&env, zstd).contains("gzip"));
}

#[test]
fn test_mariadb_validation_requires_database() {
    let env = TestEnvironment::new();
    let msg = validation_error(
        &env,
        profile(
            "maria",
            ProfileType::Mariadb,
            SourceConfig {
                container_name: Some("c".to_string()),
                username: Some("root".to_string()),
                ..Default::default()
            },
        ),
    );
    assert!(msg.contains("MariaDB") && msg.contains("'database' requis"));
}
