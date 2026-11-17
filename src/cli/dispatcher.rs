//! Aiguillage des commandes CLI vers les cas d'usage correspondants.

use std::path::{Path, PathBuf};

use crate::actions::backup_action::BackupAction;
use crate::actions::history_action::HistoryAction;
use crate::actions::restore_action::RestoreAction;
use crate::actions::retention_action::RetentionAction;
use crate::actions::scheduler_action::SchedulerAction;
use crate::actions::validation_action::{ValidateConfigAction, VerifyBackupAction};
use crate::cli::args::{
    BackupSubcommands, Commands, ConfigSubcommands, RestoreSubcommands,
    RetentionSubcommands, SchedulerSubcommands,
};
use crate::core::abstract_traits::{CommandExecutor, LockService, NotificationService, StorageEngine};
use crate::core::exceptions::DomainResult;
use crate::providers::restic_provider::ResticProvider;

/// Aiguilleur central exécutant les commandes demandées.
pub struct CommandDispatcher<'a> {
    executor: &'a dyn CommandExecutor,
    storage: &'a dyn StorageEngine,
    lock_service: &'a dyn LockService,
    notifier: &'a dyn NotificationService,
}

impl<'a> CommandDispatcher<'a> {
    /// Initialise le dispatcher avec les dépendances injectées.
    pub fn new(
        executor: &'a dyn CommandExecutor,
        storage: &'a dyn StorageEngine,
        lock_service: &'a dyn LockService,
        notifier: &'a dyn NotificationService,
    ) -> Self {
        Self {
            executor,
            storage,
            lock_service,
            notifier,
        }
    }

    /// Résout le chemin de configuration à utiliser.
    fn resolve_config_path<'b>(&self, cli_default: &'b Path, opt_path: Option<&'b PathBuf>) -> &'b Path {
        if let Some(p) = opt_path {
            p.as_path()
        } else {
            cli_default
        }
    }

