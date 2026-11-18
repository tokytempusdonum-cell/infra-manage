//! Bibliothèque principale de l'application infra-manage.
//!
//! Conçue selon une architecture hexagonale (ports et adaptateurs) :
//! - `core` : modèles de données et interfaces du domaine.
//! - `providers` : adaptateurs techniques vers le système et les outils tiers.
//! - `actions` : cas d'usage d'orchestration métier.
//! - `cli` : interface en ligne de commande.
//! - `logging` : journalisation centralisée.

pub mod actions;
pub mod cli;
pub mod core;
pub mod logging;
pub mod providers;
