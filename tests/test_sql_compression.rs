//! Tests de la vraie compression des dumps MySQL / MariaDB et de la restauration associée.

mod common;
use common::TestEnvironment;

use std::fs;
use std::io::Read;
use std::path::Path;
use std::sync::{Arc, Mutex};

use backup_cli::actions::backup_action::BackupAction;
use backup_cli::actions::restore_action::RestoreAction;
use backup_cli::core::abstract_traits::{CommandExecutor, ExecutionOutput, StorageEngine};
use backup_cli::core::exceptions::DomainResult;
use backup_cli::core::model::{CompressionAlgorithm, CompressionConfig, ProfileType};
use backup_cli::providers::stream_compression::{compress_file, RestoreInput};

const SQL: &str = "-- MariaDB dump\nCREATE TABLE t (id INT);\nINSERT INTO t VALUES (1),(2),(3);\n";

/// Exécuteur de test : produit un dump SQL connu et capture le contenu reçu sur stdin.
#[derive(Clone, Default)]
struct CapturingExecutor {
    stdin_seen: Arc<Mutex<Vec<Vec<u8>>>>,
}

impl CommandExecutor for CapturingExecutor {
    fn execute(&self, _c: &str, _a: &[&str], _e: &[(&str, &str)]) -> DomainResult<ExecutionOutput> {
        Ok(ExecutionOutput { stdout: String::new(), stderr: String::new(), exit_code: 0, success: true })
    }
    fn execute_to_file(&self, _c: &str, _a: &[&str], _e: &[(&str, &str)], out: &Path) -> DomainResult<u64> {
        fs::write(out, SQL)?;
        Ok(SQL.len() as u64)
    }
    fn execute_with_stdin(&self, _c: &str, _a: &[&str], _e: &[(&str, &str)], stdin: &Path) -> DomainResult<ExecutionOutput> {
        self.stdin_seen.lock().unwrap().push(fs::read(stdin)?);
        Ok(ExecutionOutput { stdout: String::new(), stderr: String::new(), exit_code: 0, success: true })
    }
    fn is_container_running(&self, _c: &str) -> DomainResult<bool> {
        Ok(true)
    }
}

fn gunzip(bytes: &[u8]) -> String {
    let mut out = String::new();
    flate2::read::GzDecoder::new(bytes).read_to_string(&mut out).unwrap();
    out
}

fn run(env: &TestEnvironment, exec: &CapturingExecutor, algo: Option<CompressionConfig>, kind: ProfileType)
    -> (backup_cli::core::model::AppConfig, backup_cli::core::model::BackupMetadata)
{
    let mut config = env.build_sample_config();
    config.profiles.truncate(2); // authentik (postgres) + ghost (mysql)
    config.profiles[1].profile_type = kind;
    config.profiles[1].compression = algo;
    let storage = backup_cli::providers::storage_provider::DiskStorageEngine::new(exec);
    BackupAction::new(exec, &storage, &env.lock_service, &env.notifier)
        .execute(&config, Some("ghost"), false)
        .expect("sauvegarde échouée");
    let meta = storage.list_backups(&env.backup_dir, Some("ghost")).unwrap().remove(0);
    (config, meta)
}

fn gz() -> Option<CompressionConfig> {
    Some(CompressionConfig { enabled: true, algorithm: CompressionAlgorithm::Gzip, level: None })
}

#[test]
fn test_mysql_dump_is_really_gzipped() {
    let env = TestEnvironment::new();
    let exec = CapturingExecutor::default();
    let (_, meta) = run(&env, &exec, gz(), ProfileType::Mysql);

    assert!(meta.file_path.to_string_lossy().ends_with(".sql.gz"));
    assert_eq!(meta.compression, CompressionAlgorithm::Gzip);
    let bytes = fs::read(&meta.file_path).unwrap();
    assert_eq!(&bytes[..2], &[0x1F, 0x8B], "le fichier doit être réellement gzip");
    assert_eq!(gunzip(&bytes), SQL);
    assert!(meta.file_size > 0 && !meta.checksum_sha256.is_empty());
    // aucun fichier temporaire résiduel
    let leftovers: Vec<_> = fs::read_dir(&env.backup_dir).unwrap()
        .filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".part")).collect();
    assert!(leftovers.is_empty(), "fichiers .part résiduels : {:?}", leftovers);
}

