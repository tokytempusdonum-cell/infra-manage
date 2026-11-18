//! Adaptateur pour les opérations de sauvegarde incrémentale et dédupliquée avec Restic.

use std::path::Path;
use chrono::{DateTime, Utc};
use tracing::{debug, info};

use crate::core::abstract_traits::CommandExecutor;
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::{BackupProfile, RetentionPolicy};

/// Fournisseur d'intégration avec l'outil de snapshot Restic.
pub struct ResticProvider<'a> {
    executor: &'a dyn CommandExecutor,
}

impl<'a> ResticProvider<'a> {
    /// Initialise le fournisseur avec l'exécuteur injecté.
    pub fn new(executor: &'a dyn CommandExecutor) -> Self {
        Self { executor }
    }

    /// Construit les variables d'environnement nécessaires pour Restic.
    fn build_envs(&self, profile: &BackupProfile) -> DomainResult<Vec<(String, String)>> {
        let mut envs = Vec::new();
        let src = &profile.source;

        let repo = src
            .restic_repository
            .clone()
            .or_else(|| {
                profile
                    .destination
                    .path
                    .as_ref()
                    .map(|p| p.to_string_lossy().to_string())
            })
            .ok_or_else(|| {
                DomainError::Config(format!(
                    "Profil Restic '{}' : aucun dépôt spécifié",
                    profile.name
                ))
            })?;

        envs.push(("RESTIC_REPOSITORY".to_string(), repo));

        if let Some(pwd_file) = &src.restic_password_file {
            envs.push((
                "RESTIC_PASSWORD_FILE".to_string(),
                pwd_file.to_string_lossy().to_string(),
            ));
        } else if let Some(pwd_env) = &src.restic_password_env {
            if let Ok(pwd) = std::env::var(pwd_env) {
                envs.push(("RESTIC_PASSWORD".to_string(), pwd));
            } else {
                return Err(DomainError::Config(format!(
                    "Profil Restic '{}' : variable d'environnement '{}' introuvable",
                    profile.name, pwd_env
                )));
            }
        }

        Ok(envs)
    }

    /// Initialise le dépôt Restic s'il n'existe pas déjà.
    pub fn ensure_initialized(&self, profile: &BackupProfile) -> DomainResult<()> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        debug!("[LOG RESTIC] Vérification de l'état du dépôt Restic");
        let check_output = self
            .executor
            .execute("restic", &["cat", "config"], &envs_ref)?;

        if !check_output.success {
            info!("[LOG RESTIC] Dépôt non initialisé, tentative d'initialisation (restic init)");
            let init_output = self.executor.execute("restic", &["init"], &envs_ref)?;
            if !init_output.success {
                return Err(DomainError::Execution {
                    command: "restic init".to_string(),
                    message: format!(
                        "Échec d'initialisation du dépôt Restic : {}",
                        init_output.stderr
                    ),
                    exit_code: Some(init_output.exit_code),
                });
            }
            info!("[LOG RESTIC] Dépôt Restic initialisé avec succès.");
        }

