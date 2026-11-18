//! Tests des notifications : bilan détaillé et envoi HTTP via ureq (sans curl).

mod common;
use common::TestEnvironment;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use chrono::Utc;

use backup_cli::actions::backup_action::BackupAction;
use backup_cli::core::abstract_traits::NotificationService;
use backup_cli::core::model::{
    format_duration, format_size, BackupReport, BackupStatus, NotificationConfig, ProfileReport,
    ProfileType,
};
use backup_cli::providers::notification_provider::{build_slack_payload, WebhookNotificationService};

fn pr(name: &str, kind: ProfileType, ok: bool, size: Option<u64>, detail: Option<&str>) -> ProfileReport {
    ProfileReport {
        name: name.to_string(),
        profile_type: kind,
        status: if ok { BackupStatus::Success } else { BackupStatus::Failed },
        size_bytes: size,
        duration_seconds: 4.0,
        detail: detail.map(|d| d.to_string()),
    }
}

fn report(profiles: Vec<ProfileReport>) -> BackupReport {
    BackupReport { hostname: "srv-s001".to_string(), started_at: Utc::now(), duration_seconds: 72.0, profiles }
}

#[test]
fn test_headlines_and_counts() {
    let all_ok = report(vec![
        pr("a", ProfileType::Postgres, true, Some(1024), None),
        pr("b", ProfileType::Mysql, true, Some(2048), None),
    ]);
    assert_eq!(all_ok.headline(), "✅ Sauvegardes réussies — 2/2 profils sauvegardés");

    let partial = report(vec![
        pr("a", ProfileType::Postgres, true, Some(1), None),
        pr("b", ProfileType::Mysql, false, None, Some("boom")),
        pr("c", ProfileType::Mongo, true, Some(1), None),
    ]);
    assert_eq!(partial.headline(), "⚠️ Sauvegardes partielles — 2 réussies, 1 échec sur 3 profils");
    assert_eq!((partial.total(), partial.succeeded(), partial.failed()), (3, 2, 1));

    let all_ko = report(vec![pr("a", ProfileType::Postgres, false, None, Some("x")), pr("b", ProfileType::Mysql, false, None, Some("y"))]);
    assert!(all_ko.headline().starts_with("🚨 Échec des sauvegardes"));
    assert!(all_ko.headline().contains("2 sur 2"));
}

#[test]
fn test_plain_text_lists_failures_first_with_details() {
    let r = report(vec![
        pr("authentik", ProfileType::Postgres, true, Some(12_900_000), None),
        pr("ghost", ProfileType::Mysql, false, None, Some("Le processus a échoué\n  pg: connexion refusée")),
        pr("configs", ProfileType::Restic, true, None, Some("snapshot restic créé")),
    ]);
    let text = r.to_plain_text();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].contains("srv-s001"));
    assert_eq!(lines[1], "Total : 3 · Réussies : 2 · Échecs : 1 · Durée : 1 min 12 s");
    assert!(lines[2].starts_with("❌ ghost (mysql) — échec : Le processus a échoué pg: connexion refusée"));
    assert!(text.contains("✅ authentik (postgres) — 12,3 Mo en 4 s"));
    assert!(text.contains("✅ configs (restic) — snapshot restic créé en 4 s"));
}

#[test]
fn test_format_helpers() {
    assert_eq!(format_size(512), "512 o");
    assert_eq!(format_size(1536), "1,5 Ko");
    assert_eq!(format_duration(4.2), "4 s");
    assert_eq!(format_duration(3720.0), "1 h 02 min");
}

#[test]
fn test_slack_payload_structure_color_and_escaping() {
    let r = report(vec![
        pr("ok", ProfileType::Postgres, true, Some(1_048_576), None),
        pr("ko<&>", ProfileType::Mysql, false, None, Some("erreur `grave` <x>")),
    ]);
    let v = build_slack_payload(&r);
    assert_eq!(v["text"], r.headline());
    assert_eq!(v["attachments"][0]["color"], "#ecb22e");
    let blocks = &v["attachments"][0]["blocks"];
    let fields = blocks[0]["fields"].as_array().unwrap();
    assert!(fields[0]["text"].as_str().unwrap().contains("*Total*\n2"));
    assert!(fields[1]["text"].as_str().unwrap().contains("*Réussies*\n✅ 1"));
    assert!(fields[2]["text"].as_str().unwrap().contains("*Échecs*\n❌ 1"));
    let detail = blocks[1]["text"]["text"].as_str().unwrap();
    assert!(detail.starts_with("❌ *ko&lt;&amp;&gt;* (mysql)"), "échec attendu en premier : {}", detail);
    assert!(detail.contains("erreur 'grave' &lt;x&gt;"));
    assert!(detail.contains("✅ *ok* (postgres) — 1,0 Mo en 4 s"));

    let green = build_slack_payload(&report(vec![pr("a", ProfileType::Postgres, true, Some(1), None)]));
    assert_eq!(green["attachments"][0]["color"], "#2eb67d");
    let red = build_slack_payload(&report(vec![pr("a", ProfileType::Postgres, false, None, Some("x"))]));
    assert_eq!(red["attachments"][0]["color"], "#e01e5a");
}

