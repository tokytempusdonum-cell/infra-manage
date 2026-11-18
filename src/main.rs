//! Point d'entrée exécutable de l'application CLI backup-cli.

use std::process;
use clap::Parser;
use tracing::error;

use backup_cli::cli::args::Cli;
use backup_cli::cli::dispatcher::CommandDispatcher;
use backup_cli::core::model::AppConfig;
use backup_cli::logging::init_logging;
use backup_cli::providers::command_executor::SystemCommandExecutor;
use backup_cli::providers::lock_provider::SystemLockService;
use backup_cli::providers::notification_provider::WebhookNotificationService;
use backup_cli::providers::storage_provider::DiskStorageEngine;

fn main() {
    let cli = Cli::parse();

    // Lecture optionnelle de la configuration pour configurer la journalisation
    let (log_level, log_file, notif_cfg) = if cli.config.exists() {
        if let Ok(content) = std::fs::read_to_string(&cli.config) {
            if let Ok(cfg) = serde_yaml::from_str::<AppConfig>(&content) {
                (
                    cfg.settings.log_level.clone(),
                    Some(cfg.settings.log_file.clone()),
                    cfg.settings.notifications.clone(),
                )
            } else {
                ("info".to_string(), None, None)
            }
        } else {
            ("info".to_string(), None, None)
        }
    } else {
        ("info".to_string(), None, None)
    };

    init_logging(&log_level, log_file.as_deref());

    let executor = SystemCommandExecutor::new();
    let storage = DiskStorageEngine::new(&executor);
    let lock_service = SystemLockService::new();
    let notifier = WebhookNotificationService::new(notif_cfg);

    let dispatcher = CommandDispatcher::new(&executor, &storage, &lock_service, &notifier);

    if let Err(e) = dispatcher.dispatch(cli.command, &cli.config) {
        error!("[LOG MAIN] Erreur critique : {}", e);
        eprintln!("\n❌ Erreur : {}", e);
        process::exit(1);
    }
}
