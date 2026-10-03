use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use serde_json::{Value, json};
use sha2::Digest;
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, path::Path};

pub async fn capture(request: Request, next: Next) -> Result<Response, StatusCode> {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let (parts, body) = request.into_parts();
    let input = to_bytes(body, 16384)
        .await
        .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    let response = next
        .run(Request::from_parts(parts, Body::from(input.clone())))
        .await;
    let (parts, body) = response.into_parts();
    let output = to_bytes(body, 65536)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let record = json!({"method":method,"path":path,"request":redact(&input),"status":parts.status.as_u16(),"response":redact(&output),"headers":parts.headers.iter().filter(|(name,_)|!matches!(name.as_str(),"set-cookie"|"location")).map(|(name,value)|(name.to_string(),value.to_str().unwrap_or("").to_owned())).collect::<Vec<_>>(),"requestSha256":format!("{:x}",sha2::Sha256::digest(&input)),"responseSha256":format!("{:x}",sha2::Sha256::digest(&output))});
    let archive = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut file = OpenOptions::new()
        .append(true)
        .create(true)
        .mode(0o600)
        .open(Path::new(&archive).join("authority-http.jsonl"))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    serde_json::to_writer(&mut file, &record).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    file.write_all(b"\n")
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    file.sync_all()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Response::from_parts(parts, Body::from(output)))
}
fn redact(bytes: &[u8]) -> Value {
    if let Ok(mut value) = serde_json::from_slice::<Value>(bytes) {
        scrub(&mut value);
        return value;
    }
    let text = String::from_utf8_lossy(bytes);
    if text.starts_with("<!doctype") {
        let mut result = text.into_owned();
        for name in ["request", "csrf", "code", "transaction"] {
            let marker = format!("name=\"{name}\" value=\"");
            if let Some(start) = result.find(&marker) {
                let start = start + marker.len();
                if let Some(end) = result.get(start..).and_then(|s| s.find('"')) {
                    result.replace_range(start..start + end, "[REDACTED]");
                }
            }
        }
        return json!(result);
    }
    if text.contains('=') && !text.starts_with('<') {
        return json!(
            url::form_urlencoded::parse(text.as_bytes())
                .map(|(name, _)| (name.into_owned(), "[REDACTED]".to_owned()))
                .collect::<std::collections::BTreeMap<_, _>>()
        );
    }
    json!(text)
}
fn scrub(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (name, value) in fields {
                if matches!(
                    name.as_str(),
                    "credential"
                        | "challenge"
                        | "rawKey"
                        | "nonce"
                        | "transaction"
                        | "code"
                        | "handle"
                        | "token"
                        | "csrfToken"
                        | "proofId"
                        | "proof_id"
                        | "authorizationUrl"
                        | "state"
                        | "code_verifier"
                        | "access_token"
                        | "client_secret"
                ) {
                    *value = json!("[REDACTED]");
                } else {
                    scrub(value);
                }
            }
        }
        Value::Array(values) => values.iter_mut().for_each(scrub),
        _ => {}
    }
}
