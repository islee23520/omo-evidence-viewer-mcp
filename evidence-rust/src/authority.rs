//! Fixed private service authorization protocol. No caller identity header is trusted.
use crate::{Error, Result, now};
use axum::http::{HeaderMap, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{collections::HashSet, time::Duration};
use url::Url;

#[derive(Clone)]
pub struct Authority {
    origin: Url,
    secret: String,
    client: reqwest::Client,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: String,
    pub is_admin: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Principal {
    pub user: User,
    pub service: String,
    pub expires_at: i64,
    #[serde(rename = "principalType")]
    pub lane: String,
    pub effective_scopes: Vec<String>,
    pub scopes: Option<Vec<String>>,
    #[serde(deserialize_with = "nullable_author")]
    pub author_binding: Option<AuthorBinding>,
    pub machine_id: Option<String>,
    pub key_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthorBinding {
    pub github_user_id: i64,
    pub github_handle: String,
    pub binding_version: i64,
}
fn nullable_author<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<AuthorBinding>, D::Error> {
    let author = Option::<AuthorBinding>::deserialize(deserializer)?;
    if author.as_ref().is_some_and(|a| {
        a.github_user_id <= 0
            || a.binding_version <= 0
            || a.github_handle.is_empty()
            || a.github_handle.len() > 39
            || !a
                .github_handle
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    }) {
        return Err(serde::de::Error::custom("invalid verified author binding"));
    }
    Ok(author)
}
impl Principal {
    pub fn author(&self) -> Result<&AuthorBinding> {
        self.author_binding.as_ref().ok_or(Error::AuthorRequired)
    }
    pub fn same_submission(&self, current: &Self) -> Result<()> {
        if self.user.id != current.user.id
            || self.lane != current.lane
            || self.machine_id != current.machine_id
            || self.key_id != current.key_id
            || self.author()? != current.author()?
        {
            return Err(Error::Conflict);
        }
        Ok(())
    }
}

impl Authority {
    pub fn mutation(&self, headers: &HeaderMap, origin: &str, principal: &Principal) -> Result<()> {
        use base64::Engine;
        use hmac::Mac;
        if principal.lane == "machine" {
            return Ok(());
        }
        if headers.get_all("origin").iter().count() != 1
            || headers.get("origin").and_then(|v| v.to_str().ok()) != Some(origin)
            || headers.get_all("x-csrf-token").iter().count() != 1
        {
            return Err(Error::Authority(StatusCode::FORBIDDEN));
        }
        let (_, handle) = credential(headers)?;
        let supplied = headers
            .get("x-csrf-token")
            .and_then(|v| v.to_str().ok())
            .ok_or(Error::Authority(StatusCode::FORBIDDEN))?;
        let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(supplied)
            .map_err(|_| Error::Authority(StatusCode::FORBIDDEN))?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(self.secret.as_bytes())
            .map_err(|_| Error::Unavailable)?;
        mac.update(format!("csrf:evidence:{handle}").as_bytes());
        mac.verify_slice(&bytes)
            .map_err(|_| Error::Authority(StatusCode::FORBIDDEN))
    }
    pub fn new(origin: Url, secret: String) -> Result<Self> {
        if origin.path() != "/"
            || origin.query().is_some()
            || origin.fragment().is_some()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || !(origin.scheme() == "https"
                || (origin.scheme() == "http"
                    && matches!(origin.host_str(), Some("127.0.0.1" | "localhost"))))
            || secret.len() < 32
        {
            return Err(Error::Invalid);
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Error::Unavailable)?;
        Ok(Self {
            origin,
            secret,
            client,
        })
    }
    pub async fn authorize(
        &self,
        headers: &HeaderMap,
        method: &str,
        path: &str,
        scope: &str,
    ) -> Result<Principal> {
        let (kind, credential) = credential(headers)?;
        let mut response=self.client.post(self.origin.join("internal/v1/authorize").map_err(|_|Error::Invalid)?)
            .bearer_auth(&self.secret).json(&json!({"service":"evidence","credentialType":kind,"credential":credential,"method":method,"path":path}))
            .send().await.map_err(|_|Error::Authority(StatusCode::SERVICE_UNAVAILABLE))?;
        let status = response.status();
        if !status.is_success() {
            return Err(Error::Authority(match status.as_u16() {
                401 => StatusCode::UNAUTHORIZED,
                403 => StatusCode::FORBIDDEN,
                _ => StatusCode::SERVICE_UNAVAILABLE,
            }));
        }
        if status.as_u16() != 200 || response.content_length().is_some_and(|n| n > 16384) {
            return Err(Error::Authority(StatusCode::SERVICE_UNAVAILABLE));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| Error::Unavailable)? {
            if chunk.len() > 16384_usize.saturating_sub(bytes.len()) {
                return Err(Error::Unavailable);
            }
            bytes.extend_from_slice(&chunk);
        }
        let principal: Principal =
            serde_json::from_slice(&bytes).map_err(|_| Error::Unavailable)?;
        let scopes: HashSet<_> = principal.effective_scopes.iter().collect();
        if principal.service != "evidence"
            || principal.expires_at <= now()
            || principal.user.id.is_empty()
            || principal.lane
                != if kind == "machine-key" {
                    "machine"
                } else {
                    "browser"
                }
            || scopes.len() != principal.effective_scopes.len()
            || principal.effective_scopes.iter().any(|s| {
                !s.starts_with("evidence:")
                    || s.len() <= 9
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b":-".contains(&b))
            })
            || (principal.lane == "machine"
                && (principal.user.is_admin
                    || principal.machine_id.as_ref().is_none_or(String::is_empty)
                    || principal.key_id.as_ref().is_none_or(String::is_empty)
                    || principal.scopes.as_ref().is_none_or(|issued| {
                        principal.effective_scopes.iter().any(|s| {
                            !issued.contains(s)
                                || s.split(':')
                                    .any(|p| matches!(p, "admin" | "publish" | "browser"))
                        })
                    })))
        {
            return Err(Error::Unavailable);
        }
        if !principal.effective_scopes.iter().any(|s| s == scope) {
            return Err(Error::Authority(StatusCode::FORBIDDEN));
        }
        Ok(principal)
    }
}
fn credential(headers: &HeaderMap) -> Result<(&'static str, String)> {
    if headers.contains_key("authorization") {
        if headers.get_all("authorization").iter().count() != 1 {
            return Err(Error::Authority(StatusCode::UNAUTHORIZED));
        }
        let raw = headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(Error::Authority(StatusCode::UNAUTHORIZED))?;
        if !raw.starts_with("mk2_") || raw.bytes().any(|b| b <= 32 || b == 127) {
            return Err(Error::Authority(StatusCode::UNAUTHORIZED));
        }
        return Ok(("machine-key", raw.to_owned()));
    }
    let cookies: Vec<_> = headers
        .get_all("cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|v| v.trim().strip_prefix("__Host-linalab-evidence="))
        .collect();
    let handle = match cookies.as_slice() {
        [handle] => *handle,
        _ => return Err(Error::Authority(StatusCode::UNAUTHORIZED)),
    };
    if handle.len() != 43
        || !handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(Error::Authority(StatusCode::UNAUTHORIZED));
    }
    Ok(("service-session", handle.to_owned()))
}
