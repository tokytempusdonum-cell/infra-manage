//! Adaptateur pour l'envoi de notifications d'état et d'alerte (Slack / webhook).
//!
//! Les requêtes HTTP sont émises par la bibliothèque Rust `ureq` (rustls), sans processus
//! externe `curl`. Le transport est abstrait par `HttpPoster` pour pouvoir être testé.

use std::time::Duration;

use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::core::abstract_traits::NotificationService;
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{BackupReport, NotificationConfig};

/// Délai maximal d'une requête vers le webhook.
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// Taille maximale du texte d'un bloc Slack « section » (limite Slack : 3000 caractères).
const SLACK_SECTION_LIMIT: usize = 2800;

/// Port de transport HTTP minimal : POST d'un corps JSON.
pub trait HttpPoster: Send + Sync {
    /// Envoie `body` (JSON) en POST vers `url`. Ne doit jamais inclure `url` dans ses erreurs
    /// (l'URL d'un webhook Slack est un secret).
    fn post_json(&self, url: &str, body: &str) -> DomainResult<()>;
}

/// Transport HTTP réel basé sur la bibliothèque `ureq`.
pub struct UreqHttpPoster {
    timeout: Duration,
}

impl UreqHttpPoster {
    /// Crée le transport avec le délai par défaut (20 s).
    pub fn new() -> Self {
        Self {
            timeout: HTTP_TIMEOUT,
        }
    }
}

impl Default for UreqHttpPoster {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpPoster for UreqHttpPoster {
    fn post_json(&self, url: &str, body: &str) -> DomainResult<()> {
        let agent = ureq::AgentBuilder::new().timeout(self.timeout).build();

        match agent
            .post(url)
            .set("Content-Type", "application/json")
            .send_string(body)
        {
            Ok(_) => Ok(()),
            Err(ureq::Error::Status(code, response)) => {
                let detail = response.into_string().unwrap_or_default();
                Err(DomainError::Notification(format!(
                    "le webhook a répondu HTTP {} : {}",
                    code,
                    detail.trim()
                )))
            }
            Err(ureq::Error::Transport(t)) => Err(DomainError::Notification(format!(
                "connexion au webhook impossible ({:?}){}",
                t.kind(),
                t.message().map(|m| format!(" : {}", m)).unwrap_or_default()
            ))),
        }
    }
}

/// Échappe les caractères spéciaux du mrkdwn Slack.
fn slack_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Construit le message Slack (texte + pièce jointe colorée en blocs) pour un bilan de cycle.
pub fn build_slack_payload(report: &BackupReport) -> Value {
    let color = if report.failed() == 0 {
        "#2eb67d" // vert
    } else if report.succeeded() == 0 {
        "#e01e5a" // rouge
    } else {
        "#ecb22e" // orange
    };

    let lines: Vec<String> = report
        .sorted_profiles()
        .into_iter()
        .map(|p| {
            let name = slack_escape(&p.name);
            if p.is_success() {
                format!(
                    "✅ *{}* ({}) — {}",
                    name,
                    p.profile_type.label(),
                    slack_escape(&p.summary())
                )
            } else {
                let error = slack_escape(&crate::core::model::short_error(
                    p.detail.as_deref().unwrap_or("erreur inconnue"),
                ))
                .replace('`', "'");
                format!(
                    "❌ *{}* ({}) — échec : `{}`",
                    name,
                    p.profile_type.label(),
                    error
                )
            }
        })
        .collect();

    let mut blocks = vec![json!({
        "type": "section",
        "fields": [
            { "type": "mrkdwn", "text": format!("*Total*\n{}", report.total()) },
            { "type": "mrkdwn", "text": format!("*Réussies*\n✅ {}", report.succeeded()) },
            { "type": "mrkdwn", "text": format!("*Échecs*\n❌ {}", report.failed()) },
            { "type": "mrkdwn", "text": format!(
                "*Durée*\n{}", crate::core::model::format_duration(report.duration_seconds)
            ) },
        ]
    })];

    // Détail par profil, découpé en blocs pour respecter la limite de taille de Slack.
    let mut chunk = String::new();
    for line in lines {
        if !chunk.is_empty() && chunk.chars().count() + line.chars().count() + 1 > SLACK_SECTION_LIMIT {
            blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": chunk } }));
            chunk = String::new();
        }
        if !chunk.is_empty() {
            chunk.push('\n');
        }
        chunk.push_str(&line);
    }
    if !chunk.is_empty() {
        blocks.push(json!({ "type": "section", "text": { "type": "mrkdwn", "text": chunk } }));
    }

    blocks.push(json!({
        "type": "context",
        "elements": [{
            "type": "mrkdwn",
            "text": format!(
                "🖥️ {} · {} UTC",
                slack_escape(&report.hostname),
                report.started_at.format("%Y-%m-%d %H:%M")
            )
        }]
    }));

    json!({
        "text": report.headline(),
        "attachments": [{ "color": color, "blocks": blocks }]
    })
}

/// Fournisseur d'alerting et de notification via webhook HTTP (Slack).
pub struct WebhookNotificationService {
    config: Option<NotificationConfig>,
    poster: Box<dyn HttpPoster>,
}

impl WebhookNotificationService {
    /// Crée le service avec le transport HTTP réel (`ureq`).
    pub fn new(config: Option<NotificationConfig>) -> Self {
        Self::with_poster(config, Box::new(UreqHttpPoster::new()))
    }

    /// Crée le service avec un transport HTTP personnalisé (tests).
    pub fn with_poster(config: Option<NotificationConfig>, poster: Box<dyn HttpPoster>) -> Self {
        Self { config, poster }
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

    fn send(&self, payload: &Value, description: &str) -> DomainResult<()> {
        let url = match self.resolve_url() {
            Some(u) => u,
            None => {
                debug!("[LOG NOTIFICATION] Webhook non configuré ou désactivé, notification ignorée");
                return Ok(());
            }
        };

        info!("[LOG NOTIFICATION] Envoi d'une notification : {}", description);

        match self.poster.post_json(&url, &payload.to_string()) {
            Ok(()) => {
                info!("[LOG NOTIFICATION] Notification envoyée avec succès.");
                Ok(())
            }
            Err(e) => {
                warn!("[LOG NOTIFICATION] Échec de l'envoi de la notification : {}", e);
                Err(e)
            }
        }
    }
}

impl NotificationService for WebhookNotificationService {
    fn is_configured(&self) -> bool {
        self.resolve_url().is_some()
    }

    fn notify(&self, message: &str) -> DomainResult<()> {
        self.send(&json!({ "text": message }), message)
    }

    fn notify_report(&self, report: &BackupReport) -> DomainResult<()> {
        self.send(&build_slack_payload(report), &report.headline())
    }
}
