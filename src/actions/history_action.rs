//! Action d'enregistrement et de consultation de l'historique des opérations.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter};
use std::path::Path;
use chrono::Utc;
use tracing::debug;

use crate::core::exceptions::DomainResult;
use crate::core::model::HistoryEntry;

/// Nom du fichier d'historique persistant.
pub const HISTORY_FILENAME: &str = "history.json";

/// Action de consultation et d'enregistrement de l'historique des opérations.
pub struct HistoryAction;

impl HistoryAction {
    /// Enregistre une nouvelle entrée dans le journal d'historique.
    pub fn record(
        base_dir: &Path,
        operation: &str,
        profile: Option<&str>,
        backup_id: Option<&str>,
        status: &str,
        details: &str,
    ) -> DomainResult<()> {
        let _ = fs::create_dir_all(base_dir);
        let history_path = base_dir.join(HISTORY_FILENAME);

        let mut list = Self::list(base_dir).unwrap_or_default();

        let entry = HistoryEntry {
            timestamp: Utc::now(),
            operation: operation.to_string(),
            profile: profile.map(|s| s.to_string()),
            backup_id: backup_id.map(|s| s.to_string()),
            status: status.to_string(),
            details: details.to_string(),
        };

        list.push(entry);

        // Limite de taille de l'historique (dernières 1000 entrées)
        if list.len() > 1000 {
            list.drain(0..(list.len() - 1000));
        }

        let temp_path = base_dir.join(format!("{}.part", HISTORY_FILENAME));
        let file = File::create(&temp_path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer_pretty(writer, &list)?;
        fs::rename(temp_path, history_path)?;

        debug!(
            "[LOG HISTORY] Opération '{}' enregistrée ({})",
            operation, status
        );
        Ok(())
    }

    /// Liste toutes les entrées d'historique enregistrées, triées par ordre antichronologique.
    pub fn list(base_dir: &Path) -> DomainResult<Vec<HistoryEntry>> {
        let history_path = base_dir.join(HISTORY_FILENAME);
        if !history_path.exists() {
            return Ok(Vec::new());
        }

        let file = File::open(&history_path)?;
        let reader = BufReader::new(file);
        let mut list: Vec<HistoryEntry> = serde_json::from_reader(reader).unwrap_or_default();
        list.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        Ok(list)
    }
}
