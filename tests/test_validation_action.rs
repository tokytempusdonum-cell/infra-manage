//! Tests unitaires de validation et d'intégrité des fichiers.

use std::fs;

mod common;
use common::TestEnvironment;

use backup_cli::actions::validation_action::VerifyBackupAction;
use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::core::exceptions::DomainError;
use backup_cli::core::model::{BackupMetadata, BackupStatus, CompressionAlgorithm, ProfileType};

#[test]
fn test_compute_sha256() {
    let env = TestEnvironment::new();
    let file_path = env.temp_dir.path().join("hash_test.txt");
    fs::write(&file_path, "hello world").unwrap();

    let hash = VerifyBackupAction::compute_sha256(&file_path).unwrap();
    // SHA256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
    assert_eq!(
        hash,
        "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
    );
}

#[test]
fn test_magic_bytes_detection() {
    let env = TestEnvironment::new();

    // 1. PostgreSQL magic PGDMP
    let pg_file = env.temp_dir.path().join("test.dump");
    fs::write(&pg_file, b"PGDMP\x01\x02\x03custom_postgres_content").unwrap();
    let res = VerifyBackupAction::verify_magic_bytes(&pg_file).unwrap();
    assert!(res.contains("PostgreSQL"));

    // 2. GZIP magic 0x1F 0x8B
    let gz_file = env.temp_dir.path().join("test.sql.gz");
    fs::write(&gz_file, &[0x1F, 0x8B, 0x08, 0x00, 0x00, 0x00]).unwrap();
    let res_gz = VerifyBackupAction::verify_magic_bytes(&gz_file).unwrap();
    assert!(res_gz.contains("GZIP"));

    // 3. Zstandard magic 0x28 0xB5 0x2F 0xFD
    let zst_file = env.temp_dir.path().join("test.tar.zst");
    fs::write(&zst_file, &[0x28, 0xB5, 0x2F, 0xFD, 0x00, 0x00]).unwrap();
    let res_zst = VerifyBackupAction::verify_magic_bytes(&zst_file).unwrap();
    assert!(res_zst.contains("Zstandard"));
}

#[test]
fn test_verify_action_detects_corrupted_hash() {
    let env = TestEnvironment::new();
    let storage = env.storage();

    let backup_file = env.backup_dir.join("corrupt_test.dump");
    fs::write(&backup_file, b"PGDMP_CONTENU_VALIDE").unwrap();

    let metadata = BackupMetadata {
        id: "corrupt_test".to_string(),
        profile_name: "authentik".to_string(),
        profile_type: ProfileType::Postgres,
        created_at: chrono::Utc::now(),
        file_path: backup_file,
        file_size: 20,
        checksum_sha256: "0000000000000000000000000000000000000000000000000000000000000000".to_string(), // Faux hash
        compression: CompressionAlgorithm::None,
        status: BackupStatus::Success,
        duration_seconds: 1.0,
    };
    storage.save_metadata(&env.backup_dir, &metadata).unwrap();

    let verify_action = VerifyBackupAction::new(&storage);
    let result = verify_action.execute(&env.backup_dir, "corrupt_test");

    assert!(result.is_err());
    match result.unwrap_err() {
        DomainError::Integrity { resource, message } => {
            assert_eq!(resource, "corrupt_test");
            assert!(message.contains("non concordante"));
        }
        err => panic!("Type d'erreur inattendu : {:?}", err),
    }
}
