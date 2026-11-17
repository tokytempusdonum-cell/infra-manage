//! Tests unitaires du planificateur et de la génération de crontab.

mod common;
use common::TestEnvironment;

use backup_cli::actions::scheduler_action::SchedulerAction;

#[test]
fn test_generate_crontab_from_profiles() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();

    let crontab = SchedulerAction::generate_crontab(
        &config,
        &env.config_file,
        Some("/usr/local/bin/backup-cli"),
    )
    .expect("Génération crontab échouée");

    assert!(crontab.contains("0 2 * * * /usr/local/bin/backup-cli backup run --config"));
    assert!(crontab.contains("--profile authentik"));
    assert!(crontab.contains("0 3 * * * /usr/local/bin/backup-cli backup run --config"));
    assert!(crontab.contains("--profile ghost"));
    assert!(crontab.contains("0 4 * * * /usr/local/bin/backup-cli backup run --config"));
    assert!(crontab.contains("--profile configs"));
}
