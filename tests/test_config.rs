//! Tests unitaires du modèle et de la validation de configuration.

mod common;
use common::TestEnvironment;

use backup_cli::actions::validation_action::ValidateConfigAction;
use backup_cli::core::exceptions::DomainError;
use backup_cli::core::model::{BackupProfile, ProfileType, SourceConfig};

#[test]
fn test_valid_configuration_parsing() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    env.write_config_yaml(&config);

    let parsed = ValidateConfigAction::execute(&env.config_file).expect("Validation échouée");
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.profiles.len(), 3);
    assert_eq!(parsed.profiles[0].name, "authentik");
    assert_eq!(parsed.profiles[1].name, "ghost");
}

#[test]
fn test_missing_required_fields_fails_validation() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    // Suppression du database obligatoire pour un profil Postgres
    config.profiles[0].source.database = None;
    env.write_config_yaml(&config);

    let err = ValidateConfigAction::execute(&env.config_file).unwrap_err();
    match err {
        DomainError::Config(msg) => {
            assert!(msg.contains("'database' requis"));
        }
        _ => panic!("Type d'erreur inattendu : {:?}", err),
    }
}

#[test]
fn test_duplicate_profile_names_rejected() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.profiles.push(BackupProfile {
        name: "authentik".to_string(), // Nom dupliqué
        enabled: true,
        profile_type: ProfileType::Postgres,
        schedule: None,
        source: SourceConfig {
            container_name: Some("c".to_string()),
            username: Some("u".to_string()),
            database: Some("db".to_string()),
            ..Default::default()
        },
        destination: Default::default(),
        compression: None,
        retention: None,
        validation: None,
    });
    env.write_config_yaml(&config);

    let err = ValidateConfigAction::execute(&env.config_file).unwrap_err();
    match err {
        DomainError::Config(msg) => {
            assert!(msg.contains("Nom de profil dupliqué"));
        }
        _ => panic!("Type d'erreur inattendu : {:?}", err),
    }
}

#[test]
fn test_invalid_schema_version_rejected() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.version = 2; // Non supporté
    env.write_config_yaml(&config);

    let err = ValidateConfigAction::execute(&env.config_file).unwrap_err();
    match err {
        DomainError::Config(msg) => {
            assert!(msg.contains("Version de schéma de configuration non supportée"));
        }
        _ => panic!("Type d'erreur inattendu : {:?}", err),
    }
}

#[test]
fn test_filesystem_profile_without_paths_rejected() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.profiles[2].source.paths = Some(vec![]);
    env.write_config_yaml(&config);

    let err = ValidateConfigAction::execute(&env.config_file).unwrap_err();
    match err {
        DomainError::Config(msg) => {
            assert!(msg.contains("au moins un chemin dans 'paths'"));
        }
        _ => panic!("Type d'erreur inattendu : {:?}", err),
    }
}
