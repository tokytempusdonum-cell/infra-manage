//! Module des cas d'usage et d'orchestration (actions).
//!
//! Chaque action représente une opération métier complète :
//! sauvegarde, restauration, rétention, vérification d'intégrité, historique et planification.

pub mod backup_action;
pub mod history_action;
pub mod restore_action;
pub mod retention_action;
pub mod scheduler_action;
pub mod validation_action;

pub use backup_action::*;
pub use history_action::*;
pub use restore_action::*;
pub use retention_action::*;
pub use scheduler_action::*;
pub use validation_action::*;
