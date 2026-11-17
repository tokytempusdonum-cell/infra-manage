//! Compression et décompression en flux de fichiers uniques (dumps SQL).
//!
//! La détection du format se fait sur les octets d'en-tête (magic bytes) et non sur
//! l'extension : d'anciennes sauvegardes MySQL nommées `.sql.gz` contiennent du SQL en clair
//! et doivent rester restaurables.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::CompressionAlgorithm;

/// Détecte l'algorithme de compression d'un fichier d'après ses premiers octets.
pub fn detect_compression(path: &Path) -> DomainResult<CompressionAlgorithm> {
    let mut header = [0u8; 4];
    let n = File::open(path)?.read(&mut header)?;
    if n >= 2 && header[..2] == [0x1F, 0x8B] {
        Ok(CompressionAlgorithm::Gzip)
    } else if n >= 4 && header == [0x28, 0xB5, 0x2F, 0xFD] {
        Ok(CompressionAlgorithm::Zstd)
    } else {
        Ok(CompressionAlgorithm::None)
    }
}

/// Compresse `src` vers `dst` avec l'algorithme demandé et retourne la taille du fichier produit.
pub fn compress_file(
    src: &Path,
    dst: &Path,
    algo: CompressionAlgorithm,
    level: Option<i32>,
) -> DomainResult<u64> {
    let mut reader = BufReader::new(File::open(src)?);
    let writer = BufWriter::new(File::create(dst)?);

    let storage_err = |what: &str, e: io::Error| {
        DomainError::Storage(format!("Échec de compression {} : {}", what, e))
    };

    match algo {
        CompressionAlgorithm::Gzip => {
            let level = level
                .map(|l| Compression::new(l.clamp(0, 9) as u32))
                .unwrap_or_else(Compression::default);
            let mut enc = GzEncoder::new(writer, level);
            io::copy(&mut reader, &mut enc).map_err(|e| storage_err("gzip", e))?;
            let mut w = enc.finish().map_err(|e| storage_err("gzip", e))?;
            w.flush().map_err(|e| storage_err("gzip", e))?;
        }
        CompressionAlgorithm::Zstd => {
            let mut enc = zstd::Encoder::new(writer, level.unwrap_or(3))
                .map_err(|e| storage_err("zstd", e))?;
            io::copy(&mut reader, &mut enc).map_err(|e| storage_err("zstd", e))?;
            let mut w = enc.finish().map_err(|e| storage_err("zstd", e))?;
            w.flush().map_err(|e| storage_err("zstd", e))?;
        }
        CompressionAlgorithm::None => {
            let mut w = writer;
            io::copy(&mut reader, &mut w).map_err(|e| storage_err("none", e))?;
            w.flush().map_err(|e| storage_err("none", e))?;
        }
    }

    Ok(fs::metadata(dst)?.len())
}

/// Fichier d'entrée prêt à être envoyé sur le stdin d'un client SQL.
///
/// Si la sauvegarde est compressée, un fichier temporaire décompressé est créé à côté
/// de l'original et supprimé automatiquement à la destruction de la garde.
pub struct RestoreInput {
    path: PathBuf,
    temp: Option<PathBuf>,
}

impl RestoreInput {
    /// Prépare le fichier à restaurer (décompression éventuelle d'après les magic bytes).
    pub fn prepare(file: &Path) -> DomainResult<Self> {
        let algo = detect_compression(file)?;
        if algo == CompressionAlgorithm::None {
            return Ok(Self {
                path: file.to_path_buf(),
                temp: None,
            });
        }

        let temp = PathBuf::from(format!("{}.restore.part", file.display()));
        let result: io::Result<()> = (|| {
            let reader = BufReader::new(File::open(file)?);
            let mut writer = BufWriter::new(File::create(&temp)?);
            match algo {
                CompressionAlgorithm::Gzip => {
                    io::copy(&mut GzDecoder::new(reader), &mut writer)?;
                }
                CompressionAlgorithm::Zstd => {
                    io::copy(&mut zstd::Decoder::new(reader)?, &mut writer)?;
                }
                CompressionAlgorithm::None => unreachable!(),
            }
            writer.flush()
        })();

        if let Err(e) = result {
            let _ = fs::remove_file(&temp);
            return Err(DomainError::Storage(format!(
                "Échec de décompression de {:?} avant restauration : {}",
                file, e
            )));
        }

        Ok(Self {
            path: temp.clone(),
            temp: Some(temp),
        })
    }

    /// Chemin du fichier SQL en clair à fournir au client.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RestoreInput {
    fn drop(&mut self) {
        if let Some(t) = &self.temp {
            let _ = fs::remove_file(t);
        }
    }
}
