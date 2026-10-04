use omo_evidence_storage::{Error, authority::Authority};
use std::{fs, os::unix::fs::PermissionsExt, path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::Command,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

// A subprocess is the configuration boundary; no test changes shared env.
#[tokio::test]
#[ignore = "invoked only by the transport parent with isolated configuration"]
async fn configured_client() -> Result {
    let result = Authority::new(
        std::env::var("TRANSPORT_ORIGIN")?.parse()?,
        "synthetic-evidence-transport-secret-32".into(),
    );
    if std::env::var("TRANSPORT_EXPECT")? == "startup" {
        assert!(matches!(result, Err(Error::Invalid | Error::Unavailable)));
        return Ok(());
    }
    let authority = result?;
    // Loading happens once. Removing the reference cannot affect this client.
    if let Ok(path) = std::env::var("TRANSPORT_REMOVE_CA") {
        fs::remove_file(path)?;
    }
    let mut headers = axum::http::HeaderMap::new();
    headers.insert("authorization", "Bearer mk2_synthetic".parse()?);
    let result = authority
        .authorize(&headers, "POST", "/api/evidence", "evidence:upload")
        .await;
    let status: u16 = std::env::var("TRANSPORT_EXPECT")?.parse()?;
    assert!(
        matches!(result, Err(Error::Authority(s)) if s.as_u16() == status),
        "expected {status}, got {result:?}"
    );
    Ok(())
}

async fn client(
    origin: &str,
    ca: Option<&Path>,
    expected: &str,
    proxy: &str,
    remove: bool,
) -> Result {
    let mut command = Command::new(std::env::current_exe()?);
    command
        .args(["--ignored", "--exact", "configured_client", "--nocapture"])
        .env_remove("AUTH_PRIVATE_CA_FILE")
        .env("TRANSPORT_ORIGIN", origin)
        .env("TRANSPORT_EXPECT", expected)
        .env("HTTPS_PROXY", proxy)
        .env("HTTP_PROXY", proxy)
        .env("ALL_PROXY", proxy)
        .env("https_proxy", proxy)
        .env("http_proxy", proxy)
        .env("all_proxy", proxy)
        .env("NO_PROXY", "")
        .env("no_proxy", "")
        .kill_on_drop(true)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(path) = ca {
        command.env("AUTH_PRIVATE_CA_FILE", path);
        if remove {
            command.env("TRANSPORT_REMOVE_CA", path);
        }
    }
    let output = tokio::time::timeout(Duration::from_secs(15), command.spawn()?.wait_with_output())
        .await??;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}

async fn certificate(root: &Path, name: &str, san: &str, days: &str) -> Result {
    let output = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "rsa:2048",
            "-nodes",
            "-sha256",
            "-subj",
            "/CN=synthetic-transport",
            "-days",
            days,
            "-addext",
            san,
            "-addext",
            "basicConstraints=critical,CA:FALSE",
            "-keyout",
            &root.join(format!("{name}.key")).to_string_lossy(),
            "-out",
            &root.join(format!("{name}.pem")).to_string_lossy(),
        ])
        .kill_on_drop(true)
        .output()
        .await?;
    assert!(
        output.status.success(),
        "certificate fixture generation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::set_permissions(
        root.join(format!("{name}.pem")),
        fs::Permissions::from_mode(0o600),
    )?;
    Ok(())
}

struct Server {
    child: tokio::process::Child,
    lines: tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    origin: String,
}
impl Server {
    async fn start(root: &Path, name: &str, redirect: Option<&str>) -> Result<Self> {
        // Only a real TLS handshake can reach the request event. The fixture
        // returns a denial/redirect, never an invented authority principal.
        let script = r#"
const fs=require('node:fs'),https=require('node:https');
let calls=0;
const server=https.createServer({key:fs.readFileSync(process.argv[1]),cert:fs.readFileSync(process.argv[2])},(q,r)=>{
 calls++;q.resume();r.writeHead(process.argv[3]?302:401,process.argv[3]?{location:process.argv[3]}:{});r.end('{}');
});
server.on('tlsClientError',()=>{});
server.listen(0,'127.0.0.1',()=>console.log('READY '+server.address().port));
process.stdin.on('data',()=>console.log('CALLS '+calls));
"#;
        let mut child = Command::new("node")
            .arg("-e")
            .arg(script)
            .arg(root.join(format!("{name}.key")))
            .arg(root.join(format!("{name}.pem")))
            .arg(redirect.unwrap_or(""))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let mut lines = BufReader::new(child.stdout.take().ok_or("stdout")?).lines();
        let ready = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
            .await??
            .ok_or("ready")?;
        let port: u16 = ready
            .strip_prefix("READY ")
            .ok_or("ready prefix")?
            .parse()?;
        Ok(Self {
            child,
            lines,
            origin: format!("https://localhost:{port}/"),
        })
    }
    async fn calls(&mut self) -> Result<usize> {
        use tokio::io::AsyncWriteExt;
        self.child
            .stdin
            .as_mut()
            .ok_or("stdin")?
            .write_all(b"count\n")
            .await?;
        let line = tokio::time::timeout(Duration::from_secs(10), self.lines.next_line())
            .await??
            .ok_or("calls")?;
        Ok(line.strip_prefix("CALLS ").ok_or("calls prefix")?.parse()?)
    }
    async fn stop(mut self) -> Result {
        self.child.kill().await?;
        self.child.wait().await?;
        Ok(())
    }
}

