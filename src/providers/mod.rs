//! Module des adaptateurs techniques (providers) vers le monde extérieur.
//!
//! Ce module implémente les interfaces définies dans le domaine `core/` :
//! interaction avec Docker, bases de données, système de fichiers, restic, stockage et notifications.

pub mod command_executor;
pub mod database_provider;
pub mod docker_provider;
pub mod filesystem_provider;
pub mod index_lock;
pub mod lock_provider;
pub mod notification_provider;
pub mod restic_provider;
pub mod storage_provider;
pub mod stream_compression;

pub use command_executor::*;
pub use database_provider::*;
pub use docker_provider::*;
pub use filesystem_provider::*;
pub use index_lock::*;
pub use lock_provider::*;
pub use notification_provider::*;
pub use restic_provider::*;
pub use storage_provider::*;
pub use stream_compression::*;
