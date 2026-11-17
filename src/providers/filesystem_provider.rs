//! Adaptateur pour la sauvegarde et la restauration de systèmes de fichiers (tar, gzip, zstd).

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use glob::Pattern;
use tar::{Archive, Builder};
use tracing::{debug, info};
use walkdir::WalkDir;

use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{CompressionAlgorithm, CompressionConfig};

/// Fournisseur d'opérations d'archivage et de décompression de systèmes de fichiers.
pub struct FilesystemProvider;

impl FilesystemProvider {
    /// Crée une nouvelle instance du fournisseur de système de fichiers.
    pub fn new() -> Self {
        Self
    }

    /// Compile les motifs d'exclusion globaux en motifs glob::Pattern vérifiés.
    fn compile_patterns(excludes: &[String]) -> DomainResult<Vec<Pattern>> {
        let mut patterns = Vec::with_capacity(excludes.len());
        for pattern_str in excludes {
            let pat = Pattern::new(pattern_str).map_err(|e| {
                DomainError::Config(format!(
                    "Motif d'exclusion invalide '{}' : {}",
                    pattern_str, e
                ))
            })?;
            patterns.push(pat);
        }
        Ok(patterns)
    }

    /// Détermine si un chemin correspond à l'un des motifs d'exclusion.
    fn is_excluded(path: &Path, patterns: &[Pattern]) -> bool {
        let path_str = path.to_string_lossy();
        let file_name = path
            .file_name()
            .map(|f| f.to_string_lossy())
            .unwrap_or_default();

        for pat in patterns {
            if pat.matches(&path_str) || pat.matches(&file_name) {
                return true;
            }
        }
        false
    }

    /// Crée une archive tar (compressée ou non) à partir d'une liste de chemins sources.
    pub fn create_archive(
        &self,
        sources: &[PathBuf],
        output_file: &Path,
        excludes: &[String],
        compression: Option<&CompressionConfig>,
    ) -> DomainResult<u64> {
        let patterns = Self::compile_patterns(excludes)?;
        let file = File::create(output_file).map_err(|e| {
            DomainError::Storage(format!(
                "Impossible de créer le fichier d'archive {:?} : {}",
                output_file, e
            ))
        })?;

        let writer = BufWriter::new(file);
        let algorithm = compression
            .filter(|c| c.enabled)
            .map(|c| c.algorithm)
            .unwrap_or(CompressionAlgorithm::None);

        match algorithm {
            CompressionAlgorithm::Gzip => {
                debug!("[LOG FILESYSTEM] Archivage avec compression GZIP");
                let level = compression
                    .and_then(|c| c.level)
                    .map(|l| Compression::new(l as u32))
                    .unwrap_or_else(Compression::default);
                let gz_writer = GzEncoder::new(writer, level);
                let mut tar_builder = Builder::new(gz_writer);
                self.populate_tar(&mut tar_builder, sources, &patterns)?;
                let gz = tar_builder.into_inner().map_err(|e| {
                    DomainError::Storage(format!("Échec de clôture de l'archive tar : {}", e))
                })?;
                gz.finish().map_err(|e| {
                    DomainError::Storage(format!("Échec de compression GZIP : {}", e))
                })?;
            }
            CompressionAlgorithm::Zstd => {
                debug!("[LOG FILESYSTEM] Archivage avec compression ZSTD");
                let level = compression.and_then(|c| c.level).unwrap_or(3);
                let zstd_writer = zstd::Encoder::new(writer, level).map_err(|e| {
                    DomainError::Storage(format!("Échec d'initialisation ZSTD : {}", e))
                })?;
                let mut tar_builder = Builder::new(zstd_writer);
                self.populate_tar(&mut tar_builder, sources, &patterns)?;
                let zstd = tar_builder.into_inner().map_err(|e| {
                    DomainError::Storage(format!("Échec de clôture de l'archive tar : {}", e))
                })?;
                zstd.finish().map_err(|e| {
                    DomainError::Storage(format!("Échec de compression ZSTD : {}", e))
                })?;
            }
            CompressionAlgorithm::None => {
                debug!("[LOG FILESYSTEM] Archivage sans compression");
                let mut tar_builder = Builder::new(writer);
                self.populate_tar(&mut tar_builder, sources, &patterns)?;
                tar_builder.finish().map_err(|e| {
                    DomainError::Storage(format!("Échec de clôture de l'archive tar : {}", e))
                })?;
            }
        }

        let metadata = fs::metadata(output_file)?;
        info!(
            "[LOG FILESYSTEM] Archive créée avec succès ({:?}, {} octets)",
            output_file,
            metadata.len()
        );
        Ok(metadata.len())
    }

