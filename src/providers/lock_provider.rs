//! Adaptateur de verrouillage exclusif pour la protection contre la concurrence.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use fs2::FileExt;
use tracing::{debug, error, info};

use crate::core::abstract_traits::{LockGuard, LockService};
use crate::core::exceptions::{DomainError, DomainResult};

/// Garde RAII maintenant le fichier verrouillé jusqu'à sa destruction.
pub struct FileLockGuard {
    _file: File,
    lock_path: PathBuf,
}

impl LockGuard for FileLockGuard {}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        debug!(
            "[LOG LOCK] Libération du verrou sur {:?}",
            self.lock_path
        );
        let _ = self._file.unlock();
    }
}

/// Service d'acquisition de verrous système basés sur les fichiers (flock / fcntl).
pub struct SystemLockService;

impl SystemLockService {
    /// Crée une nouvelle instance du service de verrouillage.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemLockService {
    fn default() -> Self {
        Self::new()
    }
}

impl LockService for SystemLockService {
    fn acquire_lock(&self, lock_path: &Path) -> DomainResult<Box<dyn LockGuard>> {
        debug!(
            "[LOG LOCK] Tentative d'acquisition du verrou sur {:?}",
            lock_path
        );

        if let Some(parent) = lock_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|e| {
                DomainError::Lock(format!(
                    "Impossible d'ouvrir le fichier de verrou {:?} : {}",
                    lock_path, e
                ))
            })?;

        match file.try_lock_exclusive() {
            Ok(()) => {
                info!(
                    "[LOG LOCK] Verrou exclusif acquis avec succès sur {:?}",
                    lock_path
                );
                Ok(Box::new(FileLockGuard {
                    _file: file,
                    lock_path: lock_path.to_path_buf(),
                }))
            }
            Err(e) => {
                error!(
                    "[LOG LOCK] Une exécution est déjà en cours (verrou {:?} non disponible) : {}",
                    lock_path, e
                );
                Err(DomainError::Lock(format!(
                    "Une exécution est déjà en cours (verrou {:?} inaccessible)",
                    lock_path
                )))
            }
        }
    }
}
