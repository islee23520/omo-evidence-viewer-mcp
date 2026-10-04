use super::support::{Fixture, Result};
use sea_orm::ConnectionTrait;
use std::{
    fs, io::Write, os::unix::fs::OpenOptionsExt, path::Path, process::Stdio, time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

pub async fn verify(
    f: &Fixture,
    content: &Path,
    database: &url::Url,
    secret: &str,
    session: &str,
) -> Result {
    let temp = tempfile::tempdir()?;
    let root = fs::canonicalize(temp.path())?;
    let key = root.join("leaf.key");
    let ca = root.join("leaf.pem");
    let certificate = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-sha256",
            "-days",
            "1",
            "-subj",
            "/CN=synthetic-transport",
            "-addext",
            "subjectAltName=DNS:localhost",
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-keyout",
        ])
        .arg(&key)
        .arg("-out")
        .arg(&ca)
        .kill_on_drop(true)
        .output()
        .await?;
    assert!(certificate.status.success());
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&ca, fs::Permissions::from_mode(0o600))?;
    let script = r#"
const fs=require('node:fs'),https=require('node:https'),http=require('node:http');let calls=0;
const s=https.createServer({key:fs.readFileSync(process.argv[1]),cert:fs.readFileSync(process.argv[2])},(q,r)=>{
 calls++; const p=http.request(new URL(q.url,process.argv[3]),{method:q.method,headers:q.headers},v=>{r.writeHead(v.statusCode,v.headers);v.pipe(r)});p.on('error',()=>{r.writeHead(503);r.end()});q.pipe(p);
});s.on('tlsClientError',()=>{});s.listen(0,'127.0.0.1',()=>console.log('READY '+s.address().port));process.stdin.on('data',()=>console.log('CALLS '+calls));
"#;
    let mut proxy = Command::new("node")
        .arg("-e")
        .arg(script)
        .arg(&key)
        .arg(&ca)
        .arg(&f.private.origin)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut lines = BufReader::new(proxy.stdout.take().ok_or("proxy stdout")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await??
        .ok_or("TLS ready")?;
    let port: u16 = ready
        .strip_prefix("READY ")
        .ok_or("TLS ready prefix")?
        .parse()?;
    let origin = format!("https://localhost:{port}/");
    let dbfile = root.join("database");
    let service = root.join("service");
    for (path, value) in [(&dbfile, database.as_str()), (&service, secret)] {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)?;
        file.write_all(value.as_bytes())?;
        file.sync_all()?;
    }
    let binary =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/omo-evidence-storage");
    let mut child = Command::new(&binary)
        .env("EVIDENCE_DATABASE_URL_FILE", &dbfile)
        .env("EVIDENCE_CONTENT_ROOT", content)
        .env("AUTH_EVIDENCE_SERVICE_SECRET_FILE", &service)
        .env("AUTH_PRIVATE_ORIGIN", &origin)
        .env("AUTH_PRIVATE_CA_FILE", &ca)
        .env("REVIEW_ORIGIN", "https://evidence.linalab.io")
        .env("EVIDENCE_BIND", "127.0.0.1:0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut output = BufReader::new(child.stdout.take().ok_or("binary stdout")?).lines();
    let ready = tokio::time::timeout(Duration::from_secs(10), output.next_line())
        .await??
        .ok_or("Evidence ready")?;
    let address = ready
        .strip_prefix("EVIDENCE_LISTENING ")
        .ok_or("Evidence prefix")?;
    let response = f
        .client
        .get(format!("http://{address}/api/evidence"))
        .header("cookie", format!("__Host-linalab-evidence={session}"))
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    // The next request observes current authority state, not a TLS cache.
    f.db.execute_unprepared("UPDATE management.service SET enabled=false WHERE id='evidence'")
        .await?;
    let denied = f
        .client
        .get(format!("http://{address}/api/evidence"))
        .header("cookie", format!("__Host-linalab-evidence={session}"))
        .send()
        .await?;
    assert_eq!(denied.status(), 403);
    f.db.execute_unprepared("UPDATE management.service SET enabled=true WHERE id='evidence'")
        .await?;
    proxy
        .stdin
        .as_mut()
        .ok_or("proxy stdin")?
        .write_all(b"count\n")
        .await?;
    let calls = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await??
        .ok_or("calls")?;
    assert_eq!(calls, "CALLS 2");
    child.kill().await?;
    child.wait().await?;
    // Startup failures use the actual binary's existing redacted error path.
    for reference in [root.join("missing.pem"), ca.clone()] {
        if reference == ca {
            fs::set_permissions(&ca, fs::Permissions::from_mode(0o644))?;
        }
        let output = tokio::time::timeout(
            Duration::from_secs(10),
            Command::new(&binary)
                .env("EVIDENCE_DATABASE_URL_FILE", &dbfile)
                .env("EVIDENCE_CONTENT_ROOT", content)
                .env("AUTH_EVIDENCE_SERVICE_SECRET_FILE", &service)
                .env("AUTH_PRIVATE_ORIGIN", &origin)
                .env("AUTH_PRIVATE_CA_FILE", &reference)
                .env("REVIEW_ORIGIN", "https://evidence.linalab.io")
                .env("EVIDENCE_BIND", "127.0.0.1:0")
                .kill_on_drop(true)
                .output(),
        )
        .await??;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8(output.stderr)?;
        assert!(!error.contains(secret));
        assert!(!error.contains(reference.to_str().ok_or("reference")?));
        assert!(error.contains("Evidence operation failed"));
    }
    proxy.kill().await?;
    proxy.wait().await?;
    temp.close()?;
    println!("EVIDENCE_BINARY_CURRENT_AUTHORITY_TLS_PASS");
    Ok(())
}
