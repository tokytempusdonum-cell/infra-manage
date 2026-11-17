//! Tests d'intégration du dispatcher CLI et de ses sous-commandes.

mod common;
use common::TestEnvironment;

use backup_cli::core::abstract_traits::StorageEngine;
use backup_cli::cli::args::{
    BackupCommandGroup, BackupSubcommands, Commands, ConfigArgs, ConfigSubcommands,
    HistoryArgs, SchedulerCommandGroup, SchedulerSubcommands,
};
use backup_cli::cli::dispatcher::CommandDispatcher;

#[test]
fn test_cli_dispatch_config_validate_and_list() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    env.write_config_yaml(&config);

    let storage = env.storage();
    let dispatcher = CommandDispatcher::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    // 1. config validate
    let cmd_validate = Commands::Config(ConfigArgs {
        command: ConfigSubcommands::Validate {
            file: env.config_file.clone(),
        },
    });
    dispatcher
        .dispatch(cmd_validate, &env.config_file)
        .expect("config validate a échoué");

    // 2. config list
    let cmd_list = Commands::Config(ConfigArgs {
        command: ConfigSubcommands::List { file: None },
    });
    dispatcher
        .dispatch(cmd_list, &env.config_file)
        .expect("config list a échoué");
}

#[test]
fn test_cli_dispatch_backup_cycle_and_list_info() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    env.write_config_yaml(&config);

    let storage = env.storage();
    let dispatcher = CommandDispatcher::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    // 1. backup run
    let cmd_backup = Commands::Backup(BackupCommandGroup {
        command: BackupSubcommands::Run {
            config: None,
            profile: Some("authentik".to_string()),
            dry_run: false,
        },
    });
    dispatcher
        .dispatch(cmd_backup, &env.config_file)
        .expect("backup run a échoué");

    // 2. backup list
    let cmd_list = Commands::Backup(BackupCommandGroup {
        command: BackupSubcommands::List {
            config: None,
            profile: None,
        },
    });
    dispatcher
        .dispatch(cmd_list, &env.config_file)
        .expect("backup list a échoué");

    let backups = storage
        .list_backups(&env.backup_dir, Some("authentik"))
        .unwrap();
    assert_eq!(backups.len(), 1);
    let id = backups[0].id.clone();

    // 3. backup info
    let cmd_info = Commands::Backup(BackupCommandGroup {
        command: BackupSubcommands::Info {
            backup_id: id.clone(),
            config: None,
        },
    });
    dispatcher
        .dispatch(cmd_info, &env.config_file)
        .expect("backup info a échoué");

    // 4. backup verify
    let cmd_verify = Commands::Backup(BackupCommandGroup {
        command: BackupSubcommands::Verify {
            backup_id: id,
            config: None,
        },
    });
    dispatcher
        .dispatch(cmd_verify, &env.config_file)
        .expect("backup verify a échoué");

    // 5. history
    let cmd_history = Commands::History(HistoryArgs {
        config: None,
        limit: 10,
    });
    dispatcher
        .dispatch(cmd_history, &env.config_file)
        .expect("history a échoué");
}

#[test]
fn test_cli_dispatch_scheduler_crontab() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    env.write_config_yaml(&config);

    let storage = env.storage();
    let dispatcher = CommandDispatcher::new(
        &env.executor,
        &storage,
        &env.lock_service,
        &env.notifier,
    );

    let cmd_cron = Commands::Scheduler(SchedulerCommandGroup {
        command: SchedulerSubcommands::Crontab {
            config: None,
            binary_path: Some("/opt/backup-cli/bin/backup-cli".to_string()),
        },
    });
    dispatcher
        .dispatch(cmd_cron, &env.config_file)
        .expect("scheduler crontab a échoué");
}
