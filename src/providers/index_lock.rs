//! Verrou court et bloquant protégeant les fichiers d'index partagés (manifest.json, history.json).
//!
//! Plusieurs profils peuvent s'exécuter en parallèle (un verrou par profil, voir
//! `AppConfig::profile_lock_path`). Ils écrivent pourtant dans les mêmes fichiers d'index
//! du dossier de sauvegarde : chaque lecture-modification-écriture doit donc être sérialisée.

use std::fs::{self, OpenOptions};
use std::path::Path;

use fs2::FileExt;

use crate::core::exceptions::DomainResult;

/// Nom du fichier de verrou d'index, créé dans le dossier de sauvegarde.
pub const INDEX_LOCK_FILENAME: &str = ".index.lock";

/// Exécute `f` en détenant le verrou d'index exclusif du dossier `dir`.
///
/// L'acquisition est bloquante (la section protégée dure quelques millisecondes).
/// `f` ne doit pas rappeler `with_index_lock` sur le même dossier (interblocage).
pub fn with_index_lock<T, F>(dir: &Path, f: F) -> DomainResult<T>
where
    F: FnOnce() -> DomainResult<T>,
{
    fs::create_dir_all(dir)?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(INDEX_LOCK_FILENAME))?;
    file.lock_exclusive()?;
    let result = f();
    let _ = file.unlock();
    result
}