        Ok(())
    }

    /// Crée un nouveau snapshot de sauvegarde Restic.
    pub fn backup(&self, profile: &BackupProfile) -> DomainResult<()> {
        self.ensure_initialized(profile)?;

        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let src = &profile.source;
        let paths = src.paths.as_ref().ok_or_else(|| {
            DomainError::Config(format!(
                "Profil Restic '{}' : 'paths' manquant",
                profile.name
            ))
        })?;

        let mut args: Vec<String> = vec!["backup".to_string()];

        let tag = src
            .tags
            .as_ref()
            .and_then(|t| t.first())
            .cloned()
            .unwrap_or_else(|| profile.name.clone());

        args.push("--tag".to_string());
        args.push(tag);
        args.push("--exclude".to_string());
        args.push("*.part".to_string());

        if let Some(excludes) = &src.exclude {
            for exc in excludes {
                args.push("--exclude".to_string());
                args.push(exc.clone());
            }
        }

        let mut existing_paths_count = 0;
        for path in paths {
            if path.exists() {
                args.push(path.to_string_lossy().to_string());
                existing_paths_count += 1;
            } else {
                debug!(
                    "[LOG RESTIC] Chemin source inexistant ignoré : {:?}",
                    path
                );
            }
        }

        if existing_paths_count == 0 {
            return Err(DomainError::Config(format!(
                "Profil Restic '{}' : aucun des chemins configurés n'existe",
                profile.name
            )));
        }

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        info!(
            "[LOG RESTIC] Lancement de restic backup pour le profil '{}'",
            profile.name
        );
        let output = self.executor.execute("restic", &args_ref, &envs_ref)?;

        if !output.success {
            return Err(DomainError::Execution {
                command: "restic backup".to_string(),
                message: format!("Échec de restic backup : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        info!(
            "[LOG RESTIC] Snapshot Restic créé avec succès pour '{}'",
            profile.name
        );
        Ok(())
    }

    /// Applique la politique de rotation des snapshots (forget et prune).
    pub fn forget(&self, profile: &BackupProfile, policy: &RetentionPolicy) -> DomainResult<()> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let tag = profile
            .source
            .tags
            .as_ref()
            .and_then(|t| t.first())
            .cloned()
            .unwrap_or_else(|| profile.name.clone());

        let mut args = vec![
            "forget".to_string(),
            "--tag".to_string(),
            tag,
            "--prune".to_string(),
        ];

        if let Some(daily) = policy.daily {
            args.push("--keep-daily".to_string());
            args.push(daily.to_string());
        }
        if let Some(weekly) = policy.weekly {
            args.push("--keep-weekly".to_string());
            args.push(weekly.to_string());
        }
        if let Some(monthly) = policy.monthly {
            args.push("--keep-monthly".to_string());
            args.push(monthly.to_string());
        }

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        info!(
            "[LOG RESTIC] Application de restic forget pour le profil '{}'",
            profile.name
        );
        let output = self.executor.execute("restic", &args_ref, &envs_ref)?;

        if !output.success {
            return Err(DomainError::Execution {
                command: "restic forget".to_string(),
                message: format!("Échec de restic forget : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        info!(
            "[LOG RESTIC] Rotation des snapshots Restic effectuée pour '{}'",
            profile.name
        );
        Ok(())
    }

    /// Vérifie l'intégrité du dépôt Restic (équivalent à restic check --read-data-subset=...).
    pub fn check(&self, profile: &BackupProfile, subset_percent: Option<u32>) -> DomainResult<()> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let mut args = vec!["check".to_string()];
        if let Some(pct) = subset_percent {
            args.push(format!("--read-data-subset={}%", pct));
        }

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

        info!(
            "[LOG RESTIC] Vérification d'intégrité du dépôt Restic pour '{}'",
            profile.name
        );
        let output = self.executor.execute("restic", &args_ref, &envs_ref)?;

        if !output.success {
            return Err(DomainError::Integrity {
                resource: format!("Dépôt Restic ({})", profile.name),
                message: format!("Échec de restic check : {}", output.stderr),
            });
        }

        info!("[LOG RESTIC] Intégrité du dépôt Restic validée.");
        Ok(())
    }

    /// Restaure un snapshot Restic vers un dossier cible.
    pub fn restore(
        &self,
        profile: &BackupProfile,
        snapshot_id: &str,
        target_dir: &Path,
    ) -> DomainResult<()> {
        self.restore_filtered(profile, snapshot_id, target_dir, &[])
    }

    /// Restaure un snapshot Restic en ne gardant que les chemins `includes` (tout si vide).
    ///
    /// Les chemins sont ceux enregistrés dans le snapshot (absolus, ex: `/opt/authentik`) ;
    /// avec `--target /` ils sont restaurés à leur emplacement d'origine, propriétaires
    /// et permissions compris.
    pub fn restore_filtered(
        &self,
        profile: &BackupProfile,
        snapshot_id: &str,
        target_dir: &Path,
        includes: &[String],
    ) -> DomainResult<()> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let target_str = target_dir.to_string_lossy().to_string();
        let mut args: Vec<&str> = vec!["restore", snapshot_id, "--target", &target_str];
        for inc in includes {
            args.push("--include");
            args.push(inc.as_str());
        }

        info!(
            "[LOG RESTIC] Restauration du snapshot Restic '{}' vers {:?} ({} filtre(s))",
            snapshot_id,
            target_dir,
            includes.len()
        );
        let output = self.executor.execute("restic", &args, &envs_ref)?;

        if !output.success {
            return Err(DomainError::Execution {
                command: "restic restore".to_string(),
                message: format!("Échec de restic restore : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        info!("[LOG RESTIC] Restauration Restic réussie.");
        Ok(())
    }

    /// Liste les snapshots du dépôt (identifiant complet et date), du plus ancien au plus récent.
    pub fn list_snapshots(&self, profile: &BackupProfile) -> DomainResult<Vec<(String, DateTime<Utc>)>> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let output = self
            .executor
            .execute("restic", &["snapshots", "--json"], &envs_ref)?;
        if !output.success {
            return Err(DomainError::Execution {
                command: "restic snapshots".to_string(),
                message: format!("Échec de restic snapshots : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        let parsed: serde_json::Value = serde_json::from_str(output.stdout.trim()).map_err(|e| {
            DomainError::Storage(format!("Sortie 'restic snapshots --json' illisible : {}", e))
        })?;
        let mut snapshots: Vec<(String, DateTime<Utc>)> = parsed
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|item| {
                        let id = item.get("id")?.as_str()?.to_string();
                        let time = item.get("time")?.as_str()?;
                        let time = DateTime::parse_from_rfc3339(time).ok()?.with_timezone(&Utc);
                        Some((id, time))
                    })
                    .collect()
            })
            .unwrap_or_default();
        snapshots.sort_by_key(|(_, t)| *t);
        Ok(snapshots)
    }

    /// Choisit le snapshot à restaurer : `latest`, ou le plus récent antérieur à `before`.
    pub fn pick_snapshot(
        &self,
        profile: &BackupProfile,
        before: Option<DateTime<Utc>>,
    ) -> DomainResult<String> {
        let Some(limit) = before else {
            return Ok("latest".to_string());
        };
        self.list_snapshots(profile)?
            .into_iter()
            .rev()
            .find(|(_, t)| *t <= limit)
            .map(|(id, _)| id)
            .ok_or_else(|| {
                DomainError::Storage(format!(
                    "Aucun snapshot restic antérieur à {}",
                    limit.format("%Y-%m-%d %H:%M:%S UTC")
                ))
            })
    }

    /// Liste les dossiers situés directement sous `root` dans un snapshot (ex: `/opt/*`).
    pub fn list_child_dirs(
        &self,
        profile: &BackupProfile,
        snapshot_id: &str,
        root: &str,
    ) -> DomainResult<Vec<String>> {
        let envs_owned = self.build_envs(profile)?;
        let envs_ref: Vec<(&str, &str)> = envs_owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();

        let output = self
            .executor
            .execute("restic", &["ls", snapshot_id, root, "--json"], &envs_ref)?;
        if !output.success {
            return Err(DomainError::Execution {
                command: "restic ls".to_string(),
                message: format!("Échec de restic ls : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        let root_trimmed = root.trim_end_matches('/');
        let mut dirs: Vec<String> = output
            .stdout
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|v| v.get("type").and_then(|t| t.as_str()) == Some("dir"))
            .filter_map(|v| v.get("path").and_then(|p| p.as_str()).map(|p| p.to_string()))
            .filter(|path| {
                path.strip_prefix(root_trimmed)
                    .map(|rest| rest.starts_with('/') && !rest[1..].is_empty() && !rest[1..].contains('/'))
                    .unwrap_or(false)
            })
            .collect();
        dirs.sort();
        dirs.dedup();
        Ok(dirs)
    }
}