    /// Exécute la commande demandée.
    pub fn dispatch(&self, command: Commands, default_config: &Path) -> DomainResult<()> {
        match command {
            Commands::Config(args) => match args.command {
                ConfigSubcommands::Validate { file } => {
                    let config = ValidateConfigAction::execute(&file)?;
                    println!("Configuration valide ({:?})", file);
                    println!("Version : {}", config.version);
                    println!("Dossier de sauvegarde : {:?}", config.settings.backup_directory);
                    println!("Profils détectés ({}):", config.profiles.len());
                    for p in &config.profiles {
                        println!(
                            "  - {} (type: {:?}, actif: {})",
                            p.name, p.profile_type, p.enabled
                        );
                    }
                    Ok(())
                }
                ConfigSubcommands::List { file } => {
                    let path = self.resolve_config_path(default_config, file.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    println!("Profils disponibles dans {:?} :", path);
                    for p in &config.profiles {
                        let sched = p.schedule.as_deref().unwrap_or("manuel");
                        println!(
                            "  * {:<15} | type: {:<10} | actif: {:<5} | planning: {}",
                            p.name,
                            format!("{:?}", p.profile_type).to_lowercase(),
                            p.enabled,
                            sched
                        );
                    }
                    Ok(())
                }
            },
            Commands::Backup(args) => match args.command {
                BackupSubcommands::Run {
                    config: opt_cfg,
                    profile,
                    dry_run,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let action = BackupAction::new(
                        self.executor,
                        self.storage,
                        self.lock_service,
                        self.notifier,
                    );
                    let count = action.execute(&config, profile.as_deref(), dry_run)?;
                    println!("Sauvegarde terminée : {} profil(s) exécuté(s).", count);
                    Ok(())
                }
                BackupSubcommands::List {
                    config: opt_cfg,
                    profile,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let base_dir = &config.settings.backup_directory;
                    let backups = self.storage.list_backups(base_dir, profile.as_deref())?;

                    if backups.is_empty() {
                        println!("Aucune sauvegarde trouvée dans {:?}", base_dir);
                    } else {
                        println!("Sauvegardes disponibles ({}):", backups.len());
                        println!(
                            "{:<35} | {:<12} | {:<10} | {:<19} | {}",
                            "IDENTIFIANT", "PROFIL", "TYPE", "DATE", "TAILLE"
                        );
                        println!("{:-<95}", "");
                        for b in &backups {
                            let size_mb = b.file_size as f64 / 1_048_576.0;
                            let date_str = b.created_at.format("%Y-%m-%d %H:%M:%S").to_string();
                            println!(
                                "{:<35} | {:<12} | {:<10} | {:<19} | {:.2} Mo",
                                b.id,
                                b.profile_name,
                                format!("{:?}", b.profile_type).to_lowercase(),
                                date_str,
                                size_mb
                            );
                        }
                    }
                    Ok(())
                }
                BackupSubcommands::Info {
                    backup_id,
                    config: opt_cfg,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let base_dir = &config.settings.backup_directory;
                    let meta = self.storage.find_backup(base_dir, &backup_id)?;

                    match meta {
                        Some(m) => {
                            println!("Détails de la sauvegarde : {}", m.id);
                            println!("  Profil       : {}", m.profile_name);
                            println!("  Type         : {:?}", m.profile_type);
                            println!("  Date         : {}", m.created_at.format("%Y-%m-%d %H:%M:%S UTC"));
                            println!("  Fichier      : {:?}", m.file_path);
                            println!("  Taille       : {} octets ({:.2} Mo)", m.file_size, m.file_size as f64 / 1_048_576.0);
                            println!("  SHA-256      : {}", m.checksum_sha256);
                            println!("  Compression  : {:?}", m.compression);
                            println!("  Statut       : {:?}", m.status);
                            println!("  Durée        : {:.2}s", m.duration_seconds);
                        }
                        None => {
                            println!("Sauvegarde introuvable : '{}'", backup_id);
                        }
                    }
                    Ok(())
                }
                BackupSubcommands::Verify {
                    backup_id,
                    config: opt_cfg,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let base_dir = &config.settings.backup_directory;
                    let verify_action = VerifyBackupAction::new(self.storage);
                    let valid = verify_action.execute(base_dir, &backup_id)?;

                    if valid {
                        println!("Intégrité VALIDÉE avec succès pour '{}'", backup_id);
                    }
                    Ok(())
                }
            },
            Commands::Restore(args) => match args.command {
                RestoreSubcommands::Run {
                    backup_id,
                    config: opt_cfg,
                    target,
                    dest_db,
                    partial,
                    force,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let action = RestoreAction::new(self.executor, self.storage);
                    action.execute(
                        &config,
                        &backup_id,
                        target.as_deref(),
                        dest_db.as_deref(),
                        partial.as_deref(),
                        force,
                    )?;
                    println!("Restauration réussie pour '{}'", backup_id);
                    Ok(())
                }
            },
            Commands::Retention(args) => match args.command {
                RetentionSubcommands::Run {
                    config: opt_cfg,
                    profile,
                    dry_run,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let restic_provider = Some(ResticProvider::new(self.executor));
                    let action = RetentionAction::new(self.storage, restic_provider);
                    let purged = action.execute(&config, profile.as_deref(), dry_run)?;
                    println!("Rétention terminée : {} sauvegarde(s) purgée(s).", purged);
                    Ok(())
                }
            },
            Commands::History(args) => {
                let path = self.resolve_config_path(default_config, args.config.as_ref());
                let config = ValidateConfigAction::execute(path)?;
                let base_dir = &config.settings.backup_directory;
                let list = HistoryAction::list(base_dir)?;

                if list.is_empty() {
                    println!("Aucun historique d'opération enregistré.");
                } else {
                    println!("Historique des opérations récentes :");
                    println!(
                        "{:<20} | {:<10} | {:<10} | {:<12} | {}",
                        "DATE", "OPÉRATION", "STATUT", "PROFIL", "DÉTAILS"
                    );
                    println!("{:-<85}", "");
                    for h in list.into_iter().take(args.limit) {
                        let date_str = h.timestamp.format("%Y-%m-%d %H:%M:%S").to_string();
                        let prof = h.profile.as_deref().unwrap_or("-");
                        println!(
                            "{:<20} | {:<10} | {:<10} | {:<12} | {}",
                            date_str, h.operation, h.status, prof, h.details
                        );
                    }
                }
                Ok(())
            }
            Commands::Scheduler(args) => match args.command {
                SchedulerSubcommands::Crontab {
                    config: opt_cfg,
                    binary_path,
                } => {
                    let path = self.resolve_config_path(default_config, opt_cfg.as_ref());
                    let config = ValidateConfigAction::execute(path)?;
                    let crontab = SchedulerAction::generate_crontab(
                        &config,
                        path,
                        binary_path.as_deref(),
                    )?;
                    println!("{}", crontab);
                    Ok(())
                }
            },
        }
    }
}
