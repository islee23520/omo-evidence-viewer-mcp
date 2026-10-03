use crate::support::{Fixture, Result};
use linalab_auth::{
    account_identity::AccountAuthority,
    http::IdentityAuthority,
    machine_keys::{IssuedKey, Repository, TrustedWhoIs},
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use serde_json::{Value, json};

pub fn text<'a>(value: &'a Value, field: &str) -> Result<&'a str> {
    Ok(value
        .get(field)
        .and_then(Value::as_str)
        .ok_or("text field")?)
}
pub async fn session(f: &Fixture, service: &str) -> Result<String> {
    let nonce = "n".repeat(43);
    let send = |route: &str, body: Value| {
        f.client
            .post(format!("{}/internal/v1/{route}", f.private.origin))
            .bearer_auth(format!(
                "private-author-independent-{service}-transport-credential"
            ))
            .json(&body)
            .send()
    };
    let response = send(
        "login/start",
        json!({"service":service,"nonce":nonce,"returnPath":"/"}),
    )
    .await?;
    assert_eq!(response.status(), 200);
    let start: Value = response.json().await?;
    let transaction = text(&start, "transaction")?;
    let form = f
        .client
        .get(format!(
            "{}/continue?request={transaction}",
            f.public_origin()
        ))
        .header("cf-access-jwt-assertion", f.token("owner")?)
        .send()
        .await?
        .text()
        .await?;
    let field = |form: &str, name: &str| -> Result<String> {
        Ok(form
            .split(&format!("name=\"{name}\" value=\""))
            .nth(1)
            .ok_or("form field")?
            .split('"')
            .next()
            .ok_or("form value")?
            .into())
    };
    let csrf = field(&form, "csrf")?;
    let response = f
        .client
        .post(format!("{}/continue", f.public_origin()))
        .header("cf-access-jwt-assertion", f.token("owner")?)
        .header("origin", "https://auth.linalab.io")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("request={transaction}&csrf={csrf}"))
        .send()
        .await?;
    assert_eq!(response.status(), 200);
    let code = field(&response.text().await?, "code")?;
    let response = send(
        "login/exchange",
        json!({"service":service,"transaction":transaction,"code":code,"nonce":nonce}),
    )
    .await?;
    assert_eq!(response.status(), 200);
    Ok(text(&response.json::<Value>().await?, "handle")?.into())
}
pub async fn configure(f: &Fixture) -> Result<(String, Vec<IssuedKey>)> {
    let admin = f.users.get("admin").ok_or("admin")?;
    let row = f.db.query_one_raw(Statement::from_string(DbBackend::Postgres, format!("SELECT id FROM management.access_session_v1 WHERE user_id='{admin}' ORDER BY issued_at LIMIT 1"))).await?.ok_or("session")?;
    f.authority
        .bind_administrator(&row.try_get::<String>("", "id")?)
        .await?;
    let user = f.users.get("owner").ok_or("owner")?;
    for service in ["ci", "evidence"] {
        let current = linalab_auth::registry::get(&f.db, service).await?;
        let payload = json!({"origin":current.origin,"browserScopes":current.browser_scopes,"machineScopes":current.machine_scopes,"enabled":true});
        let prepared = f.request("admin","POST","/api/portal/action-confirmations",Some(json!({"operation":{"action":"service.update","target":service,"payload":payload},"expectedVersions":{"service":1}})),None,200).await?;
        f.request("admin","PUT",&format!("/api/admin/services/{service}"),Some(json!({"origin":current.origin,"browser_scopes":current.browser_scopes,"machine_scopes":current.machine_scopes,"enabled":true,"expectedVersion":1,"expectedVersions":{"service":1}})),Some(text(&prepared,"token")?),200).await?;
        let scopes = [
            format!("{service}:read"),
            if service == "evidence" {
                "evidence:upload".into()
            } else {
                "ci:build".into()
            },
        ];
        let browser_scopes = if service == "evidence" {
            vec![
                "evidence:read".to_owned(),
                "evidence:upload".to_owned(),
                "evidence:publish".to_owned(),
                "evidence:review".to_owned(),
            ]
        } else {
            scopes.to_vec()
        };
        let versions = json!({"service":2,"grant":0});
        let prepared = f.request("admin","POST","/api/portal/action-confirmations",Some(json!({"operation":{"action":"grant.put","target":{"userId":user,"serviceId":service},"payload":{"browserScopes":browser_scopes,"machineScopes":scopes}},"expectedVersions":versions})),None,200).await?;
        f.request("admin","PUT",&format!("/api/admin/users/{user}/grants/{service}"),Some(json!({"browserScopes":browser_scopes,"machineScopes":scopes,"expectedVersion":0,"expectedVersions":versions})),Some(text(&prepared,"token")?),200).await?;
    }
    let row =
        f.db.query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT id FROM management.access_session_v1 WHERE user_id='{user}'"),
        ))
        .await?
        .ok_or("owner session")?;
    let session: String = row.try_get("", "id")?;
    let actor = f.authority.session(&session).await.map_err(|_| "actor")?;
    let owner = AccountAuthority::account(f.authority.as_ref(), user)
        .await
        .map_err(|_| "owner")?;
    f.db.execute_unprepared(&format!("INSERT INTO management.machine_owner_binding VALUES ('author-tailnet','author-user','{user}','{}','fixture','unique-qa',true,1); INSERT INTO management.machine_node VALUES ('author-tailnet','author-node','author-user','unique-qa',true,1)",actor.subject)).await?;
    let who = TrustedWhoIs::from_whois_adapter("author-tailnet", "author-node", "author-user")?;
    let repo = Repository::new(&f.db, time::OffsetDateTime::now_utc);
    let enrollment = repo.start(&who, &owner).await?;
    let versions = json!({"enrollment":0,"binding":1,"node":1,"grant:ci":1,"service:ci":2,"grant:evidence":1,"service:evidence":2});
    let prepared = f.request("owner","POST","/api/portal/action-confirmations",Some(json!({"operation":{"action":"enrollment.approve","target":enrollment.public_id,"payload":{"services":["ci","evidence"]}},"expectedVersions":versions})),None,200).await?;
    f.request(
        "owner",
        "POST",
        &format!("/api/portal/enrollments/{}/approve", enrollment.public_id),
        Some(json!({"services":["ci","evidence"],"expectedVersions":versions})),
        Some(text(&prepared, "token")?),
        200,
    )
    .await?;
    Ok((
        session,
        repo.claim(&enrollment.challenge, &who, &owner).await?,
    ))
}
