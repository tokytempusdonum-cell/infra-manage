//! Adaptateur d'exécution de commandes système et implémentation mock pour les tests.

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tracing::{debug, error};

use crate::core::abstract_traits::{CommandExecutor, ExecutionOutput};
use crate::core::exceptions::{DomainError, DomainResult};

/// Exécuteur système réel utilisant std::process::Command.
pub struct SystemCommandExecutor;

impl SystemCommandExecutor {
    /// Crée une nouvelle instance de l'exécuteur système réel.
    pub fn new() -> Self {
        Self
    }
}

impl Default for SystemCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandExecutor for SystemCommandExecutor {
    fn execute(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> DomainResult<ExecutionOutput> {
        debug!(
            "[LOG EXECUTOR] Exécution de la commande : {} {:?}",
            command, args
        );

        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }

        let output = cmd.output().map_err(|e| DomainError::Execution {
            command: command.to_string(),
            message: format!("Échec du lancement du processus : {}", e),
            exit_code: None,
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        let success = output.status.success();

        if !success {
            error!(
                "[LOG EXECUTOR] La commande '{}' a échoué avec le code {} : {}",
                command, exit_code, stderr
            );
        }

        Ok(ExecutionOutput {
            stdout,
            stderr,
            exit_code,
            success,
        })
    }

    fn execute_to_file(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        output_file: &Path,
    ) -> DomainResult<u64> {
        debug!(
            "[LOG EXECUTOR] Exécution avec redirection vers {:?} : {} {:?}",
            output_file, command, args
        );

        let file = File::create(output_file).map_err(|e| DomainError::Storage(format!(
            "Impossible de créer le fichier de sortie temporaire {:?} : {}",
            output_file, e
        )))?;

        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.stdout(Stdio::from(file));
        cmd.stderr(Stdio::piped());

        let mut child = cmd.spawn().map_err(|e| DomainError::Execution {
            command: command.to_string(),
            message: format!("Échec du lancement du processus : {}", e),
            exit_code: None,
        })?;

        let status = child.wait().map_err(|e| DomainError::Execution {
            command: command.to_string(),
            message: format!("Échec de l'attente du processus : {}", e),
            exit_code: None,
        })?;

        if !status.success() {
            let mut stderr_content = String::new();
            if let Some(mut err_stream) = child.stderr.take() {
                let _ = err_stream.read_to_string(&mut stderr_content);
            }
            return Err(DomainError::Execution {
                command: command.to_string(),
                message: format!(
                    "Le processus a échoué avec le code {:?} : {}",
                    status.code(),
                    stderr_content
                ),
                exit_code: status.code(),
            });
        }

        let metadata = std::fs::metadata(output_file)?;
        Ok(metadata.len())
    }

    fn execute_with_stdin(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        stdin_file: &Path,
    ) -> DomainResult<ExecutionOutput> {
        debug!(
            "[LOG EXECUTOR] Exécution avec entrée standard depuis {:?} : {} {:?}",
            stdin_file, command, args
        );

        let file = File::open(stdin_file).map_err(|e| DomainError::Storage(format!(
            "Impossible d'ouvrir le fichier d'entrée {:?} : {}",
            stdin_file, e
        )))?;

        let mut cmd = Command::new(command);
        cmd.args(args);
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::from(file));

        let output = cmd.output().map_err(|e| DomainError::Execution {
            command: command.to_string(),
            message: format!("Échec du lancement du processus : {}", e),
            exit_code: None,
        })?;

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let exit_code = output.status.code().unwrap_or(-1);
        let success = output.status.success();

        Ok(ExecutionOutput {
            stdout,
            stderr,
            exit_code,
            success,
        })
    }

    fn is_container_running(&self, container: &str) -> DomainResult<bool> {
        let output = self.execute(
            "docker",
            &["inspect", "-f", "{{.State.Running}}", container],
            &[],
        )?;

        if output.success {
            Ok(output.stdout.trim() == "true")
        } else {
            Ok(false)
        }
    }
}

/// Enregistrement d'une commande exécutée par le MockCommandExecutor.
#[derive(Debug, Clone)]
pub struct MockRecordedCall {
    pub command: String,
    pub args: Vec<String>,
    pub envs: Vec<(String, String)>,
}

/// Exécuteur simulé pour isoler les frontières externes dans les tests automatisés.
#[derive(Clone)]
pub struct MockCommandExecutor {
    calls: Arc<Mutex<Vec<MockRecordedCall>>>,
    container_states: Arc<Mutex<HashMap<String, bool>>>,
    mock_responses: Arc<Mutex<HashMap<String, ExecutionOutput>>>,
    file_generator_content: Arc<Mutex<Option<Vec<u8>>>>,
}

impl MockCommandExecutor {
    /// Initialise un nouveau mock sans appels enregistrés.
    pub fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            container_states: Arc::new(Mutex::new(HashMap::new())),
            mock_responses: Arc::new(Mutex::new(HashMap::new())),
            file_generator_content: Arc::new(Mutex::new(None)),
        }
    }

    /// Définit l'état simulé d'un conteneur Docker.
    pub fn set_container_running(&self, container: &str, running: bool) {
        let mut map = self.container_states.lock().unwrap();
        map.insert(container.to_string(), running);
    }

    /// Définit la réponse simulée pour une commande donnée.
    pub fn set_command_response(&self, command: &str, output: ExecutionOutput) {
        let mut map = self.mock_responses.lock().unwrap();
        map.insert(command.to_string(), output);
    }

    /// Définit le contenu binaire écrit lors d'un appel à `execute_to_file`.
    pub fn set_file_generator_content(&self, bytes: Vec<u8>) {
        let mut lock = self.file_generator_content.lock().unwrap();
        *lock = Some(bytes);
    }

    /// Retourne la liste des appels enregistrés.
    pub fn get_calls(&self) -> Vec<MockRecordedCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl Default for MockCommandExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandExecutor for MockCommandExecutor {
    fn execute(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> DomainResult<ExecutionOutput> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(MockRecordedCall {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            envs: envs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });

        let responses = self.mock_responses.lock().unwrap();
        if let Some(resp) = responses.get(command) {
            Ok(resp.clone())
        } else {
            Ok(ExecutionOutput {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: 0,
                success: true,
            })
        }
    }

    fn execute_to_file(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        output_file: &Path,
    ) -> DomainResult<u64> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(MockRecordedCall {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            envs: envs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });

        let mut file = File::create(output_file).map_err(|e| DomainError::Storage(format!(
            "Impossible d'écrire dans {:?} : {}",
            output_file, e
        )))?;

        let default_content = b"PGDMP_MOCK_DUMP_DATA_VALID";
        let content_lock = self.file_generator_content.lock().unwrap();
        let bytes_to_write = content_lock.as_deref().unwrap_or(default_content);
        file.write_all(bytes_to_write)?;

        Ok(bytes_to_write.len() as u64)
    }

    fn execute_with_stdin(
        &self,
        command: &str,
        args: &[&str],
        envs: &[(&str, &str)],
        _stdin_file: &Path,
    ) -> DomainResult<ExecutionOutput> {
        let mut calls = self.calls.lock().unwrap();
        calls.push(MockRecordedCall {
            command: command.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            envs: envs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        });

        Ok(ExecutionOutput {
            stdout: "Restored successfully".to_string(),
            stderr: String::new(),
            exit_code: 0,
            success: true,
        })
    }

    fn is_container_running(&self, container: &str) -> DomainResult<bool> {
        let states = self.container_states.lock().unwrap();
        // Par défaut dans le mock, un conteneur est considéré actif sauf indication contraire
        Ok(*states.get(container).unwrap_or(&true))
    }
}
