//! Configuration centralisée de la journalisation (console et fichier).

use std::fs::{self, OpenOptions};
use std::path::Path;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialise le système de journalisation global.
///
/// Si un chemin de fichier de log est spécifié, les messages y sont écrits
/// simultanément avec la sortie console standard.
pub fn init_logging(log_level: &str, log_file: Option<&Path>) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(log_level.to_lowercase()));

    let console_layer = fmt::layer()
        .with_target(false)
        .with_level(true)
        .with_span_events(FmtSpan::NONE);

    if let Some(path) = log_file {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if let Ok(file) = OpenOptions::new().create(true).append(true).open(path) {
            let file_layer = fmt::layer()
                .with_target(false)
                .with_ansi(false)
                .with_writer(file);

            let subscriber = tracing_subscriber::registry()
                .with(filter)
                .with(console_layer)
                .with(file_layer);

            let _ = subscriber.try_init();
            return;
        }
    }

    let subscriber = tracing_subscriber::registry()
        .with(filter)
        .with(console_layer);

    let _ = subscriber.try_init();
}
