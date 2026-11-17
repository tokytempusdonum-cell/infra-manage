//! Adaptateur pour l'envoi de notifications d'état et d'alerte (Slack / Webhook).

use tracing::{debug, info, warn};

use crate::core::abstract_traits::{CommandExecutor, NotificationService};
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::NotificationConfig;

/// Fournisseur d'alerting et de notification via webhook HTTP.
pub struct WebhookNotificationService<'a> {
    config: Option<NotificationConfig>,
    executor: &'a dyn CommandExecutor,
}

impl<'a> WebhookNotificationService<'a> {
    /// Crée un nouveau service de notification avec configuration optionnelle.
    pub fn new(config: Option<NotificationConfig>, executor: &'a dyn CommandExecutor) -> Self {
        Self { config, executor }
    }

    /// Résout l'URL effective du webhook depuis la configuration ou l'environnement.
    fn resolve_url(&self) -> Option<String> {
        let cfg = self.config.as_ref()?;
        if !cfg.enabled {
            return None;
        }

        if let Some(env_name) = &cfg.slack_webhook_env {
            if let Ok(val) = std::env::var(env_name) {
                if !val.trim().is_empty() {
                    return Some(val);
                }
            }
        }

        if let Some(url) = &cfg.slack_webhook_url {
            if !url.trim().is_empty() {
                return Some(url.clone());
            }
        }

        None
    }
}

impl<'a> NotificationService for WebhookNotificationService<'a> {
    fn notify(&self, message: &str) -> DomainResult<()> {
        let url = match self.resolve_url() {
            Some(u) => u,
            None => {
                debug!("[LOG NOTIFICATION] Webhook non configuré ou désactivé, notification ignorée");
                return Ok(());
            }
        };

        info!("[LOG NOTIFICATION] Envoi d'une notification webhook : {}", message);

        let json_body = serde_json::json!({
            "text": message
        });
        let payload_str = json_body.to_string();

        let args = [
            "-sS",
            "--max-time",
            "20",
            "-X",
            "POST",
            "-H",
            "Content-type: application/json",
            "--data",
            &payload_str,
            &url,
        ];

        let output = self.executor.execute("curl", &args, &[])?;

        if !output.success {
            warn!(
                "[LOG NOTIFICATION] Échec de l'envoi de notification (code {}) : {}",
                output.exit_code, output.stderr
            );
            return Err(DomainError::Notification(format!(
                "Échec de la requête webhook curl : {}",
                output.stderr
            )));
        }

        info!("[LOG NOTIFICATION] Notification envoyée avec succès.");
        Ok(())
    }
}
