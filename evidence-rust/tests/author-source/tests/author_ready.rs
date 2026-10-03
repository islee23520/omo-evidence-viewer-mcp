#[path = "support/capture.rs"]
mod capture;
#[path = "support/listener.rs"]
mod listener;
#[path = "support/oracle.rs"]
mod oracle;
#[path = "support/setup.rs"]
mod setup;
#[path = "support/support.rs"]
mod support;
#[path = "support/wire.rs"]
mod wire;

use omo_evidence_storage::{
    authority::Authority,
    files::Files,
    http::{App, router},
    storage::Storage,
};
use sea_orm::{ConnectionTrait, Database};
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    time::Duration,
};
use support::{Fixture, Result};
use tokio::io::AsyncWriteExt;

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    Ok(value.get(key).and_then(Value::as_str).ok_or("field")?)
}
fn archive(name: &str, value: &Value) -> Result {
    let path = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(Path::new(&path).join(name))?;
    serde_json::to_writer(&mut file, value)?;
    file.sync_all()?;
    Ok(())
}
async fn bind(f: &Fixture, version: i64, id: i64, handle: &str) -> Result {
    let proof = f.proof("owner", version, id, handle).await?;
    let token = f
        .prepare("owner", Some(text(&proof, "proofId")?), version, 200)
        .await?;
    f.confirm(
        "owner",
        Some(text(&proof, "proofId")?),
        version,
        text(&token, "token")?,
        200,
    )
    .await?;
    Ok(())
}
fn csrf(handle: &str) -> Result<String> {
    use base64::Engine;
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(
        b"private-author-independent-evidence-transport-credential",
    )?;
    mac.update(format!("csrf:evidence:{handle}").as_bytes());
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes()))
}
fn form(slug: &str) -> reqwest::multipart::Form {
    reqwest::multipart::Form::new().text("metadata",json!({"slug":slug,"title":"Author-ready synthetic","repository":"https://github.com/example/author-source","visibility":"private"}).to_string())
    .part("files",reqwest::multipart::Part::bytes(vec![0,13,10,239,187,191,97,98,13,10,99,255]).file_name("proof.bin"))
    .part("files",reqwest::multipart::Part::bytes(b"<html><body>Synthetic</body></html>".to_vec()).file_name("index.html"))
    .part("files",reqwest::multipart::Part::bytes(json!([{"sha256":"aee1d09ecc1633154fa7002849c4b17cead7948c8d3e61afd7a0d44545ae2845","name":"Synthetic"}]).to_string().into_bytes()).file_name("manifest.json"))
}
async fn capture_response(response: reqwest::Response) -> Result<Value> {
    use base64::Engine;
    let status = response.status().as_u16();
    let path = response.url().path().to_owned();
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_str().unwrap_or("").to_owned()))
        .collect::<Vec<_>>();
    let bytes = response.bytes().await?;
    let record = json!({"path":path,"status":status,"headers":headers,"bodyBase64":base64::engine::general_purpose::STANDARD.encode(&bytes),"sha256":omo_evidence_storage::hash(&bytes)});
    let archive = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(Path::new(&archive).join("author-storage-http.jsonl"))?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    let mut value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if let Some(object) = value.as_object_mut() {
        object.insert("httpStatus".into(), json!(status));
    } else {
        value = json!({"httpStatus":status,"bodySha256":omo_evidence_storage::hash(&bytes)});
    }
    Ok(value)
}
#[tokio::test]
async fn author_ready_source_storage() -> Result {
    let f = Fixture::start().await?;
    let result = run(&f).await;
    let cleanup = f.cleanup().await;
    result?;
    cleanup
}
async fn run(f: &Fixture) -> Result {
    let (_, keys) = setup::configure(f).await?;
    let browser = setup::session(f, "evidence").await?;
    let machine = keys
        .iter()
        .find(|key| key.metadata.service_id == "evidence")
        .ok_or("machine key")?;
    bind(f, 0, 81001, "source-author").await?;
    let admin = Database::connect(support::qa_url()?.as_str()).await?;
    let dbname = format!(
        "evidence_content19_{}_{}",
        std::process::id(),
        time::OffsetDateTime::now_utc().unix_timestamp_nanos()
    );
    admin
        .execute_unprepared(&format!("CREATE DATABASE {dbname}"))
        .await?;
    println!("{}", json!({"ownedDatabase":dbname,"phase":"created"}));
    let mut url = support::qa_url()?;
    url.set_path(&format!("/{dbname}"));
    let db = Database::connect(url.as_str()).await?;
    Storage::migrate(&db).await?;
    let temp = tempfile::tempdir()?;
    let root = fs::canonicalize(temp.path())?.join("content");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let storage = Storage {
        db: db.clone(),
        files: Files::open(&root)?,
    };
    let secret = "private-author-independent-evidence-transport-credential";
    let app = router(App {
        storage: storage.clone(),
        authority: Authority::new(f.private.origin.parse()?, secret.into())?,
        origin: "https://evidence.linalab.io".into(),
    });
    let surface = wire::Surface::start(app).await?;
    let origin = surface.origin.clone();
    let cookie = format!("__Host-linalab-evidence={browser}");
    let csrf = csrf(&browser)?;
    let mut records = Vec::new();
    for (lane, credential) in [
        ("browser", browser.as_str()),
        ("machine", machine.raw_key.as_str()),
    ] {
        let request = f
            .client
            .post(format!("{origin}/api/evidence"))
            .header("x-github-handle", "caller-forged")
            .header("x-user-id", "caller-forged")
            .multipart(form(lane));
        let request = if lane == "browser" {
            request
                .header("cookie", &cookie)
                .header("origin", "https://evidence.linalab.io")
                .header("x-csrf-token", &csrf)
        } else {
            request.bearer_auth(credential)
        };
        let record = capture_response(request.send().await?).await?;
        assert_eq!(record.get("httpStatus"), Some(&json!(201)));
        assert_eq!(
            record.pointer("/authorBinding/githubUserId"),
            Some(&json!(81001))
        );
        assert_eq!(
            record.pointer("/authorBinding/githubHandle"),
            Some(&json!("source-author"))
        );
        assert_eq!(
            record.pointer("/authorBinding/bindingVersion"),
            Some(&json!(1))
        );
        let slug = text(&record, "slug")?;
        let entry = storage.get(slug).await?;
        assert_eq!(
            entry.owner.as_deref(),
            f.users.get("owner").map(String::as_str)
        );
        assert_eq!(
            entry.provenance.pointer("/submittedBy/principalType"),
            Some(&json!(lane))
        );
        assert_eq!(
            entry
                .provenance
                .pointer("/submittedBy/keyId")
                .and_then(Value::as_str),
            if lane == "machine" {
                Some(machine.metadata.id.as_str())
            } else {
                None
            }
        );
        let read = capture_response(
            f.client
                .get(format!("{origin}/evidence/{slug}/proof.bin?raw=1"))
                .header("cookie", &cookie)
                .send()
                .await?,
        )
        .await?;
        assert_eq!(read.get("httpStatus"), Some(&json!(200)));
        assert_eq!(
            read.get("bodySha256"),
            Some(&json!(
                "aee1d09ecc1633154fa7002849c4b17cead7948c8d3e61afd7a0d44545ae2845"
            ))
        );
        let denied = capture_response(
            f.client
                .patch(format!("{origin}/api/evidence/{slug}/visibility"))
                .bearer_auth(&machine.raw_key)
                .json(&json!({"visibility":"public"}))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(denied.get("httpStatus"), Some(&json!(403)));
        let denied=capture_response(f.client.post(format!("{origin}/api/reviews?slug={slug}")).bearer_auth(&machine.raw_key).json(&json!({"sha256":"aee1d09ecc1633154fa7002849c4b17cead7948c8d3e61afd7a0d44545ae2845","verdict":"pass","note":"machine denied"})).send().await?).await?;
        assert_eq!(denied.get("httpStatus"), Some(&json!(403)));
        let ranged = capture_response(
            f.client
                .get(format!("{origin}/evidence/{slug}/proof.bin?raw=1"))
                .header("cookie", &cookie)
                .header("range", "bytes=1-4")
                .send()
                .await?,
        )
        .await?;
        assert_eq!(ranged.get("httpStatus"), Some(&json!(206)));
        assert_eq!(
            ranged.get("bodySha256"),
            Some(&json!(omo_evidence_storage::hash(&[13, 10, 239, 187])))
        );
        let reviewed=capture_response(f.client.post(format!("{origin}/api/reviews?slug={slug}")).header("cookie",&cookie).header("origin","https://evidence.linalab.io").header("x-csrf-token",&csrf).json(&json!({"sha256":"aee1d09ecc1633154fa7002849c4b17cead7948c8d3e61afd7a0d44545ae2845","verdict":"pass","note":"source-author review"})).send().await?).await?;
        assert_eq!(reviewed.get("httpStatus"), Some(&json!(200)));
        records.push(record);
    }
    // Stream acceptance is observed in the durable stage ledger before unlink.
    let publication_events = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let metadata=json!({"slug":"stale-author","title":"Synthetic","repository":"https://github.com/example/author-source"}).to_string();
    let prefix = format!(
        "--qa\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--qa\r\nContent-Disposition: form-data; name=\"files\"; filename=\"partial.txt\"\r\n\r\naccepted bytes"
    );
    let suffix = "\r\n--qa--\r\n";
    let mut events = storage.files.subscribe();
    let mut tcp = tokio::net::TcpStream::connect(origin.trim_start_matches("http://")).await?;
    let headers = format!(
        "POST /api/evidence HTTP/1.1\r\nHost: evidence.linalab.io\r\nCookie: {cookie}\r\nOrigin: https://evidence.linalab.io\r\nX-CSRF-Token: {csrf}\r\nContent-Type: multipart/form-data; boundary=qa\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        prefix.len() + suffix.len()
    );
    // Exact event seam is supplied by stage fsync; listener records through an owned file subscription.
    let before = storage.list().await?.len();
    tcp.write_all(headers.as_bytes()).await?;
    tcp.write_all(prefix.as_bytes()).await?;
    let allocated = tokio::time::timeout(Duration::from_secs(10), events.recv()).await??;
    assert_eq!(allocated.get("state"), Some(&json!("allocated")));
    let confirmation = f.prepare("owner", None, 1, 200).await?;
    f.confirm("owner", None, 1, text(&confirmation, "token")?, 200)
        .await?;
    tcp.write_all(suffix.as_bytes()).await?;
    let mut response = Vec::new();
    use tokio::io::AsyncReadExt;
    tokio::time::timeout(Duration::from_secs(10), tcp.read_to_end(&mut response)).await??;
    assert!(
        String::from_utf8_lossy(&response).contains("403")
            || String::from_utf8_lossy(&response).contains("409")
    );
    assert_eq!(storage.list().await?.len(), before);
    archive(
        "author-source-proof.json",
        &json!({"authorityCommit":evidence_author_source_qa::AUTHORITY_COMMIT,"authorityTree":evidence_author_source_qa::AUTHORITY_TREE,"records":records,"staleAuthorDenied":true,"sourceOnly":true,"archive":publication_events}),
    )?;
    bind(f, 2, 81002, "relinked-source").await?;
    for (case, column, value) in [
        ("handle", "handle", "'changed-handle'"),
        ("version", "version", "4"),
    ] {
        let mut events = storage.files.subscribe();
        let mut tcp = tokio::net::TcpStream::connect(origin.trim_start_matches("http://")).await?;
        tcp.write_all(headers.as_bytes()).await?;
        tcp.write_all(prefix.as_bytes()).await?;
        let event = tokio::time::timeout(Duration::from_secs(10), events.recv()).await??;
        assert_eq!(event.get("state"), Some(&json!("allocated")));
        let user = f.users.get("owner").ok_or("owner")?;
        f.db.execute_unprepared(&format!(
            "UPDATE management.github_binding_v1 SET {column}={value} WHERE user_id='{user}'"
        ))
        .await?;
        tcp.write_all(suffix.as_bytes()).await?;
        let mut response = Vec::new();
        tokio::time::timeout(Duration::from_secs(10), tcp.read_to_end(&mut response)).await??;
        assert!(
            String::from_utf8_lossy(&response).contains("409"),
            "stale {case}"
        );
        assert_eq!(storage.list().await?.len(), before);
        f.db.execute_unprepared(&format!("UPDATE management.github_binding_v1 SET handle='relinked-source',version=3 WHERE user_id='{user}'")).await?;
    }
    let healthy = records.first().ok_or("healthy upload")?;
    let entry = storage.get(text(healthy, "slug")?).await?;
    let request = capture_response(
        f.client
            .get(format!("{origin}/evidence/{}/proof.bin", entry.slug))
            .header("cookie", &cookie)
            .header("x-github-handle", "forged")
            .send()
            .await?,
    )
    .await?;
    assert_eq!(request.get("httpStatus"), Some(&json!(200)));
    let user = f.users.get("owner").ok_or("owner")?;
    for (change, restore) in [
        (
            "UPDATE management.service_grant SET revoked=true WHERE service_id='evidence'",
            "UPDATE management.service_grant SET revoked=false WHERE service_id='evidence'",
        ),
        (
            "UPDATE management.service SET enabled=false WHERE id='evidence'",
            "UPDATE management.service SET enabled=true WHERE id='evidence'",
        ),
        (
            "UPDATE management.service_session_v1 SET revoked_at=expires_at-1 WHERE service_id='evidence'",
            "UPDATE management.service_session_v1 SET revoked_at=NULL WHERE service_id='evidence'",
        ),
        (
            "UPDATE management.access_account_v1 SET active=false,suspended_at=extract(epoch FROM now())::bigint",
            "UPDATE management.access_account_v1 SET active=true,suspended_at=NULL",
        ),
    ] {
        f.db.execute_unprepared(change).await?;
        let denied = capture_response(
            f.client
                .post(format!("{origin}/api/evidence"))
                .header("cookie", &cookie)
                .header("origin", "https://evidence.linalab.io")
                .header("x-csrf-token", &csrf)
                .multipart(form("revoked"))
                .send()
                .await?,
        )
        .await?;
        assert_eq!(denied.get("httpStatus"), Some(&json!(403)));
        assert_eq!(storage.list().await?.len(), before);
        f.db.execute_unprepared(restore).await?;
    }
    assert!(user.starts_with("access:v1:"));
    // Real SQL delay barrier occurs after durable files and before final authority observation.
    // It never substitutes an author oracle or allows a caller-controlled service field.
    db.execute_unprepared("CREATE FUNCTION evidence.author_commit_barrier() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN PERFORM pg_advisory_xact_lock(191920270003); RETURN NEW; END$$; CREATE TRIGGER author_commit_barrier BEFORE INSERT ON evidence.evidence_audit FOR EACH ROW WHEN (NEW.action='upload.commit') EXECUTE FUNCTION evidence.author_commit_barrier()").await?;
    let barrier = Database::connect(url.as_str()).await?;
    barrier
        .execute_unprepared("SELECT pg_advisory_lock(191920270003)")
        .await?;
    let started = f
        .client
        .post(format!("{origin}/api/evidence"))
        .header("cookie", &cookie)
        .header("origin", "https://evidence.linalab.io")
        .header("x-csrf-token", &csrf)
        .multipart(form("pre-pointer"));
    let mut events = storage.files.subscribe();
    let upload = tokio::spawn(async move { started.send().await });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = events.recv().await?;
            if event.get("state") == Some(&json!("immutable")) {
                return Ok::<(), tokio::sync::broadcast::error::RecvError>(());
            }
        }
    })
    .await??;
    let proof = f.proof("owner", 3, 81003, "latency-relinked").await?;
    let confirmation = f
        .prepare("owner", Some(text(&proof, "proofId")?), 3, 200)
        .await?;
    f.confirm(
        "owner",
        Some(text(&proof, "proofId")?),
        3,
        text(&confirmation, "token")?,
        200,
    )
    .await?;
    barrier
        .execute_unprepared("SELECT pg_advisory_unlock(191920270003)")
        .await?;
    barrier.close().await?;
    let denied =
        capture_response(tokio::time::timeout(Duration::from_secs(10), upload).await???).await?;
    assert_eq!(denied.get("httpStatus"), Some(&json!(409)));
    assert_eq!(storage.list().await?.len(), before);
    db.execute_unprepared("DROP TRIGGER author_commit_barrier ON evidence.evidence_audit; DROP FUNCTION evidence.author_commit_barrier()").await?;
    // Same successful HTTP path: a held owned SQL lock prevents pointer mutation;
    // the fsynced immutable event precedes killing only this storage process.
    let secret_path = fs::canonicalize(temp.path())?.join("service-secret");
    let db_path = fs::canonicalize(temp.path())?.join("content-db");
    for (path, value) in [(&secret_path, secret), (&db_path, url.as_str())] {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
    }
    db.execute_unprepared("CREATE FUNCTION evidence.pointer_ready_signal() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN PERFORM pg_advisory_xact_lock(191920270004); RETURN NEW; END$$; CREATE TRIGGER pointer_ready_signal BEFORE UPDATE OF current_revision_id ON evidence.evidence_entry FOR EACH ROW EXECUTE FUNCTION evidence.pointer_ready_signal()").await?;
    let gate = Database::connect(url.as_str()).await?;
    gate.execute_unprepared("SELECT pg_advisory_lock(191920270004)")
        .await?;
    let binary =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/omo-evidence-storage");
    let mut child = tokio::process::Command::new(binary)
        .env("EVIDENCE_DATABASE_URL_FILE", &db_path)
        .env("EVIDENCE_CONTENT_ROOT", &root)
        .env("AUTH_EVIDENCE_SERVICE_SECRET_FILE", &secret_path)
        .env("AUTH_PRIVATE_ORIGIN", &f.private.origin)
        .env("REVIEW_ORIGIN", "https://evidence.linalab.io")
        .env("EVIDENCE_BIND", "127.0.0.1:0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    use tokio::io::AsyncBufReadExt;
    let mut ready = tokio::io::BufReader::new(child.stdout.take().ok_or("stdout")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(10), ready.next_line())
        .await??
        .ok_or("readiness")?;
    let crash_origin = format!(
        "http://{}",
        ready
            .strip_prefix("EVIDENCE_LISTENING ")
            .ok_or("listening")?
    );
    let mut events = tokio::io::BufReader::new(child.stderr.take().ok_or("stderr")?).lines();
    let started = f
        .client
        .post(format!("{crash_origin}/api/evidence"))
        .header("cookie", &cookie)
        .header("origin", "https://evidence.linalab.io")
        .header("x-csrf-token", &csrf)
        .multipart(form("crash-upload"));
    let request = tokio::spawn(async move { started.send().await });
    let immutable = tokio::time::timeout(Duration::from_secs(10), async {
        while let Some(line) = events.next_line().await? {
            if let Ok(event) = serde_json::from_str::<Value>(&line)
                && event.get("state") == Some(&json!("immutable"))
            {
                return Ok::<Value, Box<dyn std::error::Error + Send + Sync>>(event);
            }
        }
        Err("immutable event absent".into())
    })
    .await??;
    child.kill().await?;
    assert!(
        !tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await??
            .success()
    );
    let _request = tokio::time::timeout(Duration::from_secs(10), request).await?;
    gate.execute_unprepared("SELECT pg_advisory_unlock(191920270004)")
        .await?;
    gate.close().await?;
    db.execute_unprepared("DROP TRIGGER pointer_ready_signal ON evidence.evidence_entry; DROP FUNCTION evidence.pointer_ready_signal()").await?;
    assert_eq!(storage.list().await?.len(), before);
    archive(
        "successful-upload-crash.json",
        &json!({"immutable":immutable,"ownedProcessKilled":true,"lastHealthyCount":before,"pointerUnchanged":true}),
    )?;
    let referenced = storage
        .list()
        .await?
        .iter()
        .map(|entry| entry.digest.clone())
        .collect();
    let recovered = storage.files.recover(&referenced)?;
    archive(
        "author-upload-orphans.json",
        &json!({"records":recovered,"noDeletion":true}),
    )?;
    for record in &records {
        let entry = storage.get(text(record, "slug")?).await?;
        assert_eq!(
            entry.provenance.pointer("/authorBinding/githubUserId"),
            Some(&json!(81001))
        );
    }
    let backup_path = fs::canonicalize(temp.path())?.join("backup");
    let backup_hash = omo_evidence_storage::backup::create(&storage, &backup_path).await?;
    archive(
        "author-backup-proof.json",
        &json!({"backupHash":backup_hash,"manifest":serde_json::from_slice::<Value>(&fs::read(backup_path.join("backup.json"))?)?,"immutableAuthorsPreserved":true}),
    )?;
    let restore_name = format!("{dbname}_restore");
    admin
        .execute_unprepared(&format!("CREATE DATABASE {restore_name}"))
        .await?;
    println!(
        "{}",
        json!({"ownedDatabase":restore_name,"phase":"created"})
    );
    let mut restore_url = url.clone();
    restore_url.set_path(&format!("/{restore_name}"));
    let restore_db = Database::connect(restore_url.as_str()).await?;
    Storage::migrate(&restore_db).await?;
    let restore_root = fs::canonicalize(temp.path())?.join("restored");
    fs::create_dir(&restore_root)?;
    fs::set_permissions(&restore_root, fs::Permissions::from_mode(0o700))?;
    let restored = Storage {
        db: restore_db.clone(),
        files: Files::open(&restore_root)?,
    };
    omo_evidence_storage::backup::restore(&restored, &backup_path).await?;
    for record in &records {
        let slug = text(record, "slug")?;
        let original = storage.get(slug).await?;
        let restored_entry = restored.get(slug).await?;
        assert_eq!(restored_entry.provenance, original.provenance);
        assert_eq!(restored_entry.revision, original.revision);
        assert_eq!(
            restored.files.read(&restored_entry.digest, "proof.bin")?,
            storage.files.read(&original.digest, "proof.bin")?
        );
        assert_eq!(
            restored.reviews(&restored_entry).await?,
            storage.reviews(&original).await?
        );
    }
    restore_db.close().await?;
    admin
        .execute_unprepared(&format!("DROP DATABASE {restore_name}"))
        .await?;
    surface.stop().await?;
    db.close().await?;
    admin
        .execute_unprepared(&format!("DROP DATABASE {dbname}"))
        .await?;
    admin.close().await?;
    fn thaw(path: &Path) -> Result {
        if path.is_dir() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            for child in fs::read_dir(path)? {
                thaw(&child?.path())?;
            }
        }
        Ok(())
    }
    thaw(temp.path())?;
    temp.close()?;
    Ok(())
}
