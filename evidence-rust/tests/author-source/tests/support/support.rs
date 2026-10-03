use super::{
    oracle::{Account, Oracle},
    wire::{Surface, Wire, sign},
};
use axum::{Json, Router, routing::get};
use linalab_auth::{
    access_identity::{AccessIdentity, Verifier},
    access_migration, action_migration, github_migration, github_oauth, grant_migration,
    machine_migration, migration, portal, session_http, session_migration,
};
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use sea_orm_migration::MigratorTrait;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc, time::Duration};
use time::OffsetDateTime;

pub type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
pub type Authority = AccessIdentity<Wire, fn() -> OffsetDateTime>;
pub struct Fixture {
    pub db: DatabaseConnection,
    root: DatabaseConnection,
    name: String,
    public: Surface,
    pub private: Surface,
    missing: Surface,
    jwks: Surface,
    pub oracle: Oracle,
    pub authority: Arc<Authority>,
    tokens: BTreeMap<String, String>,
    csrf: BTreeMap<String, String>,
    pub users: BTreeMap<String, String>,
    pub client: reqwest::Client,
    key: std::path::PathBuf,
    service_files: [std::path::PathBuf; 2],
}
impl Fixture {
    pub fn public_origin(&self) -> &str {
        &self.public.origin
    }
    pub fn token(&self, who: &str) -> Result<&str> {
        Ok(self.tokens.get(who).ok_or("signed token")?)
    }
    pub async fn start() -> Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut url = qa_url()?;
        let root = connect(url.as_str()).await?;
        let name = format!(
            "evidence_author19_{}_{}",
            std::process::id(),
            OffsetDateTime::now_utc().unix_timestamp_nanos()
        );
        root.execute_unprepared(&format!("CREATE DATABASE {name}"))
            .await?;
        println!("{}", json!({"ownedDatabase":name,"phase":"created"}));
        url.set_path(&format!("/{name}"));
        let db = connect(url.as_str()).await?;
        migration::Migrator::up(&db, None).await?;
        grant_migration::Migrator::up(&db, None).await?;
        session_migration::Migrator::up(&db, None).await?;
        machine_migration::Migrator::up(&db, None).await?;
        access_migration::Migrator::up(&db, None).await?;
        action_migration::Migrator::up(&db, None).await?;
        github_migration::Migrator::up(&db, None).await?;
        github_migration::Migrator::up(&db, None).await?;
        let signed = sign()?;
        let jwks = Surface::start(Router::new().route(
            "/jwks",
            get({
                let keys = signed.jwks;
                move || async move { Json(keys) }
            }),
        ))
        .await?;
        let authority = Arc::new(AccessIdentity::new(
            db.clone(),
            Arc::new(Verifier::with_source(
                "action-qa",
                Wire(format!("{}/jwks", jwks.origin)),
            )?),
            OffsetDateTime::now_utc as fn() -> OffsetDateTime,
        ));
        let oracle = Oracle::start().await?;
        let key = std::env::temp_dir().join(format!("{name}.csrf"));
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&key)?
            .write_all(b"github-task8-isolated-independent-csrf-credential")?;
        let public = Surface::start(
            portal::github_router(
                db.clone(),
                authority.clone(),
                portal::CsrfKey::from_file(&key)?,
                Some(authority.clone()),
                Some(Arc::new(github_oauth::Client::isolated(&format!(
                    "{}/",
                    oracle.surface.origin
                ))?)),
            )
            .merge(session_http::continuation_router(
                db.clone(),
                authority.clone(),
                session_http::ContinuationKey::from_file(&key)?,
            ))
            .layer(axum::middleware::from_fn(crate::capture::capture)),
        )
        .await?;
        let (private, service_files) = crate::listener::start(&db, &authority, &key).await?;
        let missing = Surface::start(portal::github_router(
            db.clone(),
            authority.clone(),
            portal::CsrfKey::from_file(&key)?,
            None,
            None,
        ))
        .await?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut fixture = Self {
            db,
            root,
            name,
            public,
            private,
            missing,
            jwks,
            oracle,
            authority,
            tokens: signed.tokens,
            csrf: BTreeMap::new(),
            users: BTreeMap::new(),
            client,
            key,
            service_files,
        };
        fixture.observe().await?;
        Ok(fixture)
    }
    async fn observe(&mut self) -> Result {
        for who in ["admin", "owner", "other", "session"] {
            let value = self
                .request(who, "GET", "/api/portal/me", None, None, 200)
                .await?;
            self.csrf.insert(
                who.into(),
                value
                    .get("csrfToken")
                    .and_then(Value::as_str)
                    .ok_or("CSRF")?
                    .into(),
            );
            self.users.insert(
                who.into(),
                value
                    .pointer("/user/id")
                    .and_then(Value::as_str)
                    .ok_or("user")?
                    .into(),
            );
        }
        Ok(())
    }
    pub async fn request(
        &self,
        who: &str,
        method: &str,
        path: &str,
        body: Option<Value>,
        confirmation: Option<&str>,
        status: u16,
    ) -> Result<Value> {
        let method = reqwest::Method::from_bytes(method.as_bytes())?;
        let mut request = self
            .client
            .request(method.clone(), format!("{}{path}", self.public.origin))
            .header(
                "cf-access-jwt-assertion",
                self.tokens.get(who).ok_or("signed token")?,
            );
        if method != reqwest::Method::GET {
            request = request
                .header("origin", "https://auth.linalab.io")
                .header("x-csrf-token", self.csrf.get(who).ok_or("csrf")?);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(token) = confirmation {
            request = request.header("x-action-confirmation", token);
        }
        let response = request.send().await?;
        let actual = response.status().as_u16();
        assert_eq!(
            actual,
            status,
            "{method} {}",
            path.split('?').next().ok_or("path")?
        );
        assert_eq!(
            response
                .headers()
                .get("cache-control")
                .and_then(|v| v.to_str().ok()),
            Some("no-store")
        );
        println!(
            "{}",
            json!({"packet":{"principal":who,"method":method.as_str(),"path":path.split('?').next(),"status":actual}})
        );
        let bytes = response.bytes().await?;
        Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }
    pub async fn callback_url(
        &self,
        who: &str,
        version: i64,
        account: Account,
    ) -> Result<url::Url> {
        let started = self
            .request(
                who,
                "POST",
                "/api/identity/github/start",
                Some(json!({"expectedBindingVersion":version})),
                None,
                200,
            )
            .await?;
        let url = started
            .get("authorizationUrl")
            .and_then(Value::as_str)
            .ok_or("authorization")?;
        self.oracle.select(account).await;
        let response = self.client.get(url).send().await?;
        assert_eq!(response.status(), 307);
        Ok(url::Url::parse(
            response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .ok_or("callback")?,
        )?)
    }
    pub async fn callback(&self, who: &str, url: &url::Url, status: u16) -> Result<Value> {
        self.request(
            who,
            "GET",
            &format!("{}?{}", url.path(), url.query().ok_or("query")?),
            None,
            None,
            status,
        )
        .await
    }
    pub async fn proof(&self, who: &str, version: i64, id: i64, handle: &str) -> Result<Value> {
        let url = self
            .callback_url(who, version, Account::normal(id, handle))
            .await?;
        self.callback(who, &url, 200).await
    }
    pub async fn prepare(
        &self,
        who: &str,
        proof: Option<&str>,
        version: i64,
        status: u16,
    ) -> Result<Value> {
        let operation = proof.map_or(
            json!({"action":"github.unlink"}),
            |id| json!({"action":"github.bind","proof_id":id}),
        );
        self.request(
            who,
            "POST",
            "/api/portal/action-confirmations",
            Some(json!({"operation":operation,"expectedVersions":{"githubBinding":version}})),
            None,
            status,
        )
        .await
    }
    pub async fn confirm(
        &self,
        who: &str,
        proof: Option<&str>,
        version: i64,
        token: &str,
        status: u16,
    ) -> Result<Value> {
        let (method, body) = proof.map_or(
            (
                "DELETE",
                json!({"expectedVersions":{"githubBinding":version}}),
            ),
            |id| {
                (
                    "PUT",
                    json!({"proofId":id,"expectedVersions":{"githubBinding":version}}),
                )
            },
        );
        self.request(
            who,
            method,
            "/api/identity/github",
            Some(body),
            Some(token),
            status,
        )
        .await
    }
    pub async fn cleanup(self) -> Result {
        self.public.stop().await?;
        self.private.stop().await?;
        self.missing.stop().await?;
        self.oracle.stop().await?;
        self.jwks.stop().await?;
        drop(self.authority);
        self.db.close().await?;
        self.root
            .execute_unprepared(&format!("DROP DATABASE {}", self.name))
            .await?;
        self.root.close().await?;
        std::fs::remove_file(self.key)?;
        for file in self.service_files {
            std::fs::remove_file(file)?;
        }
        println!(
            "{}",
            json!({"ownedDatabase":self.name,"phase":"removed","listeners":"closed","temporaryCsrf":"removed"})
        );
        Ok(())
    }
}
async fn connect(url: &str) -> Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(url);
    options
        .max_connections(8)
        .connect_timeout(Duration::from_secs(5))
        .acquire_timeout(Duration::from_secs(5))
        .sqlx_logging(false);
    Ok(Database::connect(options).await?)
}
pub fn qa_url() -> Result<url::Url> {
    use std::os::unix::fs::PermissionsExt;
    let path = std::env::var("AUTH_RUST_QA_FILE")
        .unwrap_or_else(|_| "/tmp/auth-rust-qa-20261002.env".into());
    if std::fs::metadata(&path)?.permissions().mode() & 0o077 != 0 {
        return Err("protected QA reference required".into());
    }
    let data = std::fs::read_to_string(path)?;
    let source = data
        .lines()
        .find_map(|line| line.strip_prefix("PG_QA_DATABASE_URL="))
        .ok_or("QA URL field")?;
    let mut url = url::Url::parse(source)?;
    let port =
        std::env::var("AUTH_RUST_QA_PORT").map_or(Ok(15440), |value| value.parse::<u16>())?;
    if !matches!(port, 15439 | 15440) {
        return Err("owned QA port required".into());
    }
    url.set_port(Some(port)).map_err(|()| "QA port")?;
    if url.host_str() != Some("127.0.0.1") || url.path() != "/auth_qa" {
        return Err("existing loopback QA reference required".into());
    }
    Ok(url)
}
