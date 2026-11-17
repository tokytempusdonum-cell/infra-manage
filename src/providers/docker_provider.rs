//! Adaptateur d'inspection et d'interaction avec l'écosystème Docker.

use tracing::{debug, error, info};
use crate::core::abstract_traits::CommandExecutor;
use crate::core::exceptions::{DomainError, DomainResult};

/// Fournisseur d'opérations Docker sécurisées.
pub struct DockerProvider<'a> {
    executor: &'a dyn CommandExecutor,
}

impl<'a> DockerProvider<'a> {
    /// Initialise le fournisseur avec son exécuteur injecté.
    pub fn new(executor: &'a dyn CommandExecutor) -> Self {
        Self { executor }
    }

    /// Vérifie qu'un conteneur est présent et actif, ou lève une exception du domaine.
    pub fn ensure_container_running(&self, container_name: &str) -> DomainResult<()> {
        debug!(
            "[LOG DOCKER] Vérification de l'état du conteneur '{}'",
            container_name
        );

        let running = self.executor.is_container_running(container_name)?;
        if !running {
            error!(
                "[LOG DOCKER] Le conteneur '{}' n'est pas démarré.",
                container_name
            );
            return Err(DomainError::Execution {
                command: "docker inspect".to_string(),
                message: format!("Le conteneur '{}' n'est pas démarré.", container_name),
                exit_code: Some(1),
            });
        }

        info!(
            "[LOG DOCKER] Le conteneur '{}' est bien en cours d'exécution.",
            container_name
        );
        Ok(())
    }
}
