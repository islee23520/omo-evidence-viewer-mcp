use super::{support::Result, wire::Surface};
use axum::{
    Json, Router,
    extract::{Form, Query, State},
    http::{HeaderMap, StatusCode},
    response::Redirect,
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, sync::Arc};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct Account {
    pub id: i64,
    pub handle: String,
    pub scope: String,
    pub user_scope: String,
    pub wrong_pkce: bool,
}
impl Account {
    pub fn normal(id: i64, handle: &str) -> Self {
        Self {
            id,
            handle: handle.into(),
            scope: String::new(),
            user_scope: String::new(),
            wrong_pkce: false,
        }
    }
}
struct Code {
    challenge: String,
    account: Account,
}
#[derive(Default)]
struct Data {
    next: Option<Account>,
    codes: BTreeMap<String, Code>,
    tokens: BTreeMap<String, Account>,
    counter: u64,
    pub exchanges: u64,
}
#[derive(Clone)]
struct OracleState(Arc<Mutex<Data>>);
pub struct Oracle {
    pub surface: Surface,
    state: OracleState,
}
impl Oracle {
    pub async fn start() -> Result<Self> {
        let state = OracleState(Arc::new(Mutex::new(Data::default())));
        let surface = Surface::start(
            Router::new()
                .route("/login/oauth/authorize", get(authorize))
                .route("/login/oauth/access_token", post(exchange))
                .route("/user", get(user))
                .with_state(state.clone())
                .layer(axum::middleware::from_fn(crate::capture::capture)),
        )
        .await?;
        Ok(Self { surface, state })
    }
    pub async fn select(&self, account: Account) {
        self.state.0.lock().await.next = Some(account);
    }
    pub async fn stop(self) -> Result {
        self.surface.stop().await
    }
}
async fn authorize(
    State(state): State<OracleState>,
    Query(query): Query<BTreeMap<String, String>>,
) -> std::result::Result<Redirect, StatusCode> {
    let keys: Vec<_> = query.keys().map(String::as_str).collect();
    if keys
        != [
            "allow_signup",
            "client_id",
            "code_challenge",
            "code_challenge_method",
            "redirect_uri",
            "scope",
            "state",
        ]
        || query.get("client_id").map(String::as_str) != Some("github-link-synthetic-client")
        || query.get("redirect_uri").map(String::as_str)
            != Some(linalab_auth::github_oauth::CALLBACK)
        || query.get("scope").map(String::as_str) != Some("")
        || query.get("code_challenge_method").map(String::as_str) != Some("S256")
        || query.get("allow_signup").map(String::as_str) != Some("false")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let challenge = query.get("code_challenge").ok_or(StatusCode::BAD_REQUEST)?;
    if challenge.len() != 43 || URL_SAFE_NO_PAD.decode(challenge).is_err() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut data = state.0.lock().await;
    let account = data.next.take().ok_or(StatusCode::BAD_REQUEST)?;
    data.counter += 1;
    let code = format!("synthetic-code-{}", data.counter);
    let challenge = if account.wrong_pkce {
        "wrong-challenge".into()
    } else {
        challenge.clone()
    };
    data.codes.insert(code.clone(), Code { challenge, account });
    let mut callback = url::Url::parse(linalab_auth::github_oauth::CALLBACK)
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    callback
        .query_pairs_mut()
        .append_pair("code", &code)
        .append_pair("state", query.get("state").ok_or(StatusCode::BAD_REQUEST)?);
    Ok(Redirect::temporary(callback.as_str()))
}
async fn exchange(
    State(state): State<OracleState>,
    headers: HeaderMap,
    Form(form): Form<BTreeMap<String, String>>,
) -> std::result::Result<Json<Value>, StatusCode> {
    if headers.get("accept").and_then(|v| v.to_str().ok()) != Some("application/json")
        || form.keys().map(String::as_str).collect::<Vec<_>>()
            != [
                "client_id",
                "client_secret",
                "code",
                "code_verifier",
                "redirect_uri",
            ]
        || form.get("client_id").map(String::as_str) != Some("github-link-synthetic-client")
        || form.get("client_secret").map(String::as_str) != Some("github-link-synthetic-secret")
        || form.get("redirect_uri").map(String::as_str)
            != Some(linalab_auth::github_oauth::CALLBACK)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut data = state.0.lock().await;
    data.exchanges += 1;
    let code = data
        .codes
        .remove(form.get("code").ok_or(StatusCode::BAD_REQUEST)?)
        .ok_or(StatusCode::FORBIDDEN)?;
    let verifier = form.get("code_verifier").ok_or(StatusCode::BAD_REQUEST)?;
    if verifier.len() != 43
        || code.challenge != URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
    {
        return Ok(Json(json!({"error":"bad_verification_code"})));
    }
    let token = format!("synthetic-token-{}", data.counter);
    data.tokens.insert(token.clone(), code.account.clone());
    Ok(Json(
        json!({"access_token":token,"token_type":"bearer","scope":code.account.scope}),
    ))
}
async fn user(
    State(state): State<OracleState>,
    headers: HeaderMap,
) -> std::result::Result<(HeaderMap, Json<Value>), StatusCode> {
    if headers.get("accept").and_then(|v| v.to_str().ok()) != Some("application/vnd.github+json")
        || headers
            .get("x-github-api-version")
            .and_then(|v| v.to_str().ok())
            != Some("2026-03-10")
        || headers.get("user-agent").and_then(|v| v.to_str().ok())
            != Some("linalab-github-identity-link")
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let account = state
        .0
        .lock()
        .await
        .tokens
        .remove(token)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-oauth-scopes",
        account
            .user_scope
            .parse()
            .map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    Ok((
        headers,
        Json(
            json!({"id":account.id,"login":account.handle,"type":"User","email":"ignored@example.com","site_admin":true}),
        ),
    ))
}