    /// Rapproche et ajoute les entrées de fichiers dans le builder tar en respectant les exclusions.
    fn populate_tar<W: Write>(
        &self,
        builder: &mut Builder<W>,
        sources: &[PathBuf],
        patterns: &[Pattern],
    ) -> DomainResult<()> {
        for src in sources {
            if !src.exists() {
                debug!(
                    "[LOG FILESYSTEM] Source ignorée (n'existe pas) : {:?}",
                    src
                );
                continue;
            }

            if src.is_file() {
                if !Self::is_excluded(src, patterns) {
                    let mut file = File::open(src)?;
                    let relative_path = src.strip_prefix("/").unwrap_or(src);
                    builder
                        .append_file(relative_path, &mut file)
                        .map_err(|e| {
                            DomainError::Storage(format!(
                                "Échec d'ajout du fichier {:?} à l'archive : {}",
                                src, e
                            ))
                        })?;
                }
            } else if src.is_dir() {
                for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if Self::is_excluded(path, patterns) {
                        continue;
                    }

                    let relative_path = path.strip_prefix("/").unwrap_or(path);
                    if path.is_dir() {
                        builder.append_dir(relative_path, path).map_err(|e| {
                            DomainError::Storage(format!(
                                "Échec d'ajout du dossier {:?} à l'archive : {}",
                                path, e
                            ))
                        })?;
                    } else if path.is_file() {
                        let mut file = File::open(path)?;
                        builder.append_file(relative_path, &mut file).map_err(|e| {
                            DomainError::Storage(format!(
                                "Échec d'ajout du fichier {:?} à l'archive : {}",
                                path, e
                            ))
                        })?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Décompresse et extrait une archive tar vers un répertoire cible.
    ///
    /// Supporte une extraction partielle si `partial_filter` est spécifié.
    pub fn extract_archive(
        &self,
        archive_path: &Path,
        target_dir: &Path,
        algorithm: CompressionAlgorithm,
        partial_filter: Option<&str>,
    ) -> DomainResult<usize> {
        info!(
            "[LOG FILESYSTEM] Extraction de {:?} vers {:?}",
            archive_path, target_dir
        );

        fs::create_dir_all(target_dir).map_err(|e| {
            DomainError::Storage(format!(
                "Impossible de créer le dossier cible {:?} : {}",
                target_dir, e
            ))
        })?;

        let file = File::open(archive_path).map_err(|e| {
            DomainError::Storage(format!(
                "Impossible d'ouvrir l'archive {:?} : {}",
                archive_path, e
            ))
        })?;
        let reader = BufReader::new(file);

        let extracted_count = match algorithm {
            CompressionAlgorithm::Gzip => {
                let decoder = GzDecoder::new(reader);
                let mut archive = Archive::new(decoder);
                self.unpack_entries(&mut archive, target_dir, partial_filter)?
            }
            CompressionAlgorithm::Zstd => {
                let decoder = zstd::Decoder::new(reader).map_err(|e| {
                    DomainError::Storage(format!("Échec initialisation décompresseur ZSTD : {}", e))
                })?;
                let mut archive = Archive::new(decoder);
                self.unpack_entries(&mut archive, target_dir, partial_filter)?
            }
            CompressionAlgorithm::None => {
                let mut archive = Archive::new(reader);
                self.unpack_entries(&mut archive, target_dir, partial_filter)?
            }
        };

        info!(
            "[LOG FILESYSTEM] Extraction terminée : {} élément(s) restauré(s)",
            extracted_count
        );
        Ok(extracted_count)
    }

    /// Dépaquette les entrées en appliquant le filtre partiel et la sécurité anti-zip-slip.
    fn unpack_entries<R: Read>(
        &self,
        archive: &mut Archive<R>,
        target_dir: &Path,
        partial_filter: Option<&str>,
    ) -> DomainResult<usize> {
        let mut count = 0;
        let entries = archive.entries().map_err(|e| {
            DomainError::Storage(format!("Impossible de lire les entrées tar : {}", e))
        })?;

        for entry_res in entries {
            let mut entry = entry_res.map_err(|e| {
                DomainError::Storage(format!("Entrée tar invalide ou corrompue : {}", e))
            })?;

            let entry_path = entry
                .path()
                .map_err(|e| {
                    DomainError::Storage(format!("Chemin d'entrée tar invalide : {}", e))
                })?
                .to_path_buf();

            let path_str = entry_path.to_string_lossy().to_string();

            if let Some(filter) = partial_filter {
                let filter_clean = filter.trim_start_matches('/');
                if !path_str.starts_with(filter_clean) {
                    continue;
                }
            }

            // Protection anti-traversée de répertoire (zip-slip)
            let dest = target_dir.join(&entry_path);
            if !dest.starts_with(target_dir) {
                return Err(DomainError::Integrity {
                    resource: path_str,
                    message: "Tentative d'écriture hors du répertoire cible détectée".to_string(),
                });
            }

            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }

            let entry_path_display = entry_path.display().to_string();
            entry.unpack(&dest).map_err(|e| {
                DomainError::Storage(format!(
                    "Échec d'extraction de l'entrée {} vers {:?} : {}",
                    entry_path_display, dest, e
                ))
            })?;

            count += 1;
        }

        Ok(count)
    }
}

impl Default for FilesystemProvider {
    fn default() -> Self {
        Self::new()
    }
}
