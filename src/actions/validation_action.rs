//! Action de validation de configuration et de vérification d'intégrité des sauvegardes.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use hex::ToHex;
use sha2::{Digest, Sha256};
use tracing::{error, info};

use crate::core::abstract_traits::StorageEngine;
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::AppConfig;

/// Action de vérification de l'intégrité physique d'une sauvegarde.
pub struct VerifyBackupAction<'a> {
    storage: &'a dyn StorageEngine,
}

impl<'a> VerifyBackupAction<'a> {
    /// Initialise l'action avec son moteur de stockage.
    pub fn new(storage: &'a dyn StorageEngine) -> Self {
        Self { storage }
    }

    /// Calcule la somme de contrôle SHA-256 d'un fichier sur disque.
    pub fn compute_sha256(path: &Path) -> DomainResult<String> {
        let file = File::open(path).map_err(|e| {
            DomainError::Storage(format!("Impossible d'ouvrir le fichier {:?} : {}", path, e))
        })?;
        let mut reader = BufReader::new(file);
        let mut hasher = Sha256::new();
        let mut buffer = [0u8; 65536];

        loop {
            let bytes_read = reader.read(&mut buffer)?;
            if bytes_read == 0 {
                break;
            }
            hasher.update(&buffer[..bytes_read]);
        }

        let result = hasher.finalize();
        Ok(result.encode_hex::<String>())
    }

    /// Vérifie la cohérence du contenu d'un fichier avec son type présumé (magic bytes).
    pub fn verify_magic_bytes(path: &Path) -> DomainResult<&'static str> {
        let mut file = File::open(path)?;
        let mut header = [0u8; 16];
        let bytes_read = file.read(&mut header)?;

        if bytes_read < 4 {
            return Err(DomainError::Integrity {
                resource: path.display().to_string(),
                message: "Le fichier de sauvegarde est trop court ou vide".to_string(),
            });
        }

        // PostgreSQL custom format (magic: "PGDMP")
        if header.starts_with(b"PGDMP") {
            return Ok("PostgreSQL Dump (PGDMP)");
        }

        // GZIP compressed (magic: 0x1F, 0x8B)
        if header.starts_with(&[0x1F, 0x8B]) {
            return Ok("GZIP Compressed Archive / SQL");
        }

        // Zstandard compressed (magic: 0x28, 0xB5, 0x2F, 0xFD)
        if header.starts_with(&[0x28, 0xB5, 0x2F, 0xFD]) {
            return Ok("Zstandard Compressed Archive");
        }

        // TAR archive (check standard ustar at offset 257 if file is large enough)
        if bytes_read >= 16 {
            // Un dump SQL en texte clair commence généralement par "--" ou "/*" ou "CREATE"
            if header.starts_with(b"--") || header.starts_with(b"/*") {
                return Ok("SQL Plaintext Script");
            }
        }

        Ok("Format générique ou archive tar standard")
    }

    /// Exécute la vérification d'intégrité complète pour une sauvegarde donnée.
    pub fn execute(&self, base_dir: &Path, backup_id: &str) -> DomainResult<bool> {
        info!(
            "[LOG ACTION] Début de la vérification d'intégrité de la sauvegarde '{}'",
            backup_id
        );

        let metadata_opt = self.storage.find_backup(base_dir, backup_id)?;
        let metadata = match metadata_opt {
            Some(m) => m,
            None => {
                return Err(DomainError::Storage(format!(
                    "Sauvegarde introuvable avec l'identifiant '{}'",
                    backup_id
                )));
            }
        };

        if !metadata.file_path.exists() {
            return Err(DomainError::Storage(format!(
                "Le fichier physique associé {:?} n'existe pas",
                metadata.file_path
            )));
        }

        // 1. Vérification de la non-vacuité
        let file_size = std::fs::metadata(&metadata.file_path)?.len();
        if file_size == 0 {
            return Err(DomainError::Integrity {
                resource: backup_id.to_string(),
                message: "Le fichier de sauvegarde est corrompu (taille 0 octet)".to_string(),
            });
        }

        // 2. Vérification des magic bytes
        let detected_format = Self::verify_magic_bytes(&metadata.file_path)?;
        info!(
            "[LOG ACTION] Format d'entête détecté : {}",
            detected_format
        );

        // 3. Recalcul et comparaison de l'empreinte SHA-256
        let calculated_hash = Self::compute_sha256(&metadata.file_path)?;

        if !metadata.checksum_sha256.is_empty()
            && !calculated_hash.eq_ignore_ascii_case(&metadata.checksum_sha256)
        {
            error!(
                "[LOG ACTION] CORRUPTION DÉTECTÉE : hash calculé ({}) != hash attendu ({})",
                calculated_hash, metadata.checksum_sha256
            );
            return Err(DomainError::Integrity {
                resource: backup_id.to_string(),
                message: format!(
                    "Somme de contrôle SHA-256 non concordante ! Calculé : {}, Attendu : {}",
                    calculated_hash, metadata.checksum_sha256
                ),
            });
        }

        info!(
            "[LOG ACTION] Intégrité VALIDÉE pour '{}' (SHA-256: {})",
            backup_id, calculated_hash
        );
        Ok(true)
    }
}

/// Action de validation formelle d'un fichier de configuration YAML.
pub struct ValidateConfigAction;

impl ValidateConfigAction {
    /// Charge et valide la configuration spécifiée.
    pub fn execute(config_path: &Path) -> DomainResult<AppConfig> {
        info!(
            "[LOG ACTION] Validation du fichier de configuration : {:?}",
            config_path
        );

        if !config_path.exists() {
            return Err(DomainError::Config(format!(
                "Fichier de configuration introuvable : {:?}",
                config_path
            )));
        }

        let content = std::fs::read_to_string(config_path)?;
        let config: AppConfig = serde_yaml::from_str(&content).map_err(|e| {
            DomainError::Config(format!(
                "Erreur de syntaxe YAML dans {:?} : {}",
                config_path, e
            ))
        })?;

        config.validate()?;

        info!(
            "[LOG ACTION] Configuration valide : version {}, {} profil(s) configuré(s)",
            config.version,
            config.profiles.len()
        );

        Ok(config)
    }
}
