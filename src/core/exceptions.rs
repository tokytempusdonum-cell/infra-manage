//! Hiérarchie d'exceptions et d'erreurs du domaine de sauvegarde.

use thiserror::Error;

/// Erreur racine du domaine de sauvegarde.
///
/// Toutes les erreurs techniques survenant aux frontières du système
/// (I/O, processus externes, parsing) sont converties en cette hiérarchie
/// afin de préserver la cause racine sans exposer les détails techniques
/// directement au domaine supérieur.
#[derive(Debug, Error)]
pub enum DomainError {
    /// Erreur liée au chargement, au parsing ou à la validation de la configuration YAML.
    #[error("Erreur de configuration : {0}")]
    Config(String),

    /// Erreur levée lors de l'exécution d'une commande externe (ex: docker, pg_dump, mysqldump, restic).
    #[error("Erreur d'exécution de commande [{command}] : {message}")]
    Execution {
        command: String,
        message: String,
        exit_code: Option<i32>,
    },

    /// Erreur levée en cas d'échec de vérification de l'intégrité (somme SHA-256 ou format de fichier).
    #[error("Échec de vérification d'intégrité pour '{resource}' : {message}")]
    Integrity {
        resource: String,
        message: String,
    },

    /// Erreur liée au stockage local ou distant (création de dossier, rclone, snapshot).
    #[error("Erreur de stockage : {0}")]
    Storage(String),

    /// Erreur levée lorsqu'un verrou de concurrence ne peut pas être acquis.
    #[error("Erreur de verrouillage : {0}")]
    Lock(String),

    /// Erreur levée pour protéger une cible contre un écrasement accidentel lors d'une restauration.
    #[error("Sécurité de restauration : la cible '{target}' existe déjà et requiert confirmation ou --force")]
    RestoreSafety { target: String },

    /// Erreur levée lors de l'envoi d'une notification (webhook Slack, etc.).
    #[error("Erreur de notification : {0}")]
    Notification(String),

    /// Erreur I/O système sous-jacente.
    #[error("Erreur I/O système : {0}")]
    Io(#[from] std::io::Error),

    /// Erreur de sérialisation ou désérialisation YAML.
    #[error("Erreur de parsing YAML : {0}")]
    Yaml(#[from] serde_yaml::Error),

    /// Erreur de sérialisation ou désérialisation JSON.
    #[error("Erreur de parsing JSON : {0}")]
    Json(#[from] serde_json::Error),
}

/// Alias standard pour les retours de résultats du domaine.
pub type DomainResult<T> = Result<T, DomainError>;
