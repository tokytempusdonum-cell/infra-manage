# Restauration d'un serveur tombé (depuis S3)

Si `s001` tombe, on recrée ses applications sur une autre machine, à partir des sauvegardes
rangées par serveur sur S3 (`backups/s001/…`, `backups/s002/…`).

## Prérequis sur la machine de remplacement

- `backup-cli`, `rclone`, `restic`, `docker` installés.
- Le fichier `backup.yml` **du serveur d'origine** (il contient les profils, conteneurs et mots de
  passe à utiliser). Il est dans les sauvegardes de configuration (`/root/backup-script`).
- La config rclone (`settings.storage.rclone_config`) et le fichier mot de passe restic
  (`restic_password_file`) du serveur d'origine.
- Les variables d'environnement des mots de passe de bases (`password_env`) positionnées.
- Dans `backup.yml`, le profil restic `configs` doit avoir `destination.path` (le dépôt restic envoyé
  sur S3), sinon la configuration n'est pas sur S3.

## Commandes

```bash
# Que contient S3 pour ce serveur ? (dernière sauvegarde de chaque profil)
backup-cli -c backup.yml restore list --server s001

# Simulation : rien n'est téléchargé ni modifié
backup-cli -c backup.yml restore --server s001 --application authentik --dry-run

# Une application, plusieurs, ou toutes
backup-cli -c backup.yml restore --server s001 --application authentik --start
backup-cli -c backup.yml restore --server s001 --application authentik,ghost --start
backup-cli -c backup.yml restore --server s001 --application all --start

# État à une date donnée (UTC) : le plus récent antérieur à cette date
backup-cli -c backup.yml restore --server s001 --application ghost --before 2026-11-12
```

| Option | Rôle |
|---|---|
| `--server` | serveur d'origine sur S3 (s001, s002…) |
| `--application` | nom, liste `a,b` ou `all` (alias `--app`) |
| `--start` | lance `docker compose up -d` avant de restaurer les bases |
| `--before` | `YYYY-MM-DD` (fin de journée), `YYYY-MM-DD_HH-MM-SS` ou RFC 3339, en UTC |
| `--dry-run` | simulation |
| `--force` | autorise l'écrasement d'une application déjà présente sur cette machine |
| `--include-system` | avec `all` : restaure aussi `/etc`, crontab… (exige `--force`) |
| `--keep-downloads` / `--work-dir` | conserve / choisit le dossier de téléchargement |

## Ce qui est restauré, dans l'ordre

Pour chaque application :

1. **Configuration** (`docker-compose.yml`, `.env`, dossiers de config) depuis le dépôt restic, vers
   `<applications_root>/<application>` (propriétaires et permissions conservés).
2. **Fichiers uploadés** : les profils `filesystem` rattachés à l'application (`application: ghost`),
   remis à leur emplacement d'origine.
3. **Démarrage** (`--start`) : `docker compose --project-directory /opt/<app> up -d`.
4. **Bases de données** (postgres, mysql, mariadb, mongo) : dernier dump, vers le conteneur de la
   machine courante.

Chaque téléchargement est vérifié (`rclone check`) avant usage. Le bilan indique, étape par étape,
ce qui a réussi ou échoué ; le code de sortie est non nul s'il y a un échec.

## Sauvegarder les fichiers uploadés

Les dossiers de volumes sont exclus du profil `configs`. Leurs fichiers utiles (uploads, médias) se
sauvegardent avec un profil `filesystem` rattaché à l'application :

```yaml
- name: ghost-uploads
  type: filesystem
  application: ghost            # regroupe avec la base « ghost »
  schedule: "30 0 * * *"
  source:
    paths: [/opt/ghost/volumes/content]
  compression: { enabled: true, algorithm: zstd }
  retention: { days: 14 }
```

Ne pas sauvegarder ainsi les fichiers bruts d'une base de données : elle a son propre profil (dump).

## Sécurités

- Une application déjà présente sur la machine (`/opt/<app>` non vide, ou chemins uploadés non vides)
  n'est pas écrasée sans `--force`.
- Si le conteneur d'une base n'est pas démarré, l'étape échoue avec la commande à lancer (ou `--start`).
- La restauration d'une base **remplace** ses données (`--clean` / `--drop`).
- Les archives téléchargées sont refusées si elles contiennent des chemins `..`.
- Un nom de serveur erroné est détecté avant tout téléchargement.

## Limites

- Les fichiers de volumes non sauvegardés par un profil `filesystem` ne reviennent pas.
- Les mots de passe ne sont pas dans les sauvegardes : ils doivent être fournis (variables d'environnement).
- Si l'application tourne déjà et utilise la base, arrêter ses services avant de restaurer la base.
