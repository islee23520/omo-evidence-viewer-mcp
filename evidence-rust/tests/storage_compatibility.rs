//! Listening native binary, real PostgreSQL and externally launched source-owned authority.
use omo_evidence_storage::{
    backup,
    files::{Files, safe_path},
    hash,
    storage::{Storage, statement},
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};
use tokio::io::{AsyncBufReadExt, BufReader};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Fixture {
    #[serde(rename = "databaseURL")]
    database_url: String,
    #[serde(rename = "ownerDatabaseURL")]
    owner_database_url: String,
    authority_origin: String,
    service_secret: String,
    browser_handle: String,
    owner_id: String,
    #[serde(rename = "authorityDatabaseURL")]
    authority_database_url: String,
    #[serde(rename = "restoreDatabaseURL")]
    restore_database_url: String,
    machine_key: String,
}
fn protected(path: &Path, bytes: &[u8]) -> Result {
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(bytes)?;
    Ok(())
}
async fn capture(response: reqwest::Response) -> Result<reqwest::Response> {
    use base64::Engine;
    let status = response.status();
    let headers = response.headers().clone();
    let version = response.version();
    let path = response.url().path().to_owned();
    let bytes = response.bytes().await?;
    let record = json!({"path":path,"status":status.as_u16(),"headers":headers.iter().map(|(k,v)|(k.to_string(),v.to_str().unwrap_or("").to_owned())).collect::<Vec<_>>(),"bodyBase64":base64::engine::general_purpose::STANDARD.encode(&bytes),"bodySha256":hash(&bytes)});
    let archive = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let mut file = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(Path::new(&archive).join("http-complete.jsonl"))?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    let mut builder = axum::http::Response::builder()
        .status(status)
        .version(version);
    *builder.headers_mut().ok_or("response headers")? = headers;
    Ok(builder.body(bytes.to_vec())?.into())
}
trait CapturedSend {
    async fn send_captured(self) -> Result<reqwest::Response>;
}
impl CapturedSend for reqwest::RequestBuilder {
    async fn send_captured(self) -> Result<reqwest::Response> {
        capture(reqwest::RequestBuilder::send(self).await?).await
    }
}
async fn db(url: &str) -> Result<sea_orm::DatabaseConnection> {
    let mut options = ConnectOptions::new(url);
    options.sqlx_logging(false);
    Ok(Database::connect(options).await?)
}
#[tokio::test]
async fn storage_compatibility() -> Result {
    let path = std::env::var("EVIDENCE_STORAGE_QA_FIXTURE")?;
    assert_eq!(fs::metadata(&path)?.permissions().mode() & 0o077, 0);
    let fixture: Fixture = serde_json::from_slice(&fs::read(path)?)?;
    let database = db(&fixture.database_url).await?;
    let owner_database = db(&fixture.owner_database_url).await?;
    let authority_db = db(&fixture.authority_database_url).await?;
    let temp = tempfile::tempdir()?;
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o700))?;
    let root = fs::canonicalize(temp.path())?.join("content");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let files = Files::open(&root)?;
    println!("QA_FILES_OPEN");
    Storage::migrate(&owner_database).await?;
    println!("QA_CONTENT_MIGRATED");
    assert!(
        database
            .execute_unprepared("CREATE TABLE evidence.forbidden(id int)")
            .await
            .is_err()
    );
    assert!(
        database
            .execute_unprepared("DELETE FROM evidence.evidence_audit")
            .await
            .is_err()
    );
    let storage = Storage {
        db: database.clone(),
        files,
    };
    let original = [0, 13, 10, 239, 187, 191, 97, 98, 13, 10, 99, 255];
    let sha = hash(&original);
    assert_eq!(
        sha,
        "aee1d09ecc1633154fa7002849c4b17cead7948c8d3e61afd7a0d44545ae2845"
    );
    let mut stage = storage.files.stage()?;
    stage.add(
        "index.html".into(),
        b"<html><body><h1>Synthetic proof</h1></body></html>",
    )?;
    stage.add("assets/proof.bin".into(), &original)?;
    stage.add(
        "manifest.json".into(),
        &serde_json::to_vec(&json!([{"sha256":sha,"name":"Synthetic proof"}]))?,
    )?;
    println!("QA_STAGE_READY");
    let entry=storage.import("legacy-synthetic-20261003",Some(&fixture.owner_id),json!({"title":"Protected synthetic title","repository":"https://github.com/example/synthetic","visibility":"public","date":"2026-10-03","createdAt":"2026-10-03T00:00:00.000Z","tags":[]}),stage,&"a".repeat(64)).await?;
    assert_eq!(entry.slug, "legacy-synthetic-20261003");
    assert_eq!(entry.provenance.get("author"), Some(&json!("unknown")));
    let before = entry.revision.clone();
    let mut duplicate = storage.files.stage()?;
    duplicate.add("case.txt".into(), b"first")?;
    assert!(duplicate.add("CASE.TXT".into(), b"second").is_err());
    assert!(duplicate.add("case.txt/nested".into(), b"second").is_err());
    storage.files.abort(duplicate)?;
    // A DB failure AFTER durable files must retain the old pointer and an owned orphan.
    owner_database.execute_unprepared("CREATE FUNCTION evidence.qa_reject_revision() RETURNS trigger LANGUAGE plpgsql AS $$BEGIN RAISE EXCEPTION 'owned synthetic DB fault'; END$$; CREATE TRIGGER qa_reject_revision BEFORE INSERT ON evidence.evidence_revision FOR EACH ROW EXECUTE FUNCTION evidence.qa_reject_revision()").await?;
    let mut failed_stage = storage.files.stage()?;
    failed_stage.add("db-fault.txt".into(), b"durable synthetic orphan")?;
    assert!(
        storage
            .import(
                "db-fault-synthetic",
                Some(&fixture.owner_id),
                json!({"visibility":"private"}),
                failed_stage,
                &"b".repeat(64)
            )
            .await
            .is_err()
    );
    owner_database.execute_unprepared("DROP TRIGGER qa_reject_revision ON evidence.evidence_revision; DROP FUNCTION evidence.qa_reject_revision()").await?;
    assert_eq!(storage.get(&entry.slug).await?.revision, before);
    assert!(fs::read_to_string(root.join("ownership.jsonl"))?.contains("db_failed_orphan"));
    // Hash claims cannot seal altered actual bytes; no DB pointer is written.
    let mut tampered = storage.files.stage()?;
    tampered.add("tampered.txt".into(), b"original")?;
    fs::write(tampered.directory.join("tampered.txt"), b"altered")?;
    assert!(storage.files.seal(tampered).is_err());
    storage.files.verify(&entry.digest, &entry.manifest)?;
    protected(&temp.path().join("db"), fixture.database_url.as_bytes())?;
    protected(
        &temp.path().join("service"),
        fixture.service_secret.as_bytes(),
    )?;
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_omo-evidence-storage"));
    command
        .env("EVIDENCE_DATABASE_URL_FILE", temp.path().join("db"))
        .env("EVIDENCE_CONTENT_ROOT", &root)
        .env(
            "AUTH_EVIDENCE_SERVICE_SECRET_FILE",
            temp.path().join("service"),
        )
        .env("AUTH_PRIVATE_ORIGIN", &fixture.authority_origin)
        .env("REVIEW_ORIGIN", "https://evidence.linalab.io")
        .env("EVIDENCE_BIND", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn()?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("stdout")?).lines();
    let line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await??
        .ok_or("readiness")?;
    let origin = format!(
        "http://{}",
        line.strip_prefix("EVIDENCE_LISTENING ")
            .ok_or("readiness prefix")?
    );
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;
    let cookie = format!("__Host-linalab-evidence={}", fixture.browser_handle);
    use base64::Engine;
    use hmac::Mac;
    let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(fixture.service_secret.as_bytes())?;
    mac.update(format!("csrf:evidence:{}", fixture.browser_handle).as_bytes());
    let csrf = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    let private = format!("{origin}/evidence/{}/assets/proof.bin", entry.slug);
    let public = format!("{origin}/public/{}/assets/proof.bin", entry.slug);
    let mut http = Vec::<Value>::new();
    let response = client
        .get(&private)
        .header("cookie", &cookie)
        .send_captured()
        .await?;
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers().get("cache-control").ok_or("cache")?,
        "private, no-store"
    );
    let bytes = response.bytes().await?;
    assert_eq!(bytes.as_ref(), original);
    http.push(json!({"case":"private_raw","status":200,"bodyHex":"000d0aefbbbf61620d0a63ff","sha256":hash(&bytes)}));
    let response = client
        .get(&public)
        .header("range", "bytes=1-4")
        .send_captured()
        .await?;
    assert_eq!(response.status(), 206);
    assert_eq!(
        response.headers().get("content-range").ok_or("range")?,
        "bytes 1-4/12"
    );
    assert_eq!(
        response.headers().get("cache-control").ok_or("cache")?,
        "no-store"
    );
    let etag = response
        .headers()
        .get("etag")
        .ok_or("etag")?
        .to_str()?
        .to_owned();
    assert_eq!(response.bytes().await?.as_ref(), [13, 10, 239, 187]);
    http.push(json!({"case":"range","status":206,"contentRange":"bytes 1-4/12","etag":etag,"bodyHex":"0d0aefbb"}));
    assert_eq!(
        client
            .get(&public)
            .header("range", "bytes=99-")
            .send_captured()
            .await?
            .status(),
        416
    );
    assert_eq!(
        client
            .get(&public)
            .header("if-none-match", &etag)
            .send_captured()
            .await?
            .status(),
        304
    );
    let response = client
        .get(format!("{public}?download=1"))
        .send_captured()
        .await?;
    assert!(response.headers().contains_key("content-disposition"));
    assert_eq!(response.bytes().await?.as_ref(), original);
    let raw = client
        .get(format!("{origin}/public/{}/?raw=1", entry.slug))
        .send_captured()
        .await?
        .text()
        .await?;
    assert_eq!(raw, "<html><body><h1>Synthetic proof</h1></body></html>");
    assert!(
        client
            .get(format!("{origin}/public/{}/", entry.slug))
            .send_captured()
            .await?
            .text()
            .await?
            .contains("data-gallery-home")
    );
    assert_eq!(
        client
            .get(&private)
            .header("authorization", "Bearer old-upload-token")
            .header("x-linalab-evidence-principal", "browser")
            .header("x-linalab-admin", "true")
            .send_captured()
            .await?
            .status(),
        401
    );
    let reviews = format!("{origin}/api/reviews?slug={}", entry.slug);
    let response = client
        .post(&reviews)
        .header("cookie", &cookie)
        .header("origin", "https://evidence.linalab.io")
        .header("x-csrf-token", &csrf)
        .json(&json!({"sha256":sha,"verdict":"pass","note":"Synthetic note"}))
        .send_captured()
        .await?;
    assert_eq!(response.status(), 200);
    let saved: Value = response.json().await?;
    assert_eq!(saved.get("saved"), Some(&json!(true)));
    let reread: Value = client
        .get(&reviews)
        .header("cookie", &cookie)
        .send_captured()
        .await?
        .json()
        .await?;
    assert_eq!(
        reread.pointer(&format!("/decisions/{sha}/note")),
        Some(&json!("Synthetic note"))
    );
    http.push(json!({"case":"review","response":saved,"readback":reread}));
    assert_eq!(
        client
            .post(&reviews)
            .header("cookie", &cookie)
            .header("origin", "https://foreign.invalid")
            .header("x-csrf-token", &csrf)
            .json(&json!({"sha256":sha,"verdict":"fail","note":""}))
            .send_captured()
            .await?
            .status(),
        403
    );
    for name in [
        "../secret.txt",
        "C:/secret.txt",
        "CON.txt",
        "assets/../secret.txt",
        "a./b",
        "a /b",
    ] {
        assert!(!safe_path(name));
        let body=reqwest::multipart::Form::new().text("metadata",json!({"slug":"upload","title":"Synthetic","repository":"https://github.com/example/synthetic"}).to_string()).part("files",reqwest::multipart::Part::bytes(b"bytes".to_vec()).file_name(name));
        assert_eq!(
            client
                .post(format!("{origin}/api/evidence"))
                .header("cookie", &cookie)
                .header("origin", "https://evidence.linalab.io")
                .header("x-csrf-token", &csrf)
                .multipart(body)
                .send_captured()
                .await?
                .status(),
            400
        );
    }
    let form=reqwest::multipart::Form::new().text("metadata",json!({"slug":"upload","title":"Synthetic","repository":"https://github.com/example/synthetic"}).to_string()).part("files",reqwest::multipart::Part::bytes(original.to_vec()).file_name("proof.bin"));
    let response = client
        .post(format!("{origin}/api/evidence"))
        .header("cookie", &cookie)
        .header("origin", "https://evidence.linalab.io")
        .header("x-csrf-token", &csrf)
        .multipart(form)
        .send_captured()
        .await?;
    assert_eq!(response.status(), 403);
    let denied: Value = response.json().await?;
    assert_eq!(
        denied.get("error"),
        Some(&json!("author_identity_required"))
    );
    http.push(json!({"case":"author_contract_absent","status":403,"body":denied}));
    let metadata = json!({"slug":"negative","title":"Synthetic","repository":"https://github.com/example/synthetic"}).to_string();
    let machine_form = || {
        reqwest::multipart::Form::new()
            .text("metadata", metadata.clone())
            .part(
                "files",
                reqwest::multipart::Part::bytes(b"synthetic".to_vec()).file_name("machine.txt"),
            )
    };
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .bearer_auth(&fixture.machine_key)
            .multipart(machine_form())
            .send_captured()
            .await?
            .status(),
        403
    );
    authority_db
        .execute_unprepared(
            "UPDATE management.machine_key SET revoked_at=now() WHERE service_id='evidence'",
        )
        .await?;
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .bearer_auth(&fixture.machine_key)
            .multipart(machine_form())
            .send_captured()
            .await?
            .status(),
        403
    );
    authority_db
        .execute_unprepared(
            "UPDATE management.machine_key SET revoked_at=NULL WHERE service_id='evidence'",
        )
        .await?;
    let form = reqwest::multipart::Form::new()
        .text("metadata", metadata.clone())
        .part(
            "files",
            reqwest::multipart::Part::bytes(b"first".to_vec()).file_name("same.txt"),
        )
        .part(
            "files",
            reqwest::multipart::Part::bytes(b"second".to_vec()).file_name("SAME.TXT"),
        );
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .header("cookie", &cookie)
            .header("origin", "https://evidence.linalab.io")
            .header("x-csrf-token", &csrf)
            .multipart(form)
            .send_captured()
            .await?
            .status(),
        400
    );
    let mut form = reqwest::multipart::Form::new().text("metadata", metadata.clone());
    for n in 0..201 {
        form = form.part(
            "files",
            reqwest::multipart::Part::bytes(vec![b'x']).file_name(format!("{n}.txt")),
        );
    }
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .header("cookie", &cookie)
            .header("origin", "https://evidence.linalab.io")
            .header("x-csrf-token", &csrf)
            .multipart(form)
            .send_captured()
            .await?
            .status(),
        400
    );
    let truncated = format!(
        "--qa\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--qa\r\nContent-Disposition: form-data; name=\"files\"; filename=\"proof.txt\"\r\n\r\ntruncated"
    );
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .header("cookie", &cookie)
            .header("origin", "https://evidence.linalab.io")
            .header("x-csrf-token", &csrf)
            .header("content-type", "multipart/form-data; boundary=qa")
            .body(truncated)
            .send_captured()
            .await?
            .status(),
        400
    );
    let prefix = format!(
        "--qa\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n--qa\r\nContent-Disposition: form-data; name=\"files\"; filename=\"large.bin\"\r\n\r\n"
    );
    let chunks = futures_util::stream::iter(
        std::iter::once(Ok::<Vec<u8>, std::io::Error>(prefix.into_bytes()))
            .chain((0..101).map(|_| Ok(vec![b'x'; 1024 * 1024]))),
    );
    assert_eq!(
        client
            .post(format!("{origin}/api/evidence"))
            .header("cookie", &cookie)
            .header("origin", "https://evidence.linalab.io")
            .header("x-csrf-token", &csrf)
            .header("content-type", "multipart/form-data; boundary=qa")
            .body(reqwest::Body::wrap_stream(chunks))
            .send_captured()
            .await?
            .status(),
        413
    );
    // Only the deliberately retained failed seal remains; uploads removed their live stage.
    assert_eq!(fs::read_dir(root.join("staging"))?.count(), 1);
    assert_eq!(storage.get(&entry.slug).await?.revision, before);
    let patch = format!("{origin}/api/evidence/{}/visibility", entry.slug);
    assert_eq!(
        client
            .patch(&patch)
            .header("cookie", &cookie)
            .header("origin", "https://evidence.linalab.io")
            .header("x-csrf-token", &csrf)
            .json(&json!({"visibility":"private"}))
            .send_captured()
            .await?
            .status(),
        200
    );
    for header in ["range", "if-none-match"] {
        assert_eq!(
            client
                .get(&public)
                .header(
                    header,
                    if header == "range" {
                        "bytes=1-4"
                    } else {
                        &etag
                    }
                )
                .send_captured()
                .await?
                .status(),
            404
        );
    }
    authority_db.execute_raw(statement("UPDATE management.service_grant SET browser_scopes='[]'::jsonb WHERE service_id='evidence'",vec![])).await?;
    assert_eq!(
        client
            .get(&private)
            .header("cookie", &cookie)
            .header("if-none-match", &etag)
            .send_captured()
            .await?
            .status(),
        403
    );
    authority_db.execute_raw(statement("UPDATE management.service_grant SET browser_scopes='[\"evidence:read\",\"evidence:upload\",\"evidence:publish\",\"evidence:review\"]'::jsonb WHERE service_id='evidence'",vec![])).await?;
    authority_db.execute_unprepared("UPDATE management.service_session_v1 SET revoked_at=extract(epoch FROM now())::bigint WHERE service_id='evidence'").await?;
    assert_eq!(
        client
            .get(&private)
            .header("cookie", &cookie)
            .header("if-none-match", &etag)
            .send_captured()
            .await?
            .status(),
        403
    );
    authority_db
        .execute_unprepared(
            "UPDATE management.service_session_v1 SET revoked_at=NULL WHERE service_id='evidence'",
        )
        .await?;
    let backup_path = fs::canonicalize(temp.path())?.join("backup");
    let backup_hash = backup::create(&storage, &backup_path).await?;
    let backup_bytes = fs::read(backup_path.join("backup.json"))?;
    assert_eq!(hash(&backup_bytes), backup_hash);
    let restored_db = db(&fixture.restore_database_url).await?;
    Storage::migrate(&restored_db).await?;
    let restored_root = fs::canonicalize(temp.path())?.join("restored");
    fs::create_dir(&restored_root)?;
    fs::set_permissions(&restored_root, fs::Permissions::from_mode(0o700))?;
    let restored = Storage {
        db: restored_db.clone(),
        files: Files::open(&restored_root)?,
    };
    backup::restore(&restored, &backup_path).await?;
    let restored_entry = restored.get(&entry.slug).await?;
    assert_eq!(restored_entry.revision, entry.revision);
    assert_eq!(restored_entry.digest, entry.digest);
    assert_eq!(restored_entry.visibility, "private");
    assert_eq!(
        restored.files.read(&entry.digest, "assets/proof.bin")?,
        original
    );
    assert_eq!(
        restored.reviews(&restored_entry).await?,
        storage.reviews(&storage.get(&entry.slug).await?).await?
    );
    assert!(backup::restore(&restored, &backup_path).await.is_err());
    restored_db.close().await?;
    let archive = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let archive = Path::new(&archive);
    assert_eq!(fs::metadata(archive)?.permissions().mode() & 0o077, 0);
    protected(
        &archive.join("http.json"),
        &serde_json::to_vec_pretty(&http)?,
    )?;
    protected(&archive.join("backup.json"), &backup_bytes)?;
    protected(
        &archive.join("files-manifest.json"),
        &serde_json::to_vec_pretty(&entry.manifest)?,
    )?;
    protected(
        &archive.join("ownership.jsonl"),
        &fs::read(root.join("ownership.jsonl"))?,
    )?;
    // Uniquely owned child only, bounded exact exit event; no timing-based wait.
    let pid = child.id().ok_or("child pid")?;
    assert!(
        Command::new("kill")
            .args(["-INT", &pid.to_string()])
            .status()?
            .success()
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(10), child.wait())
            .await??
            .success()
    );
    authority_db.close().await?;
    database.close().await?;
    owner_database.close().await?;
    // Temp directory owns all immutable files; test cleanup does not enumerate live roots.
    fn thaw(path: &Path) -> Result {
        if path.is_dir() {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            for file in fs::read_dir(path)? {
                thaw(&file?.path())?;
            }
        } else {
            fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
    thaw(temp.path())?;
    temp.close()?;
    println!("STORAGE_COMPATIBILITY_PASS authorReadyCommits=false");
    Ok(())
}
