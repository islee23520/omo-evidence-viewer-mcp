//! Existing wire routes over immutable content and fresh service authority on every request.
use crate::{
    Error, Result,
    authority::{Authority, Principal},
    files::{Asset, MAX_BYTES, MAX_FILES},
    hash,
    storage::{Entry, Storage, valid_slug},
};
use axum::{
    Json, Router,
    body::Body,
    extract::{DefaultBodyLimit, FromRequest, Multipart, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use futures_util::TryStreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest;
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Clone)]
pub struct App {
    pub storage: Storage,
    pub authority: Authority,
    pub origin: String,
}
pub fn router(app: App) -> Router {
    Router::new()
        .fallback(handler)
        .layer(DefaultBodyLimit::disable())
        .with_state(Arc::new(app))
}
async fn handler(State(app): State<Arc<App>>, request: Request) -> Response {
    let method = request.method().to_string();
    let path = request.uri().path().to_owned();
    let headers = request.headers().clone();
    let query = request.uri().query().unwrap_or("").to_owned();
    let result = dispatch(&app, &method, &path, &query, &headers, request).await;
    let mut response = match result {
        Ok(response) => response,
        Err(error) => error.into_response(),
    };
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    if !response.headers().contains_key("cache-control") {
        response.headers_mut().insert(
            "cache-control",
            HeaderValue::from_static("private, no-store"),
        );
    }
    if method == "HEAD" {
        *response.body_mut() = Body::empty();
    }
    response
}
fn acl(entry: &Entry, principal: &Principal) -> Result<()> {
    if entry.owner.as_deref() != Some(principal.user.id.as_str()) && !principal.user.is_admin {
        return Err(Error::NotFound);
    }
    Ok(())
}
async fn authorized(
    app: &App,
    headers: &HeaderMap,
    method: &str,
    path: &str,
    scope: &str,
) -> Result<Principal> {
    app.authority.authorize(headers, method, path, scope).await
}
fn query(query: &str) -> BTreeMap<String, String> {
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect()
}
fn response(status: StatusCode, body: Vec<u8>, kind: &str) -> Result<Response> {
    Response::builder()
        .status(status)
        .header("content-type", kind)
        .body(Body::from(body))
        .map_err(|_| Error::Unavailable)
}
fn json_response(value: Value) -> Response {
    Json(value).into_response()
}
fn record(entry: &Entry) -> Value {
    let mut record = entry.metadata.clone();
    if let Some(object) = record.as_object_mut() {
        object.insert("visibility".into(), json!(entry.visibility));
        object.insert("slug".into(), json!(entry.slug));
        object.insert(
            "url".into(),
            json!(format!(
                "/{}/{}/",
                if entry.visibility == "public" {
                    "public"
                } else {
                    "evidence"
                },
                entry.slug
            )),
        );
    }
    record
}
async fn dispatch(
    app: &App,
    method: &str,
    path: &str,
    raw_query: &str,
    headers: &HeaderMap,
    request: Request,
) -> Result<Response> {
    if path == "/health" && matches!(method, "GET" | "HEAD") {
        return Ok(json_response(json!({"status":"ok"})));
    }
    if path == "/api/evidence" && method == "POST" {
        return upload(app, headers, request).await;
    }
    if path == "/" || (path == "/api/evidence" && matches!(method, "GET" | "HEAD")) {
        if !matches!(method, "GET" | "HEAD") {
            return response(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), "text/plain");
        }
        let principal = authorized(app, headers, method, path, "evidence:read").await?;
        let entries = app
            .storage
            .list()
            .await?
            .into_iter()
            .filter(|e| acl(e, &principal).is_ok())
            .collect::<Vec<_>>();
        if path == "/api/evidence" {
            return Ok(json_response(
                json!({"entries":entries.iter().map(record).collect::<Vec<_>>()}),
            ));
        }
        let mut groups = BTreeMap::<String, Vec<Entry>>::new();
        for entry in entries {
            groups
                .entry(
                    entry
                        .metadata
                        .get("repository")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .replace("https://github.com/", ""),
                )
                .or_default()
                .push(entry);
        }
        let mut sections = String::new();
        let mut groups = groups.into_iter().collect::<Vec<_>>();
        let created = |entry: &Entry| {
            entry
                .metadata
                .get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned()
        };
        groups.sort_by(|(a, av), (b, bv)| {
            bv.iter()
                .map(&created)
                .max()
                .cmp(&av.iter().map(&created).max())
                .then_with(|| a.cmp(b))
        });
        for (repository, mut entries) in groups {
            entries.sort_by(|a, b| {
                created(b).cmp(&created(a)).then_with(|| {
                    a.metadata
                        .get("title")
                        .and_then(Value::as_str)
                        .cmp(&b.metadata.get("title").and_then(Value::as_str))
                })
            });
            sections.push_str(&format!(
                "<section class=\"project\"><h2>{}</h2><ul>",
                escape(&repository)
            ));
            for entry in entries {
                let mut tags = String::new();
                if let Some(values) = entry.metadata.get("tags").and_then(Value::as_array) {
                    for tag in values {
                        let kind = tag.get("type").and_then(Value::as_str);
                        let value = tag.get("value").and_then(Value::as_str);
                        if let (Some(kind @ ("label" | "category")), Some(value)) = (kind, value) {
                            tags.push_str(&format!(
                                "<span class=\"tag {kind}\">{} · {}</span>",
                                if kind == "label" {
                                    "라벨"
                                } else {
                                    "카테고리"
                                },
                                escape(value)
                            ));
                        }
                    }
                }
                let tags = if tags.is_empty() {
                    tags
                } else {
                    format!("<div class=\"tags\">{tags}</div>")
                };
                sections.push_str(&format!(
                    "<li><div><a href=\"/evidence/{}/\">{}</a>{tags}</div><span>{} · Evidence</span></li>",
                    entry.slug,
                    escape(
                        entry
                            .metadata
                            .get("title")
                            .and_then(Value::as_str)
                            .unwrap_or(&entry.slug)
                    ),
                    escape(entry.metadata.get("date").and_then(Value::as_str).unwrap_or(""))
                ));
            }
            sections.push_str("</ul></section>");
        }
        let html = include_str!("../assets/index.html").replace("@@SECTIONS@@", &sections);
        return response(
            StatusCode::OK,
            html.into_bytes(),
            "text/html; charset=utf-8",
        );
    }
    if let Some(slug) = path
        .strip_prefix("/api/evidence/")
        .and_then(|p| p.strip_suffix("/visibility"))
    {
        if method != "PATCH" {
            return response(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), "text/plain");
        }
        if !valid_slug(slug, 240) {
            return Err(Error::Invalid);
        }
        let principal = authorized(app, headers, method, path, "evidence:publish").await?;
        if principal.lane != "browser" {
            return Err(Error::Authority(StatusCode::FORBIDDEN));
        }
        app.authority.mutation(headers, &app.origin, &principal)?;
        let entry = app.storage.get(slug).await?;
        acl(&entry, &principal)?;
        let input = bounded_json(request, 16384).await?;
        let visibility = input
            .get("visibility")
            .and_then(Value::as_str)
            .filter(|v| matches!(*v, "private" | "public"))
            .ok_or(Error::Invalid)?;
        let current = authorized(app, headers, method, path, "evidence:publish").await?;
        acl(&entry, &current)?;
        let updated = app
            .storage
            .visibility(&entry, &current.user.id, visibility)
            .await?;
        return Ok(json_response(record(&updated)));
    }
    if path == "/api/reviews" {
        return reviews(app, method, headers, &query(raw_query), request).await;
    }
    let (public, remainder) = if let Some(p) = path.strip_prefix("/public/") {
        (true, p)
    } else if let Some(p) = path.strip_prefix("/evidence/") {
        (false, p)
    } else {
        return Err(Error::NotFound);
    };
    if !matches!(method, "GET" | "HEAD") {
        return response(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), "text/plain");
    }
    let mut parts = Vec::new();
    for part in remainder.split('/').filter(|v| !v.is_empty()) {
        let part = percent_encoding::percent_decode_str(part)
            .decode_utf8()
            .map_err(|_| Error::NotFound)?
            .into_owned();
        if part.contains(['/', '\\']) || part.starts_with('.') {
            return Err(Error::NotFound);
        }
        parts.push(part);
    }
    let slug = parts.first().ok_or(Error::NotFound)?;
    if !valid_slug(slug, 240) {
        return Err(Error::NotFound);
    }
    // Current access is checked BEFORE resolving a file, ETag, Range or conditional response.
    let entry = app.storage.get(slug).await?;
    if public {
        if entry.visibility != "public" {
            return Err(Error::NotFound);
        }
    } else {
        let principal = authorized(app, headers, method, path, "evidence:read").await?;
        acl(&entry, &principal)?;
    }
    let asset_path = parts.iter().skip(1).cloned().collect::<Vec<_>>().join("/");
    let directory = asset_path.is_empty() || path.ends_with('/');
    let index = if asset_path.is_empty() {
        "index.html".to_owned()
    } else {
        format!("{asset_path}/index.html")
    };
    let asset_path = if directory && entry.manifest.assets.iter().any(|a| a.path == index) {
        index
    } else {
        asset_path
    };
    if directory && !entry.manifest.assets.iter().any(|a| a.path == asset_path) {
        let prefix = if asset_path.is_empty() {
            String::new()
        } else {
            format!("{asset_path}/")
        };
        let mut names = BTreeMap::new();
        for asset in &entry.manifest.assets {
            if let Some(child) = asset.path.strip_prefix(&prefix) {
                let (name, folder) = child
                    .split_once('/')
                    .map_or((child, false), |(name, _)| (name, true));
                names.insert(name.to_owned(), folder);
            }
        }
        if names.is_empty() {
            return Err(Error::NotFound);
        }
        let mut links = String::new();
        let mut media_items = Vec::new();
        for (name, folder) in names {
            let encoded =
                percent_encoding::utf8_percent_encode(&name, percent_encoding::NON_ALPHANUMERIC)
                    .to_string();
            let suffix = if folder { "/" } else { "" };
            links.push_str(&format!(
                "<li><a href=\"{encoded}{suffix}\">{}{suffix}</a></li>",
                escape(&name)
            ));
            if !folder {
                let kind = media(&name);
                let kind = if kind.starts_with("image/") {
                    Some("image")
                } else if kind.starts_with("video/") {
                    Some("video")
                } else if kind == "application/pdf" {
                    Some("pdf")
                } else {
                    None
                };
                if let Some(kind) = kind {
                    media_items.push(json!({"name":name,"href":encoded,"kind":kind}));
                }
            }
        }
        let title = asset_path
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(slug);
        let viewer = if media_items.is_empty() {
            String::new()
        } else {
            include_str!("../assets/media.html")
                .replace("@@TITLE@@", &escape(title))
                .replace(
                    "@@MEDIA@@",
                    &serde_json::to_string(&media_items)
                        .map_err(|_| Error::Unavailable)?
                        .replace('<', "\\u003c"),
                )
        };
        let html = include_str!("../assets/directory.html")
            .replace("@@TITLE@@", &escape(title))
            .replace("@@VIEWER@@", &viewer)
            .replace("@@LINKS@@", &links);
        let mut result = response(
            StatusCode::OK,
            html.into_bytes(),
            "text/html; charset=utf-8",
        )?;
        result.headers_mut().insert(
            "cache-control",
            HeaderValue::from_static(if public {
                "no-store"
            } else {
                "private, no-store"
            }),
        );
        return Ok(result);
    }
    let asset = entry
        .manifest
        .assets
        .iter()
        .find(|a| a.path == asset_path)
        .ok_or(Error::NotFound)?;
    let bytes = app.storage.files.read(&entry.digest, &asset.path)?;
    if hash(&bytes) != asset.sha256 || bytes.len() as u64 != asset.bytes {
        return Err(Error::Unavailable);
    }
    serve_asset(bytes, asset, headers, &query(raw_query), public)
}
pub async fn bounded_json(request: Request, limit: usize) -> Result<Value> {
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.try_next().await.map_err(|_| Error::Invalid)? {
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(Error::TooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Metadata {
    slug: String,
    title: String,
    repository: String,
    #[serde(default)]
    labels: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default = "private")]
    visibility: String,
}
fn private() -> String {
    "private".into()
}
fn validate(input: &Metadata) -> Result<()> {
    if !valid_slug(&input.slug, 80)
        || input.title.trim().is_empty()
        || input.title.chars().count() > 160
        || input.repository.len() > 300
        || !matches!(input.visibility.as_str(), "private" | "public")
        || [&input.labels, &input.categories].iter().any(|tags| {
            tags.len() > 20
                || tags
                    .iter()
                    .any(|t| t.trim().is_empty() || t.chars().count() > 40)
        })
    {
        return Err(Error::Invalid);
    }
    let regex =
        regex::Regex::new(r"(?i)^https://github\.com/[a-z0-9][a-z0-9-]*/[a-z0-9_.-]+(?:\.git)?/?$")
            .map_err(|_| Error::Unavailable)?;
    if !regex.is_match(&input.repository)
        || input
            .repository
            .rsplit('/')
            .next()
            .is_some_and(|p| matches!(p, "." | ".."))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
async fn upload(app: &App, headers: &HeaderMap, request: Request) -> Result<Response> {
    let principal = authorized(app, headers, "POST", "/api/evidence", "evidence:upload").await?;
    app.authority.mutation(headers, &app.origin, &principal)?;
    if headers
        .get("content-length")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<usize>().ok())
        .is_some_and(|n| n > MAX_BYTES)
    {
        return Err(Error::TooLarge);
    }
    // Count actual encoded body bytes as they arrive, including multipart overhead.
    let (parts, body) = request.into_parts();
    let mut total = 0_usize;
    let overflow = Arc::new(AtomicBool::new(false));
    let body_overflow = overflow.clone();
    let stream = body
        .into_data_stream()
        .map_err(|_| std::io::Error::other("invalid body"))
        .and_then(move |chunk| {
            let over = chunk.len() > MAX_BYTES.saturating_sub(total);
            total = total.saturating_add(chunk.len());
            let body_overflow = body_overflow.clone();
            async move {
                if over {
                    body_overflow.store(true, Ordering::Relaxed);
                    Err(std::io::Error::other("body too large"))
                } else {
                    Ok(chunk)
                }
            }
        });
    let request = Request::from_parts(parts, Body::from_stream(stream));
    let mut form = Multipart::from_request(request, &())
        .await
        .map_err(|_| Error::Invalid)?;
    let mut stage = app.storage.files.stage()?;
    let parsed = async {
        let mut metadata = None;
        let mut count = 0;
        while let Some(mut field) = form.next_field().await.map_err(|error| {
            if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
                Error::TooLarge
            } else {
                Error::Invalid
            }
        })? {
            match field.name() {
                Some("metadata") if metadata.is_none() => {
                    let mut bytes = Vec::new();
                    while let Some(chunk) = field.chunk().await.map_err(|_| Error::Invalid)? {
                        if chunk.len() > 16384_usize.saturating_sub(bytes.len()) {
                            return Err(Error::TooLarge);
                        }
                        bytes.extend_from_slice(&chunk);
                    }
                    let value: Metadata =
                        serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
                    validate(&value)?;
                    metadata = Some(value);
                }
                Some("files") => {
                    count += 1;
                    if count > MAX_FILES {
                        return Err(Error::Invalid);
                    }
                    let name = field.file_name().ok_or(Error::Invalid)?.to_owned();
                    let mut file = stage.file(&name)?;
                    let mut hash = sha2::Sha256::new();
                    let mut bytes = 0_u64;
                    while let Some(chunk) = field.chunk().await.map_err(|_| Error::Invalid)? {
                        if chunk.len() > MAX_BYTES.saturating_sub(stage.bytes) {
                            return Err(Error::TooLarge);
                        }
                        stage.bytes += chunk.len();
                        bytes += chunk.len() as u64;
                        hash.update(&chunk);
                        file.write_all(&chunk)?;
                    }
                    if bytes == 0 {
                        return Err(Error::Invalid);
                    }
                    file.sync_all()?;
                    stage.assets.push(Asset {
                        path: name,
                        sha256: format!("{:x}", hash.finalize()),
                        bytes,
                    });
                }
                _ => return Err(Error::Invalid),
            }
        }
        if count == 0 {
            return Err(Error::Invalid);
        }
        metadata.ok_or(Error::Invalid)
    }
    .await;
    let parsed = if overflow.load(Ordering::Relaxed) {
        Err(Error::TooLarge)
    } else {
        parsed
    };
    let result = match parsed {
        Err(error) => Err(error),
        Ok(input) => {
            if input.visibility == "public"
                && (principal.lane != "browser"
                    || !principal
                        .effective_scopes
                        .iter()
                        .any(|s| s == "evidence:publish"))
            {
                Err(Error::Authority(StatusCode::FORBIDDEN))
            } else {
                // The deployed private protocol does not return a verified author binding.
                // Do not invent fields, call the public PIN route, or accept an uploaded identity.
                let current =
                    authorized(app, headers, "POST", "/api/evidence", "evidence:upload").await;
                current.and_then(|_| Err(Error::AuthorRequired))
            }
        }
    };
    app.storage.files.abort(stage)?;
    result
}
async fn reviews(
    app: &App,
    method: &str,
    headers: &HeaderMap,
    query: &BTreeMap<String, String>,
    request: Request,
) -> Result<Response> {
    let slug = query
        .get("slug")
        .filter(|s| valid_slug(s, 240))
        .ok_or(Error::Invalid)?;
    if !matches!(method, "GET" | "HEAD" | "POST") {
        return response(StatusCode::METHOD_NOT_ALLOWED, Vec::new(), "text/plain");
    }
    let scope = if method == "POST" {
        "evidence:review"
    } else {
        "evidence:read"
    };
    let principal = authorized(app, headers, method, "/api/reviews", scope).await?;
    let entry = app.storage.get(slug).await?;
    acl(&entry, &principal)?;
    let asset = entry
        .manifest
        .assets
        .iter()
        .find(|a| a.path == "manifest.json")
        .ok_or(Error::NotFound)?;
    let bytes = app.storage.files.read(&entry.digest, "manifest.json")?;
    if hash(&bytes) != asset.sha256 {
        return Err(Error::Unavailable);
    }
    let manifest: Vec<Value> = serde_json::from_slice(&bytes).map_err(|_| Error::NotFound)?;
    if manifest.iter().any(|row| {
        !row.get("sha256")
            .and_then(Value::as_str)
            .is_some_and(valid_hash)
            || !row.get("name").is_some_and(Value::is_string)
    }) {
        return Err(Error::NotFound);
    }
    if method != "POST" {
        return Ok(json_response(app.storage.reviews(&entry).await?));
    }
    app.authority.mutation(headers, &app.origin, &principal)?;
    let input = bounded_json(request, 12000).await?;
    let sha = input
        .get("sha256")
        .and_then(Value::as_str)
        .ok_or(Error::Invalid)?;
    let row = manifest
        .iter()
        .find(|row| row.get("sha256").and_then(Value::as_str) == Some(sha))
        .ok_or(Error::Invalid)?;
    let verdict = input
        .get("verdict")
        .and_then(Value::as_str)
        .filter(|v| matches!(*v, "pass" | "fail" | "pending"))
        .ok_or(Error::Invalid)?;
    let note = input
        .get("note")
        .and_then(Value::as_str)
        .filter(|s| s.chars().count() <= 4000)
        .ok_or(Error::Invalid)?;
    let timestamp = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .map_err(|_| Error::Unavailable)?;
    let payload = json!({"sha256":sha,"decision":{"verdict":verdict,"note":note,"name":row.get("name"),"updatedAt":timestamp},"saved":true});
    let current = authorized(app, headers, method, "/api/reviews", scope).await?;
    acl(&entry, &current)?;
    app.storage
        .review(&entry, &current.user.id, payload.clone())
        .await?;
    Ok(json_response(payload))
}
fn valid_hash(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn media(path: &str) -> &'static str {
    match path
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "m4v" => "video/x-m4v",
        "pdf" => "application/pdf",
        "txt" | "md" | "markdown" => "text/plain; charset=utf-8",
        "csv" => "text/csv; charset=utf-8",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        _ => "application/octet-stream",
    }
}
fn serve_asset(
    mut bytes: Vec<u8>,
    asset: &Asset,
    headers: &HeaderMap,
    query: &BTreeMap<String, String>,
    public: bool,
) -> Result<Response> {
    let kind = media(&asset.path);
    let decorated = kind.starts_with("text/html")
        && query.get("raw").map(String::as_str) != Some("1")
        && !query.contains_key("download");
    if decorated {
        let html = String::from_utf8_lossy(&bytes);
        let link = "<a href=\"/\" data-gallery-home style=\"position:fixed;right:16px;bottom:16px;z-index:2147483647;padding:10px 16px;border-radius:24px;background:#282044;color:white;font:14px system-ui;text-decoration:none;box-shadow:0 2px 12px #0004\">\u{ac24}\u{b7ec}\u{b9ac} \u{d648}</a>";
        let regex = regex::Regex::new(r"(?i)<body([^>]*)>").map_err(|_| Error::Unavailable)?;
        bytes = if regex.is_match(&html) {
            regex
                .replacen(&html, 1, format!("<body$1>{link}"))
                .into_owned()
                .into_bytes()
        } else {
            format!("{html}{link}").into_bytes()
        };
    }
    let etag = format!("\"{}\"", hash(&bytes));
    let length = bytes.len();
    let mut status = StatusCode::OK;
    let mut content_range = None;
    if !decorated
        && headers
            .get("if-none-match")
            .and_then(|h| h.to_str().ok())
            .is_some_and(|v| v.split(',').any(|v| v.trim() == etag || v.trim() == "*"))
    {
        status = StatusCode::NOT_MODIFIED;
        bytes.clear();
    } else if !decorated
        && headers.contains_key("range")
        && headers
            .get("if-range")
            .and_then(|h| h.to_str().ok())
            .is_none_or(|v| v == etag)
    {
        let range = headers
            .get("range")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("bytes="))
            .and_then(|s| s.split_once('-'));
        let bounds = range.and_then(|(a, b)| {
            if a.is_empty() {
                let count = b.parse::<usize>().ok()?.min(length);
                if count == 0 {
                    return None;
                }
                Some((length - count, length.saturating_sub(1)))
            } else {
                let start = a.parse::<usize>().ok()?;
                let end = if b.is_empty() {
                    length.saturating_sub(1)
                } else {
                    b.parse::<usize>().ok()?.min(length.saturating_sub(1))
                };
                if start > end || start >= length {
                    return None;
                }
                Some((start, end))
            }
        });
        if let Some((start, end)) = bounds {
            bytes = bytes.get(start..=end).ok_or(Error::Unavailable)?.to_vec();
            status = StatusCode::PARTIAL_CONTENT;
            content_range = Some(format!("bytes {start}-{end}/{length}"));
        } else {
            status = StatusCode::RANGE_NOT_SATISFIABLE;
            bytes.clear();
            content_range = Some(format!("bytes */{length}"));
        }
    }
    let mut result = response(status, bytes, kind)?;
    result.headers_mut().insert(
        "cache-control",
        HeaderValue::from_static(if public {
            "no-store"
        } else {
            "private, no-store"
        }),
    );
    result.headers_mut().insert(
        "etag",
        HeaderValue::from_str(&etag).map_err(|_| Error::Unavailable)?,
    );
    result
        .headers_mut()
        .insert("accept-ranges", HeaderValue::from_static("bytes"));
    if let Some(value) = content_range {
        result.headers_mut().insert(
            "content-range",
            HeaderValue::from_str(&value).map_err(|_| Error::Unavailable)?,
        );
    }
    if query.contains_key("download") {
        let name = asset.path.rsplit('/').next().ok_or(Error::Invalid)?;
        result.headers_mut().insert(
            "content-disposition",
            HeaderValue::from_str(&format!(
                "attachment; filename*=UTF-8''{}",
                percent_encoding::utf8_percent_encode(name, percent_encoding::NON_ALPHANUMERIC)
            ))
            .map_err(|_| Error::Invalid)?,
        );
    }
    Ok(result)
}