#[test]
fn test_mysql_without_compression_config_defaults_to_gzip() {
    let env = TestEnvironment::new();
    let exec = CapturingExecutor::default();
    let (_, meta) = run(&env, &exec, None, ProfileType::Mysql);
    assert!(meta.file_path.to_string_lossy().ends_with(".sql.gz"));
    assert_eq!(gunzip(&fs::read(&meta.file_path).unwrap()), SQL);
}

#[test]
fn test_compression_disabled_keeps_plain_sql() {
    let env = TestEnvironment::new();
    let exec = CapturingExecutor::default();
    let off = Some(CompressionConfig { enabled: false, algorithm: CompressionAlgorithm::None, level: None });
    let (_, meta) = run(&env, &exec, off, ProfileType::Mysql);
    assert!(meta.file_path.to_string_lossy().ends_with(".sql"));
    assert_eq!(meta.compression, CompressionAlgorithm::None);
    assert_eq!(fs::read_to_string(&meta.file_path).unwrap(), SQL);
}

#[test]
fn test_mariadb_zstd_dump() {
    let env = TestEnvironment::new();
    let exec = CapturingExecutor::default();
    let zstd_cfg = Some(CompressionConfig { enabled: true, algorithm: CompressionAlgorithm::Zstd, level: Some(5) });
    let (_, meta) = run(&env, &exec, zstd_cfg, ProfileType::Mariadb);
    assert!(meta.file_path.to_string_lossy().ends_with(".sql.zst"));
    assert_eq!(meta.compression, CompressionAlgorithm::Zstd);
    let decoded = zstd::decode_all(&fs::read(&meta.file_path).unwrap()[..]).unwrap();
    assert_eq!(String::from_utf8(decoded).unwrap(), SQL);
}

#[test]
fn test_restore_roundtrip_sends_decompressed_sql_to_client() {
    for kind in [ProfileType::Mysql, ProfileType::Mariadb] {
        let env = TestEnvironment::new();
        let exec = CapturingExecutor::default();
        let (config, meta) = run(&env, &exec, gz(), kind);

        let storage = backup_cli::providers::storage_provider::DiskStorageEngine::new(&exec);
        RestoreAction::new(&exec, &storage)
            .execute(&config, &meta.id, None, None, None, false)
            .expect("restauration échouée");

        let seen = exec.stdin_seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(String::from_utf8(seen[0].clone()).unwrap(), SQL, "le client doit recevoir du SQL en clair");
        // le fichier temporaire de décompression est supprimé
        assert!(!Path::new(&format!("{}.restore.part", meta.file_path.display())).exists());
    }
}

#[test]
fn test_legacy_plain_file_named_sql_gz_is_still_restorable() {
    let env = TestEnvironment::new();
    let legacy = env.temp_dir.path().join("old_backup.sql.gz");
    fs::write(&legacy, SQL).unwrap(); // ancien format : SQL en clair malgré le nom

    let input = RestoreInput::prepare(&legacy).unwrap();
    assert_eq!(input.path(), legacy.as_path());
    assert_eq!(fs::read_to_string(input.path()).unwrap(), SQL);
}

#[test]
fn test_restore_input_cleans_temp_file_on_drop() {
    let env = TestEnvironment::new();
    let plain = env.temp_dir.path().join("a.sql");
    let packed = env.temp_dir.path().join("a.sql.zst");
    fs::write(&plain, SQL).unwrap();
    compress_file(&plain, &packed, CompressionAlgorithm::Zstd, None).unwrap();

    let temp_path;
    {
        let input = RestoreInput::prepare(&packed).unwrap();
        temp_path = input.path().to_path_buf();
        assert_eq!(fs::read_to_string(&temp_path).unwrap(), SQL);
    }
    assert!(!temp_path.exists());
}