#[tokio::test]
async fn protected_reference_and_real_tls_fail_closed() -> Result {
    let temp = tempfile::tempdir()?;
    let root = fs::canonicalize(temp.path())?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    certificate(&root, "valid", "subjectAltName=DNS:localhost", "1").await?;
    certificate(&root, "wrong", "subjectAltName=DNS:elsewhere.invalid", "1").await?;
    certificate(&root, "expired", "subjectAltName=DNS:localhost", "1").await?;
    // x509 permits an explicit past validity interval; no wall-clock wait.
    let expired = Command::new("openssl")
        .args(["x509", "-in"])
        .arg(root.join("expired.pem"))
        .arg("-signkey")
        .arg(root.join("expired.key"))
        .args([
            "-not_before",
            "20200101000000Z",
            "-not_after",
            "20200102000000Z",
            "-out",
        ])
        .arg(root.join("expired.pem"))
        .kill_on_drop(true)
        .output()
        .await?;
    assert!(expired.status.success(), "expired fixture signing failed");
    let mut trap = Server::start(&root, "valid", None).await?;
    let proxy = trap.origin.clone();
    let mut valid = Server::start(&root, "valid", None).await?;
    let ca = root.join("valid.pem");
    client(&valid.origin, Some(&ca), "401", &proxy, false).await?;
    assert_eq!(valid.calls().await?, 1);
    client(&valid.origin, None, "503", &proxy, false).await?;
    client(
        &valid.origin,
        Some(&root.join("wrong.pem")),
        "503",
        &proxy,
        false,
    )
    .await?;
    assert_eq!(valid.calls().await?, 1);
    for name in ["wrong", "expired"] {
        let mut server = Server::start(&root, name, None).await?;
        client(
            &server.origin,
            Some(&root.join(format!("{name}.pem"))),
            "503",
            &proxy,
            false,
        )
        .await?;
        assert_eq!(server.calls().await?, 0);
        server.stop().await?;
    }
    let mut redirect = Server::start(&root, "valid", Some(&trap.origin)).await?;
    client(&redirect.origin, Some(&ca), "503", &proxy, false).await?;
    assert_eq!(redirect.calls().await?, 1);
    assert_eq!(trap.calls().await?, 0);
    for (name, bytes, mode) in [
        ("malformed", &b"not PEM"[..], 0o600),
        ("empty", &b""[..], 0o600),
        ("oversized", &vec![b'x'; 65537][..], 0o600),
        ("unprotected", &fs::read(&ca)?[..], 0o644),
    ] {
        let path = root.join(name);
        fs::write(&path, bytes)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(mode))?;
        client(&valid.origin, Some(&path), "startup", &proxy, false).await?;
    }
    client(
        &valid.origin,
        Some(&root.join("missing")),
        "startup",
        &proxy,
        false,
    )
    .await?;
    client(
        &valid.origin,
        Some(Path::new("relative.pem")),
        "startup",
        &proxy,
        false,
    )
    .await?;
    let link = root.join("link");
    std::os::unix::fs::symlink(&ca, &link)?;
    client(&valid.origin, Some(&link), "startup", &proxy, false).await?;
    let ancestor = root.join("ancestor");
    std::os::unix::fs::symlink(&root, &ancestor)?;
    client(
        &valid.origin,
        Some(&ancestor.join("valid.pem")),
        "startup",
        &proxy,
        false,
    )
    .await?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o755))?;
    client(&valid.origin, Some(&ca), "startup", &proxy, false).await?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    client(
        "http://nonloopback.invalid/",
        None,
        "startup",
        &proxy,
        false,
    )
    .await?;
    client(&valid.origin, Some(&ca), "401", &proxy, true).await?;
    assert_eq!(valid.calls().await?, 2);
    assert_eq!(trap.calls().await?, 0);
    redirect.stop().await?;
    valid.stop().await?;
    trap.stop().await?;
    temp.close()?;
    Ok(())
}
