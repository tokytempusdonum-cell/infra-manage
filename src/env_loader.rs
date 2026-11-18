//! Chargement des variables d'environnement depuis le `.env` associé à la configuration.

use std::io;
use std::path::{Path, PathBuf};

/// Calcule le chemin du `.env` situé dans le même dossier que le fichier de configuration.
///
/// Pour `-c /root/backup-script/backup.yml`, le fichier chargé est
/// `/root/backup-script/.env`. Pour un chemin relatif comme `backup.yml`, le
/// fichier `.env` du dossier courant est utilisé.
pub fn dotenv_path_for_config(config_path: &Path) -> PathBuf {
    match config_path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent.join(".env"),
        _ => PathBuf::from(".env"),
    }
}

/// Charge le `.env` associé à la configuration, s'il existe.
///
/// Les variables déjà présentes dans l'environnement du processus ne sont pas
/// écrasées. Un `.env` absent n'est pas une erreur : la fonction retourne
/// simplement `Ok(None)`.
pub fn load_dotenv_for_config(config_path: &Path) -> Result<Option<PathBuf>, dotenvy::Error> {
    let path = dotenv_path_for_config(config_path);
    match dotenvy::from_path(&path) {
        Ok(()) => Ok(Some(path)),
        Err(dotenvy::Error::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn dotenv_path_is_next_to_config_file() {
        assert_eq!(
            dotenv_path_for_config(Path::new("/root/backup-script/backup.yml")),
            PathBuf::from("/root/backup-script/.env")
        );
    }

    #[test]
    fn relative_config_uses_current_directory() {
        assert_eq!(
            dotenv_path_for_config(Path::new("backup.yml")),
            PathBuf::from(".env")
        );
    }

    #[test]
    fn missing_dotenv_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("backup.yml");
        assert_eq!(load_dotenv_for_config(&config).unwrap(), None);
    }

    #[test]
    fn dotenv_next_to_config_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("backup.yml");
        fs::write(
            dir.path().join(".env"),
            "INFRA_MANAGE_TEST_DOTENV_VALUE=charge_depuis_le_fichier\n",
        )
        .unwrap();

        std::env::remove_var("INFRA_MANAGE_TEST_DOTENV_VALUE");
        let loaded = load_dotenv_for_config(&config).unwrap();
        assert_eq!(loaded, Some(dir.path().join(".env")));
        assert_eq!(
            std::env::var("INFRA_MANAGE_TEST_DOTENV_VALUE").unwrap(),
            "charge_depuis_le_fichier"
        );
        std::env::remove_var("INFRA_MANAGE_TEST_DOTENV_VALUE");
    }
}
