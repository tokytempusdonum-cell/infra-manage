//! Adaptateur pour la sauvegarde et la restauration des bases de données PostgreSQL et MySQL.

use std::fs;
use std::path::Path;
use tracing::info;

use crate::core::abstract_traits::CommandExecutor;
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::BackupProfile;

/// Fournisseur d'opérations sur bases de données relationnelles.
pub struct DatabaseProvider<'a> {
    executor: &'a dyn CommandExecutor,
}

impl<'a> DatabaseProvider<'a> {
    /// Crée un nouveau fournisseur de base de données avec injection de dépendance.
    pub fn new(executor: &'a dyn CommandExecutor) -> Self {
        Self { executor }
    }

    /// Résout le mot de passe de connexion de manière sécurisée (environnement ou fichier).
    ///
    /// Aucun mot de passe n'est jamais hardcodé dans les configurations.
    pub fn resolve_password(&self, profile: &BackupProfile) -> DomainResult<Option<String>> {
        let src = &profile.source;

        if let Some(env_var) = &src.password_env {
            if let Ok(val) = std::env::var(env_var) {
                return Ok(Some(val));
            } else {
                return Err(DomainError::Config(format!(
                    "Profil '{}' : la variable d'environnement '{}' est introuvable",
                    profile.name, env_var
                )));
            }
        }

        if let Some(path) = &src.password_file {
            let content = fs::read_to_string(path).map_err(|e| {
                DomainError::Config(format!(
                    "Profil '{}' : impossible de lire le fichier de mot de passe {:?} : {}",
                    profile.name, path, e
                ))
            })?;
            return Ok(Some(content.trim().to_string()));
        }

        Ok(None)
    }

    /// Exécute le dump d'une base de données PostgreSQL vers un fichier cible.
    pub fn dump_postgres(&self, profile: &BackupProfile, output_file: &Path) -> DomainResult<u64> {
        let src = &profile.source;
        let db_user = src.username.as_deref().unwrap_or("postgres");
        let db_name = src.database.as_deref().unwrap_or("postgres");

        let password = self.resolve_password(profile)?;
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push(("PGPASSWORD", pwd.as_str()));
        }