#[test]
fn test_slack_payload_splits_long_lists_into_several_blocks() {
    let profiles: Vec<_> = (0..60)
        .map(|i| pr(&format!("profil-{:02}-avec-un-nom-assez-long-pour-remplir", i), ProfileType::Filesystem, true, Some(5_000_000), None))
        .collect();
    let v = build_slack_payload(&report(profiles));
    for b in v["attachments"][0]["blocks"].as_array().unwrap() {
        if let Some(t) = b["text"]["text"].as_str() {
            assert!(t.chars().count() <= 3000);
        }
    }
}

/// Serveur HTTP minimal : accepte une connexion, renvoie `status_line` + `body`, retourne la requête reçue.
fn serve_once(status_line: &'static str, body: &'static str) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/services/T000/B000/SECRET", listener.local_addr().unwrap());
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut data = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = stream.read(&mut buf).unwrap();
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data).to_string();
            if let Some(pos) = text.find("\r\n\r\n") {
                let len: usize = text[..pos].lines()
                    .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                    .unwrap_or(0);
                if data.len() >= pos + 4 + len { break; }
            }
            if n == 0 { break; }
        }
        let resp = format!("HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status_line, body.len(), body);
        stream.write_all(resp.as_bytes()).unwrap();
        String::from_utf8_lossy(&data).to_string()
    });
    (url, handle)
}

fn cfg(url: &str) -> Option<NotificationConfig> {
    Some(NotificationConfig { enabled: true, slack_webhook_url: Some(url.to_string()), slack_webhook_env: None })
}

#[test]
fn test_real_http_post_with_ureq_sends_json_report() {
    let (url, handle) = serve_once("200 OK", "ok");
    let svc = WebhookNotificationService::new(cfg(&url));
    let r = report(vec![pr("ghost", ProfileType::Mysql, false, None, Some("boom")), pr("a", ProfileType::Postgres, true, Some(10), None)]);
    svc.notify_report(&r).expect("envoi HTTP attendu");

    let request = handle.join().unwrap();
    assert!(request.starts_with("POST /services/T000/B000/SECRET HTTP/1.1"), "{}", request.lines().next().unwrap());
    assert!(request.to_ascii_lowercase().contains("content-type: application/json"));
    let body = request.split("\r\n\r\n").nth(1).unwrap();
    let json: serde_json::Value = serde_json::from_str(body).expect("corps JSON valide");
    assert_eq!(json["text"], r.headline());
    assert!(json["attachments"][0]["blocks"].as_array().unwrap().len() >= 3);
}

#[test]
fn test_http_error_is_reported_without_leaking_webhook_url() {
    let (url, handle) = serve_once("404 Not Found", "no_service");
    let svc = WebhookNotificationService::new(cfg(&url));
    let err = svc.notify("test").unwrap_err().to_string();
    handle.join().unwrap();
    assert!(err.contains("404") && err.contains("no_service"), "{}", err);
    assert!(!err.contains("SECRET") && !err.contains("127.0.0.1"), "l'URL du webhook ne doit pas fuiter : {}", err);
}

#[test]
fn test_unreachable_webhook_error_does_not_leak_url() {
    let url = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}/SECRET", l.local_addr().unwrap())
    }; // le port est refermé : connexion refusée
    let svc = WebhookNotificationService::new(cfg(&url));
    let err = svc.notify("test").unwrap_err().to_string();
    assert!(!err.contains("SECRET"), "{}", err);
}

#[test]
fn test_disabled_or_missing_webhook_is_silent() {
    let svc = WebhookNotificationService::new(None);
    assert!(svc.notify("x").is_ok());
    let off = NotificationConfig { enabled: false, slack_webhook_url: Some("http://127.0.0.1:1/x".into()), slack_webhook_env: None };
    assert!(WebhookNotificationService::new(Some(off)).notify("x").is_ok());
}

#[test]
fn test_backup_cycle_sends_detailed_report_with_counts() {
    let env = TestEnvironment::new();
    let mut config = env.build_sample_config();
    config.profiles.truncate(2);
    config.profiles[1].source.container_name = None;
    config.profiles[1].source.host = None; // validation non rejouée ici ; l'échec vient du dump simulé
    let storage = env.storage();
    let _ = BackupAction::new(&env.executor, &storage, &env.lock_service, &env.notifier)
        .execute(&config, None, false);
    let msgs = env.notifier.get_messages();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].contains("Total : 2"), "{}", msgs[0]);
}

#[test]
fn test_dry_run_sends_no_notification() {
    let env = TestEnvironment::new();
    let config = env.build_sample_config();
    let storage = env.storage();
    BackupAction::new(&env.executor, &storage, &env.lock_service, &env.notifier)
        .execute(&config, None, true)
        .unwrap();
    assert!(env.notifier.get_messages().is_empty());
}
