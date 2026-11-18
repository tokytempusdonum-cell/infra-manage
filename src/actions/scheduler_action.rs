//! Action de planification et de génération de crontab système.

use std::path::Path;
use tracing::info;

use crate::core::exceptions::DomainResult;
use crate::core::model::AppConfig;

/// Action de gestion du planning des sauvegardes.
pub struct SchedulerAction;

impl SchedulerAction {
    /// Génère les lignes de crontab correspondant aux profils configurés.
    pub fn generate_crontab(
        config: &AppConfig,
        config_path: &Path,
        binary_path: Option<&str>,
    ) -> DomainResult<String> {
        let bin = binary_path.unwrap_or("infra-manage");
        let cfg_str = config_path.to_string_lossy();

        let mut lines = Vec::new();
        lines.push("# === infra-manage planificateur automatique ===".to_string());
        lines.push(format!(
            "# Fichier de configuration : {}",
            cfg_str
        ));
        lines.push(format!(
            "# Journal central : {:?}",
            config.settings.log_file
        ));
        lines.push("".to_string());

        let mut count = 0;
        for profile in &config.profiles {
            if !profile.enabled {
                continue;
            }

            if let Some(ref cron_expr) = profile.schedule {
                lines.push(format!("# Profil : {}", profile.name));
                lines.push(format!(
                    "{} {} backup run --config {} --profile {} >> {} 2>&1",
                    cron_expr,
                    bin,
                    cfg_str,
                    profile.name,
                    config.settings.log_file.display()
                ));
                lines.push("".to_string());
                count += 1;
            }
        }

        info!(
            "[LOG SCHEDULER] Crontab générée pour {} profil(s) planifié(s)",
            count
        );
        Ok(lines.join("\n"))
    }
}
