//! Adaptateur pour la sauvegarde et la restauration des bases de données
//! PostgreSQL, MySQL, MariaDB et MongoDB.

use std::fs;
use std::path::Path;
use tracing::info;

use crate::core::abstract_traits::CommandExecutor;
use crate::core::exceptions::{DomainError, DomainResult};
use crate::core::model::BackupProfile;
use super::stream_compression::RestoreInput;

/// Variante de la famille MySQL (binaires de dump et de client).
struct SqlFlavor {
    label: &'static str,
    dump_bin: &'static str,
    client_bin: &'static str,
}

const MYSQL: SqlFlavor = SqlFlavor {
    label: "MySQL",
    dump_bin: "mysqldump",
    client_bin: "mysql",
};

const MARIADB: SqlFlavor = SqlFlavor {
    label: "MariaDB",
    dump_bin: "mariadb-dump",
    client_bin: "mariadb",
};

/// Variable d'environnement transportant le mot de passe MongoDB (jamais en argument de commande).
const MONGO_PWD_ENV: &str = "MONGO_PWD";

/// Fournisseur d'opérations sur bases de données (PostgreSQL, MySQL, MariaDB, MongoDB).
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
        self.dump_sql(&MYSQL, profile, output_file)
    }

    /// Exécute le dump d'une base de données MariaDB (`mariadb-dump`) vers un fichier cible.
    pub fn dump_mariadb(&self, profile: &BackupProfile, output_file: &Path) -> DomainResult<u64> {
        self.dump_sql(&MARIADB, profile, output_file)
    }

    fn dump_sql(
        &self,
        flavor: &SqlFlavor,
        profile: &BackupProfile,
        output_file: &Path,
    ) -> DomainResult<u64> {
        let src = &profile.source;
        let db_user = src.username.as_deref().unwrap_or("root");
        let db_name = src.database.as_deref().unwrap_or("mysql");

        let password = self.resolve_password(profile)?;
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push(("MYSQL_PWD", pwd.as_str()));
        }

        let (command, mut args): (String, Vec<String>) = if let Some(container) = &src.container_name {
            info!(
                "[LOG DATABASE] Exécution du dump {} pour le conteneur '{}' (base: {})",
                flavor.label, container, db_name
            );
            (
                "docker".to_string(),
                vec![
                    "exec".to_string(),
                    "-e".to_string(),
                    "MYSQL_PWD".to_string(),
                    container.clone(),
                    flavor.dump_bin.to_string(),
                ],
            )
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port = src.port.unwrap_or(3306);
            info!(
                "[LOG DATABASE] Exécution du dump {} direct sur {}:{} (base: {})",
                flavor.label, host, port, db_name
            );
            (
                flavor.dump_bin.to_string(),
                vec![
                    "-h".to_string(),
                    host.to_string(),
                    "-P".to_string(),
                    port.to_string(),
                ],
            )
        };

        args.extend(
            [
                "-u",
                db_user,
                "--no-tablespaces",
                "--single-transaction",
                "--routines",
                "--triggers",
                "--events",
                db_name,
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        if let Some(extra) = &src.dump_options {
            args.extend(extra.iter().cloned());
        }

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.executor
            .execute_to_file(&command, &args_ref, &envs, output_file)
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
        self.restore_sql(&MYSQL, profile, sql_file, dest_db_override)
    }

    /// Restaure une base de données MariaDB depuis un fichier dump SQL.
    pub fn restore_mariadb(
        &self,
        profile: &BackupProfile,
        sql_file: &Path,
        dest_db_override: Option<&str>,
    ) -> DomainResult<()> {
        self.restore_sql(&MARIADB, profile, sql_file, dest_db_override)
    }

    fn restore_sql(
        &self,
        flavor: &SqlFlavor,
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
            "[LOG DATABASE] Restauration {} de {:?} vers la base '{}'",
            flavor.label, sql_file, db_name
        );

        let (command, mut args): (String, Vec<String>) = if let Some(container) = &src.container_name {
            (
                "docker".to_string(),
                vec![
                    "exec".to_string(),
                    "-i".to_string(),
                    "-e".to_string(),
                    "MYSQL_PWD".to_string(),
                    container.clone(),
                    flavor.client_bin.to_string(),
                ],
            )
        } else {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port = src.port.unwrap_or(3306);
            (
                flavor.client_bin.to_string(),
                vec![
                    "-h".to_string(),
                    host.to_string(),
                    "-P".to_string(),
                    port.to_string(),
                ],
            )
        };
        args.extend(["-u", db_user, db_name].iter().map(|s| s.to_string()));

        // Décompression automatique (gzip / zstd détectés par magic bytes) ; SQL en clair accepté.
        let input = RestoreInput::prepare(sql_file)?;

        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = self
            .executor
            .execute_with_stdin(&command, &args_ref, &envs, input.path())?;

        if !output.success {
            return Err(DomainError::Execution {
                command: flavor.client_bin.to_string(),
                message: format!(
                    "Échec de la restauration {} : {}",
                    flavor.label, output.stderr
                ),
                exit_code: Some(output.exit_code),
            });
        }

        info!("[LOG DATABASE] Restauration {} réussie.", flavor.label);
        Ok(())
    }

    // ------------------------------------------------------------------
    // MongoDB
    // ------------------------------------------------------------------

    /// Arguments de connexion et d'authentification communs à mongodump / mongorestore.
    fn mongo_connection_args(&self, profile: &BackupProfile) -> Vec<String> {
        let src = &profile.source;
        let mut args = Vec::new();

        // En mode conteneur, l'outil se connecte au mongod local du conteneur.
        if src.container_name.is_none() {
            let host = src.host.as_deref().unwrap_or("localhost");
            let port = src.port.unwrap_or(27017);
            args.push("--host".to_string());
            args.push(format!("{}:{}", host, port));
        }

        if let Some(user) = &src.username {
            args.push("--username".to_string());
            args.push(user.clone());
            args.push("--authenticationDatabase".to_string());
            args.push(
                src.auth_database
                    .clone()
                    .unwrap_or_else(|| "admin".to_string()),
            );
        }
        args
    }

    /// Construit la commande complète (docker exec ou exécution directe) pour un outil MongoDB.
    ///
    /// Le mot de passe n'apparaît jamais dans les arguments de `docker exec` ni de l'outil
    /// lancé depuis l'hôte : il transite par la variable d'environnement `MONGO_PWD`
    /// et n'est développé que par un `sh -c 'exec <outil> "$@" --password "$MONGO_PWD"'`.
    fn mongo_invocation(
        &self,
        profile: &BackupProfile,
        tool: &str,
        tool_args: Vec<String>,
        interactive: bool,
        has_password: bool,
    ) -> (String, Vec<String>) {
        let script = format!("exec {} \"$@\" --password \"$MONGO_PWD\"", tool);

        match (&profile.source.container_name, has_password) {
            (Some(container), true) => {
                let mut args = vec!["exec".to_string()];
                if interactive {
                    args.push("-i".to_string());
                }
                args.extend([
                    "-e".to_string(),
                    MONGO_PWD_ENV.to_string(),
                    container.clone(),
                    "sh".to_string(),
                    "-c".to_string(),
                    script,
                    "sh".to_string(),
                ]);
                args.extend(tool_args);
                ("docker".to_string(), args)
            }
            (Some(container), false) => {
                let mut args = vec!["exec".to_string()];
                if interactive {
                    args.push("-i".to_string());
                }
                args.push(container.clone());
                args.push(tool.to_string());
                args.extend(tool_args);
                ("docker".to_string(), args)
            }
            (None, true) => {
                let mut args = vec!["-c".to_string(), script, "sh".to_string()];
                args.extend(tool_args);
                ("sh".to_string(), args)
            }
            (None, false) => (tool.to_string(), tool_args),
        }
    }

    /// Exécute `mongodump` (archive sur stdout, gzip natif) vers un fichier cible.
    ///
    /// Sans `database`, toutes les bases de l'instance sont sauvegardées.
    pub fn dump_mongo(&self, profile: &BackupProfile, output_file: &Path) -> DomainResult<u64> {
        let src = &profile.source;
        let password = self.resolve_password(profile)?;

        // --quiet : évite de saturer le tube stderr (lu seulement après la fin du processus).
        let mut tool_args = vec!["--archive".to_string(), "--quiet".to_string()];
        if profile.mongo_uses_gzip() {
            tool_args.push("--gzip".to_string());
        }
        if let Some(db) = &src.database {
            tool_args.push("--db".to_string());
            tool_args.push(db.clone());
        }
        tool_args.extend(self.mongo_connection_args(profile));
        if let Some(extra) = &src.dump_options {
            tool_args.extend(extra.iter().cloned());
        }

        info!(
            "[LOG DATABASE] Exécution du dump MongoDB (cible: {}, base: {})",
            src.container_name
                .as_deref()
                .or(src.host.as_deref())
                .unwrap_or("localhost"),
            src.database.as_deref().unwrap_or("toutes")
        );

        let (command, args) =
            self.mongo_invocation(profile, "mongodump", tool_args, false, password.is_some());
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push((MONGO_PWD_ENV, pwd.as_str()));
        }
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        self.executor
            .execute_to_file(&command, &args_ref, &envs, output_file)
    }

    /// Restaure une archive MongoDB via `mongorestore` (collections existantes supprimées : `--drop`).
    ///
    /// `dest_db_override` restaure vers une base de destination différente (nécessite `database`
    /// dans le profil, qui désigne la base d'origine).
    pub fn restore_mongo(
        &self,
        profile: &BackupProfile,
        archive_file: &Path,
        dest_db_override: Option<&str>,
    ) -> DomainResult<()> {
        let src = &profile.source;
        let password = self.resolve_password(profile)?;

        let mut tool_args = vec!["--archive".to_string(), "--drop".to_string()];
        if archive_file
            .extension()
            .map(|e| e.eq_ignore_ascii_case("gz"))
            .unwrap_or(false)
        {
            tool_args.push("--gzip".to_string());
        }

        let target_db = match (src.database.as_deref(), dest_db_override) {
            (Some(origin), Some(dest)) if origin != dest => {
                tool_args.push("--nsFrom".to_string());
                tool_args.push(format!("{}.*", origin));
                tool_args.push("--nsTo".to_string());
                tool_args.push(format!("{}.*", dest));
                dest.to_string()
            }
            (None, Some(_)) => {
                return Err(DomainError::Config(format!(
                    "Profil MongoDB '{}' : 'dest_db' exige 'database' (base d'origine) dans le profil",
                    profile.name
                )));
            }
            (Some(origin), _) => origin.to_string(),
            (None, None) => "toutes les bases".to_string(),
        };
        tool_args.extend(self.mongo_connection_args(profile));

        info!(
            "[LOG DATABASE] Restauration MongoDB de {:?} vers '{}'",
            archive_file, target_db
        );

        let (command, args) =
            self.mongo_invocation(profile, "mongorestore", tool_args, true, password.is_some());
        let mut envs = Vec::new();
        if let Some(ref pwd) = password {
            envs.push((MONGO_PWD_ENV, pwd.as_str()));
        }
        let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        let output = self
            .executor
            .execute_with_stdin(&command, &args_ref, &envs, archive_file)?;

        if !output.success {
            return Err(DomainError::Execution {
                command: "mongorestore".to_string(),
                message: format!("Échec de mongorestore : {}", output.stderr),
                exit_code: Some(output.exit_code),
            });
        }

        info!("[LOG DATABASE] Restauration MongoDB réussie.");
        Ok(())
    }
}
