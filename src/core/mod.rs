//! Module du domaine métier : modèles, ports abstraits et exceptions.
//!
//! Ce module centralise les entités pures du système. Il ne possède aucune
//! dépendance externe et constitue le cœur du modèle hexagonal.

pub mod abstract_traits;
pub mod exceptions;
pub mod model;

pub use abstract_traits::*;
pub use exceptions::*;
pub use model::*;
