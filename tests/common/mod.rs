//! Utilitaires, fixtures et helpers pour les tests automatisés avec mocks.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

use backup_cli::core::abstract_traits::{LockGuard, LockService, NotificationService};
use backup_cli::core::exceptions::DomainResult;
use backup_cli::core::model::{
    AppConfig, BackupProfile, CompressionAlgorithm, CompressionConfig, DestinationConfig,
    GlobalSettings, ProfileType, RetentionPolicy, SourceConfig, ValidationConfig,
};
use backup_cli::providers::command_executor::MockCommandExecutor;
use backup_cli::providers::storage_provider::DiskStorageEngine;

/// Environnement de test isolé avec répertoires temporaires et mocks injectés.
#[allow(dead_code)]
pub struct TestEnvironment {
    pub temp_dir: TempDir,
    pub backup_dir: PathBuf,
    pub config_file: PathBuf,
    pub executor: MockCommandExecutor,
    pub lock_service: MockLockService,
    pub notifier: MockNotificationService,
}

#[allow(dead_code)]
impl TestEnvironment {
    /// Initialise un environnement de test complet avec configuration prête à l'emploi.
    pub fn new() -> Self {
        let temp_dir = tempfile::tempdir().expect("Impossible de créer le répertoire temporaire");
        let backup_dir = temp_dir.path().join("backup");
        fs::create_dir_all(&backup_dir).expect("Impossible de créer backup_dir");

        let config_file = temp_dir.path().join("backup.yml");
        let executor = MockCommandExecutor::new();
        let lock_service = MockLockService::new();
        let notifier = MockNotificationService::new();

        Self {
            temp_dir,
            backup_dir,
            config_file,
            executor,
            lock_service,
            notifier,
        }
    }

    /// Génère une configuration type avec plusieurs profils (Postgres, MySQL, Filesystem).
    pub fn build_sample_config(&self) -> AppConfig {
        AppConfig {
            version: 1,
            settings: GlobalSettings {
                log_level: "debug".to_string(),
                log_file: self.temp_dir.path().join("backup.log"),
                backup_directory: self.backup_dir.clone(),
                lock_file: self.temp_dir.path().join("backup.lock"),
                notifications: None,
                storage: None,
            },
            profiles: vec![
                BackupProfile {
                    name: "authentik".to_string(),
                    enabled: true,
                    profile_type: ProfileType::Postgres,
                    schedule: Some("0 2 * * *".to_string()),
                    source: SourceConfig {
                        container_name: Some("authentik-postgresql-1".to_string()),
                        username: Some("authentik".to_string()),
                        database: Some("authentik".to_string()),
                        ..Default::default()
                    },
                    destination: DestinationConfig::default(),
                    compression: None,
                    retention: Some(RetentionPolicy {
                        days: Some(14),
                        ..Default::default()
                    }),
                    validation: Some(ValidationConfig::default()),
                },
                BackupProfile {
                    name: "ghost".to_string(),
                    enabled: true,
                    profile_type: ProfileType::Mysql,
                    schedule: Some("0 3 * * *".to_string()),
                    source: SourceConfig {
                        container_name: Some("ghost-cms-db-1".to_string()),
                        username: Some("root".to_string()),
                        database: Some("ghost".to_string()),
                        ..Default::default()
                    },
                    destination: DestinationConfig::default(),
                    compression: Some(CompressionConfig {
                        enabled: true,
                        algorithm: CompressionAlgorithm::Gzip,
                        level: None,
                    }),
                    retention: Some(RetentionPolicy {
                        days: Some(14),
                        ..Default::default()
                    }),
                    validation: Some(ValidationConfig::default()),
                },
                BackupProfile {
                    name: "configs".to_string(),
                    enabled: true,
                    profile_type: ProfileType::Filesystem,
                    schedule: Some("0 4 * * *".to_string()),
                    source: SourceConfig {
                        paths: Some(vec![self.temp_dir.path().join("source_files")]),
                        exclude: Some(vec!["*.tmp".to_string()]),
                        ..Default::default()
                    },
                    destination: DestinationConfig::default(),
                    compression: Some(CompressionConfig {
                        enabled: true,
                        algorithm: CompressionAlgorithm::Gzip,
                        level: None,
                    }),
                    retention: Some(RetentionPolicy {
                        days: Some(7),
                        ..Default::default()
                    }),
                    validation: Some(ValidationConfig::default()),
                },
            ],
        }
    }

    /// Écrit la configuration au format YAML dans le fichier de config de test.
    pub fn write_config_yaml(&self, config: &AppConfig) {
        let yaml_str = serde_yaml::to_string(config).expect("Sérialisation YAML impossible");
        let mut file = File::create(&self.config_file).expect("Création fichier config impossible");
        file.write_all(yaml_str.as_bytes()).expect("Écriture YAML impossible");
    }

    /// Crée un dossier source avec des fichiers fictifs pour tester le profil filesystem.
    pub fn populate_dummy_files(&self) -> PathBuf {
        let src_dir = self.temp_dir.path().join("source_files");
        fs::create_dir_all(&src_dir).unwrap();

        let f1 = src_dir.join("data.txt");
        fs::write(f1, "Contenu important de sauvegarde").unwrap();

        let f2 = src_dir.join("cache.tmp");
        fs::write(f2, "Fichier temporaire à exclure").unwrap();

        let sub_dir = src_dir.join("sub");
        fs::create_dir_all(&sub_dir).unwrap();
        let f3 = sub_dir.join("config.json");
        fs::write(f3, r#"{"status": "ok"}"#).unwrap();

        src_dir
    }

    /// Crée une instance du moteur de stockage connecté au mock exécuteur.
    pub fn storage(&self) -> DiskStorageEngine<'_> {
        DiskStorageEngine::new(&self.executor)
    }
}

/// Mock du service de verrouillage (toujours accordé dans les tests).
pub struct MockLockService;

impl MockLockService {
    pub fn new() -> Self {
        Self
    }
}

pub struct DummyLockGuard;
impl LockGuard for DummyLockGuard {}

impl LockService for MockLockService {
    fn acquire_lock(&self, _lock_path: &Path) -> DomainResult<Box<dyn LockGuard>> {
        Ok(Box::new(DummyLockGuard))
    }
}

/// Mock du service de notification (enregistre les messages envoyés).
#[derive(Clone)]
pub struct MockNotificationService {
    pub messages: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl MockNotificationService {
    pub fn new() -> Self {
        Self {
            messages: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    #[allow(dead_code)]
    pub fn get_messages(&self) -> Vec<String> {
        self.messages.lock().unwrap().clone()
    }
}

impl NotificationService for MockNotificationService {
    fn notify(&self, message: &str) -> DomainResult<()> {
        let mut lock = self.messages.lock().unwrap();
        lock.push(message.to_string());
        Ok(())
    }
}
