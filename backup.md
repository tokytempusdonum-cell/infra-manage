Voici comment sauvegarder, en pratique. Les commandes ci-dessous ont été essayées sur une configuration de test, avec les vrais `restic` et `rclone`, sur un S3 simulé par un dossier local.

**Installer la dernière release**

```bash
curl -fL -O https://github.com/tokytempusdonum-cell/backup-cli/releases/latest/download/infra-manage-x86_64-unknown-linux-gnu.tar.gz
curl -fL -O https://github.com/tokytempusdonum-cell/backup-cli/releases/latest/download/infra-manage-x86_64-unknown-linux-gnu.tar.gz.sha256
sha256sum -c infra-manage-x86_64-unknown-linux-gnu.tar.gz.sha256
tar -xzf infra-manage-x86_64-unknown-linux-gnu.tar.gz
sudo install -m 0755 infra-manage-v*-x86_64-unknown-linux-gnu/infra-manage /usr/local/bin/infra-manage
```

**1. Avant la première sauvegarde**

- Installe `infra-manage`, `rclone`, `restic` et `docker` sur le serveur.
- Prépare ton fichier de configuration, par exemple `/root/backup-script/backup.yml`.
- Vérifie que la config rclone existe (`rclone_config`) et que le fichier du mot de passe restic existe (`restic_password_file`).
- Mets les variables des mots de passe de bases (`GHOST_DB_PASSWORD`, etc.) et du webhook dans un fichier `.env` placé **dans le même dossier que `backup.yml`**. Il est chargé automatiquement. Les variables déjà définies dans le shell restent prioritaires.
- Dans le profil `configs`, mets bien `destination.path: /backup-configuration`. Sans cette ligne, le dépôt restic n'est jamais envoyé sur S3.

**2. Définir quoi sauvegarder (dans backup.yml)**

- **Une base de données** : un profil par base. Les types possibles sont `postgres`, `mysql`, `mariadb` et `mongo`. Indique `container_name`, `username`, `database` et `password_env`.
- **Les configurations** (`.env`, `docker-compose`, `/etc`...) : le profil `configs` de type `restic`, avec la liste des `paths`. Pour exclure les volumes, ajoute par exemple `exclude: ["/opt/*/volumes"]`.
- **Les fichiers uploadés d'une application** : un profil de type `filesystem` avec `application: ghost` et `paths: [/opt/ghost/volumes/content]`. Le champ `application` le rattache à la base du même nom, pour restaurer les deux ensemble.

**3. Lancer à la main**

```
infra-manage -c /root/backup-script/backup.yml config validate -f /root/backup-script/backup.yml
infra-manage -c /root/backup-script/backup.yml backup run --dry-run
infra-manage -c /root/backup-script/backup.yml backup run
infra-manage -c /root/backup-script/backup.yml backup run --profile ghost
```

- `config validate` contrôle le fichier.
- `--dry-run` simule sans rien écrire.
- Sans `--profile`, tous les profils actifs tournent l'un après l'autre.

**4. Vérifier**

```
infra-manage -c /root/backup-script/backup.yml backup list
infra-manage -c /root/backup-script/backup.yml backup list --profile ghost
infra-manage -c /root/backup-script/backup.yml backup info <id>
infra-manage -c /root/backup-script/backup.yml backup verify <id>
rclone lsf s3backup:backups/s001/backup
rclone lsf s3backup:backups/s001/backup-configuration
```

- `backup verify` contrôle l'intégrité et la somme SHA-256 d'une sauvegarde.
- Les deux `rclone lsf` montrent ce qui est réellement arrivé sur S3. Tu dois y voir un fichier par profil, plus le dépôt restic dans le dossier `backup-configuration`.

**Tester les notifications**

```
# SLACK_WEBHOOK_URL peut être défini dans /root/backup-script/.env
infra-manage -c /root/backup-script/backup.yml notify test
infra-manage -c /root/backup-script/backup.yml notify test --message "Message personnalisé"
```

La commande échoue si `settings.notifications.enabled` vaut `false` ou si aucun webhook n'est configuré.

**5. Automatiser**

```
infra-manage -c /root/backup-script/backup.yml scheduler crontab --binary-path /usr/local/bin/infra-manage
```

Cette commande affiche seulement les lignes de cron, elle n'écrit rien. Pour les installer :

```
crontab -l > /root/crontab.bak
(crontab -l 2>/dev/null; infra-manage -c /root/backup-script/backup.yml scheduler crontab --binary-path /usr/local/bin/infra-manage) | crontab -
crontab -l
```

- Utilise un chemin absolu pour le `.yml`.
- Si tu relances l'installation, tu auras des lignes en double : retire d'abord l'ancien bloc avec `crontab -e`.
- Chaque profil a son propre verrou, donc deux profils peuvent tourner en même temps, mais pas deux fois le même profil. Tu peux garder des horaires proches.
- Mets `configs` (restic) à une heure à part : l'envoi vers S3 peut durer longtemps.
- Chaque ligne cron lance un seul profil, donc tu reçois un message Slack par profil. Pour un seul bilan par nuit, planifie une seule ligne `backup run` sans `--profile`.

**6. Ce que tu reçois dans Slack**

Chaque cycle envoie un message avec le total, le nombre de réussites, le nombre d'échecs et la durée. Les échecs sont listés en premier avec leur cause, suivis des profils réussis avec leur taille. L'URL du webhook se règle avec `slack_webhook_env: SLACK_WEBHOOK_URL`. Une simulation (`--dry-run`) n'envoie rien.

**7. Ce qui est sauvegardé et ce qui ne l'est pas**

- Les bases de données sont sauvegardées par dump, compressé en gzip ou zstd.
- Les configurations sont sauvegardées par restic, avec propriétaires et permissions.
- Les uploads sont sauvegardés si tu as créé le profil `filesystem` correspondant.
- Les mots de passe ne sont dans aucune sauvegarde. Garde-les ailleurs, dans ton gestionnaire de secrets.
- Les fichiers de volumes qu'aucun profil ne couvre ne sont pas sauvegardés.

Pour restaurer, c'est la commande `restore --server s001 --application <nom|all>`, décrite dans `restore.md`.
