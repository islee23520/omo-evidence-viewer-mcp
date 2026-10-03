//! Independent private-service listener for the existing real OAuth fixture.
use crate::{
    support::{Authority, Result},
    wire::Surface,
};
use linalab_auth::session_http;
use sea_orm::DatabaseConnection;
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub async fn start(
    db: &DatabaseConnection,
    authority: &Arc<Authority>,
    key: &Path,
) -> Result<(Surface, [PathBuf; 2])> {
    let files = [key.with_extension("ci"), key.with_extension("evidence")];
    for (file, secret) in files.iter().zip([
        "private-author-independent-ci-transport-credential",
        "private-author-independent-evidence-transport-credential",
    ]) {
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(file)?
            .write_all(secret.as_bytes())?;
    }
    let listener = Surface::start(
        session_http::private_router(
            db.clone(),
            authority.clone(),
            session_http::ServiceCredentials::from_files(
                files.first().ok_or("CI file")?,
                files.get(1).ok_or("Evidence file")?,
            )?,
            authority.clone(),
        )
        .layer(axum::middleware::from_fn(crate::capture::capture)),
    )
    .await?;
    Ok((listener, files))
}
