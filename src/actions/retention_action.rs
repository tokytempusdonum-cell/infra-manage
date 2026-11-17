//! Action de gestion du cycle de vie et de rotation des anciennes sauvegardes.

use std::collections::{HashMap, HashSet};
use chrono::{Datelike, Duration, Utc};
use tracing::info;

use crate::core::abstract_traits::StorageEngine;
use crate::core::exceptions::DomainResult;
use crate::core::model::{AppConfig, BackupMetadata, BackupProfile, ProfileType, RetentionPolicy};
use crate::providers::restic_provider::ResticProvider;

/// Action de purge et de rotation des anciennes sauvegardes selon la politique configurée.
pub struct RetentionAction<'a> {
    storage: &'a dyn StorageEngine,
    restic_provider: Option<ResticProvider<'a>>,
}

impl<'a> RetentionAction<'a> {
    /// Initialise l'action de rétention.
    pub fn new(storage: &'a dyn StorageEngine, restic: Option<ResticProvider<'a>>) -> Self {
        Self {
            storage,
            restic_provider: restic,
        }
    }

    /// Exécute la politique de rétention sur l'ensemble des profils configurés (ou un profil précis).
    pub fn execute(
        &self,
        config: &AppConfig,
        profile_filter: Option<&str>,
        dry_run: bool,
    ) -> DomainResult<usize> {
        info!(
            "[LOG ACTION] Début du cycle de rétention (mode simulation : {})",
            dry_run
        );

        let mut total_deleted = 0;

        for profile in &config.profiles {
            if let Some(target) = profile_filter {
                if !profile.name.eq_ignore_ascii_case(target) {
                    continue;
                }
            }

            let deleted = self.process_profile(config, profile, dry_run)?;
            total_deleted += deleted;
        }

        info!(
            "[LOG ACTION] Rétention terminée : {} sauvegarde(s) supprimée(s)",
            total_deleted
        );
        Ok(total_deleted)
    }

    /// Traite un profil individuel.
    fn process_profile(
        &self,
        config: &AppConfig,
        profile: &BackupProfile,
        dry_run: bool,
    ) -> DomainResult<usize> {
        let policy = match &profile.retention {
            Some(p) => p,
            None => return Ok(0),
        };

        // Si c'est un profil Restic, on délègue la rotation à Restic forget/prune
        if profile.profile_type == ProfileType::Restic {
            if let Some(ref restic) = self.restic_provider {
                if !dry_run {
                    restic.forget(profile, policy)?;
                } else {
                    info!(
                        "[LOG RETENTION] [SIMULATION] Appel restic forget/prune pour '{}'",
                        profile.name
                    );
                }
            }
            return Ok(0);
        }

        let base_dir = config.resolve_backup_dir(profile);
        let backups = self.storage.list_backups(base_dir, Some(&profile.name))?;

        if backups.is_empty() {
            return Ok(0);
        }

        let to_delete = Self::identify_expired_backups(&backups, policy);

        for backup in &to_delete {
            if dry_run {
                info!(
                    "[LOG RETENTION] [SIMULATION] Suppression prévue de '{}' ({:?}, date: {})",
                    backup.id, backup.file_path, backup.created_at
                );
            } else {
                info!(
                    "[LOG RETENTION] Suppression de l'archive expirée '{}'",
                    backup.id
                );
                self.storage.delete_backup(base_dir, backup)?;
            }
        }

        Ok(to_delete.len())
    }

    /// Identifie les sauvegardes qui doivent être purgées selon la politique.
    pub fn identify_expired_backups<'b>(
        backups: &'b [BackupMetadata],
        policy: &RetentionPolicy,
    ) -> Vec<&'b BackupMetadata> {
        let now = Utc::now();
        let mut keep_ids: HashSet<String> = HashSet::new();

        // 1. Politique simple par nombre de jours (style find -mtime +N)
        if let Some(days) = policy.days {
            let threshold = now - Duration::days(days as i64);
            for b in backups {
                if b.created_at >= threshold {
                    keep_ids.insert(b.id.clone());
                }
            }
        }

        // 2. Politique GFS (Grandfather-Father-Son : Daily / Weekly / Monthly)
        let has_gfs = policy.daily.is_some() || policy.weekly.is_some() || policy.monthly.is_some();
        if has_gfs {
            // Regroupement par jour
            if let Some(daily_count) = policy.daily {
                let mut daily_groups: HashMap<String, &BackupMetadata> = HashMap::new();
                for b in backups {
                    let key = b.created_at.format("%Y-%m-%d").to_string();
                    daily_groups
                        .entry(key)
                        .and_modify(|existing| {
                            if b.created_at > existing.created_at {
                                *existing = b;
                            }
                        })
                        .or_insert(b);
                }
                let mut sorted_days: Vec<&BackupMetadata> = daily_groups.into_values().collect();
                sorted_days.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                for b in sorted_days.into_iter().take(daily_count as usize) {
                    keep_ids.insert(b.id.clone());
                }
            }

            // Regroupement par semaine (ISO year-week)
            if let Some(weekly_count) = policy.weekly {
                let mut weekly_groups: HashMap<String, &BackupMetadata> = HashMap::new();
                for b in backups {
                    let key = format!("{}-W{:02}", b.created_at.iso_week().year(), b.created_at.iso_week().week());
                    weekly_groups
                        .entry(key)
                        .and_modify(|existing| {
                            if b.created_at > existing.created_at {
                                *existing = b;
                            }
                        })
                        .or_insert(b);
                }
                let mut sorted_weeks: Vec<&BackupMetadata> = weekly_groups.into_values().collect();
                sorted_weeks.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                for b in sorted_weeks.into_iter().take(weekly_count as usize) {
                    keep_ids.insert(b.id.clone());
                }
            }

            // Regroupement par mois (YYYY-MM)
            if let Some(monthly_count) = policy.monthly {
                let mut monthly_groups: HashMap<String, &BackupMetadata> = HashMap::new();
                for b in backups {
                    let key = b.created_at.format("%Y-%m").to_string();
                    monthly_groups
                        .entry(key)
                        .and_modify(|existing| {
                            if b.created_at > existing.created_at {
                                *existing = b;
                            }
                        })
                        .or_insert(b);
                }
                let mut sorted_months: Vec<&BackupMetadata> = monthly_groups.into_values().collect();
                sorted_months.sort_by(|a, b| b.created_at.cmp(&a.created_at));
                for b in sorted_months.into_iter().take(monthly_count as usize) {
                    keep_ids.insert(b.id.clone());
                }
            }
        }

        // Si ni days ni GFS ne sont spécifiés, on conserve tout par sécurité
        if policy.days.is_none() && !has_gfs {
            return Vec::new();
        }

        backups
            .iter()
            .filter(|b| !keep_ids.contains(&b.id))
            .collect()
    }
}