        if let Some(container) = &src.container_name {
            info!(
                "[LOG DATABASE] Exécution du dump PostgreSQL pour le conteneur '{}' (base: {})",
                container, db_name
            );

            let mut args = vec![
                "exec",
                "-e",
                "PGPASSWORD",
                container.as_str(),
                "pg_dump",
                "-U",
                db_user,
                "-Fc",
                "-d",
                db_name,
            ];

            if let Some(extra) = &src.dump_options {
                for opt in extra {
                    args.push(opt.as_str());
                }
            }

            self.executor
                .execute_to_file("docker", &args, &envs, output_file)
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port_str = src.port.unwrap_or(5432).to_string();

            info!(
                "[LOG DATABASE] Exécution du dump PostgreSQL direct sur {}:{} (base: {})",
                host, port_str, db_name
            );

            let mut args = vec![
                "-h",
                host,
                "-p",
                &port_str,
                "-U",
                db_user,
                "-Fc",
                "-d",
                db_name,
            ];

            if let Some(extra) = &src.dump_options {
                for opt in extra {
                    args.push(opt.as_str());
                }
            }

            self.executor
                .execute_to_file("pg_dump", &args, &envs, output_file)
        }
    }

    /// Exécute le dump d'une base de données MySQL vers un fichier cible.
    pub fn dump_mysql(&self, profile: &BackupProfile, output_file: &Path) -> DomainResult<u64> {
        let src = &profile.source;
        let db_user = src.username.as_deref().unwrap_or("root");
        let db_name = src.database.as_deref().unwrap_or("mysql");

        let password = self.resolve_password(profile)?;
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push(("MYSQL_PWD", pwd.as_str()));
        }

        if let Some(container) = &src.container_name {
            info!(
                "[LOG DATABASE] Exécution du dump MySQL pour le conteneur '{}' (base: {})",
                container, db_name
            );

            let mut args = vec![
                "exec",
                "-e",
                "MYSQL_PWD",
                container.as_str(),
                "mysqldump",
                "-u",
                db_user,
                "--no-tablespaces",
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                db_name,
            ];

            if let Some(extra) = &src.dump_options {
                for opt in extra {
                    args.push(opt.as_str());
                }
            }

            self.executor
                .execute_to_file("docker", &args, &envs, output_file)
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port_str = src.port.unwrap_or(3306).to_string();

            info!(
                "[LOG DATABASE] Exécution du dump MySQL direct sur {}:{} (base: {})",
                host, port_str, db_name
            );

            let mut args = vec![
                "-h",
                host,
                "-P",
                &port_str,
                "-u",
                db_user,
                "--no-tablespaces",
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                db_name,
            ];

            if let Some(extra) = &src.dump_options {
                for opt in extra {
                    args.push(opt.as_str());
                }
            }

            self.executor
                .execute_to_file("mysqldump", &args, &envs, output_file)
        }
    }

    /// Restaure une base de données PostgreSQL depuis un fichier dump.
    pub fn restore_postgres(
        &self,
        profile: &BackupProfile,
        dump_file: &Path,
        dest_db_override: Option<&str>,
    ) -> DomainResult<()> {
        let src = &profile.source;
        let db_user = src.username.as_deref().unwrap_or("postgres");
        let db_name = dest_db_override
            .or(src.database.as_deref())
            .unwrap_or("postgres");

        let password = self.resolve_password(profile)?;
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push(("PGPASSWORD", pwd.as_str()));
        }

        info!(
            "[LOG DATABASE] Restauration PostgreSQL de {:?} vers la base '{}'",
            dump_file, db_name
        );

        if let Some(container) = &src.container_name {
            let args = [
                "exec",
                "-i",
                "-e",
                "PGPASSWORD",
                container.as_str(),
                "pg_restore",
                "-U",
                db_user,
                "-d",
                db_name,
                "--clean",
                "--if-exists",
            ];

            let output = self
                .executor
                .execute_with_stdin("docker", &args, &envs, dump_file)?;

            if !output.success {
                return Err(DomainError::Execution {
                    command: "pg_restore".to_string(),
                    message: format!("Échec de pg_restore : {}", output.stderr),
                    exit_code: Some(output.exit_code),
                });
            }
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port_str = src.port.unwrap_or(5432).to_string();

            let args = [
                "-h",
                host,
                "-p",
                &port_str,
                "-U",
                db_user,
                "-d",
                db_name,
                "--clean",
                "--if-exists",
            ];

            let output = self
                .executor
                .execute_with_stdin("pg_restore", &args, &envs, dump_file)?;

            if !output.success {
                return Err(DomainError::Execution {
                    command: "pg_restore".to_string(),
                    message: format!("Échec de pg_restore : {}", output.stderr),
                    exit_code: Some(output.exit_code),
                });
            }
        }

        info!("[LOG DATABASE] Restauration PostgreSQL réussie.");
        Ok(())
    }

    /// Restaure une base de données MySQL depuis un fichier dump SQL.
    pub fn restore_mysql(
        &self,
        profile: &BackupProfile,
        sql_file: &Path,
        dest_db_override: Option<&str>,
    ) -> DomainResult<()> {
        let src = &profile.source;
        let db_user = src.username.as_deref().unwrap_or("root");
        let db_name = dest_db_override
            .or(src.database.as_deref())
            .unwrap_or("mysql");

        let password = self.resolve_password(profile)?;
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push(("MYSQL_PWD", pwd.as_str()));
        }

        info!(
            "[LOG DATABASE] Restauration MySQL de {:?} vers la base '{}'",
            sql_file, db_name
        );

        if let Some(container) = &src.container_name {
            let args = [
                "exec",
                "-i",
                "-e",
                "MYSQL_PWD",
                container.as_str(),
                "mysql",
                "-u",
                db_user,
                db_name,
            ];

            let output = self
                .executor
                .execute_with_stdin("docker", &args, &envs, sql_file)?;

            if !output.success {
                return Err(DomainError::Execution {
                    command: "mysql".to_string(),
                    message: format!("Échec de la restauration MySQL : {}", output.stderr),
                    exit_code: Some(output.exit_code),
                });
            }
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port_str = src.port.unwrap_or(3306).to_string();

            let args = ["-h", host, "-P", &port_str, "-u", db_user, db_name];

            let output = self
                .executor
                .execute_with_stdin("mysql", &args, &envs, sql_file)?;

            if !output.success {
                return Err(DomainError::Execution {
                    command: "mysql".to_string(),
                    message: format!("Échec de la restauration MySQL : {}", output.stderr),
                    exit_code: Some(output.exit_code),
                });
            }
        }

        info!("[LOG DATABASE] Restauration MySQL réussie.");
        Ok(())
    }
}
