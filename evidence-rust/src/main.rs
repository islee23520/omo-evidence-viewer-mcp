use omo_evidence_storage::{
    Error, Result,
    authority::Authority,
    backup,
    files::{Files, protected},
    http::{App, router},
    storage::Storage,
};
use sea_orm::{ConnectOptions, Database};
use serde::Deserialize;
use serde_json::Value;
use std::{
    env, fs,
    io::Read,
    path::{Path, PathBuf},
};

fn variable(name: &str) -> Result<String> {
    env::var(name).map_err(|_| Error::Invalid)
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Synthetic {
    synthetic: bool,
    slug: String,
    owner_user_id: Option<String>,
    metadata: Value,
    files: Vec<Source>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Source {
    path: String,
    sha256: String,
    bytes: u64,
}
#[tokio::main]
async fn main() {
    if run().await.is_err() {
        eprintln!("Evidence operation failed; no private input logged");
        std::process::exit(1);
    }
}
async fn run() -> Result<()> {
    let mut options = ConnectOptions::new(protected(Path::new(&variable(
        "EVIDENCE_DATABASE_URL_FILE",
    )?))?);
    options.sqlx_logging(false);
    let db = Database::connect(options).await?;
    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("migrate") {
        Storage::migrate(&db).await?;
        db.close().await?;
        println!("EVIDENCE_MIGRATED");
        return Ok(());
    }
    let files = Files::open(Path::new(&variable("EVIDENCE_CONTENT_ROOT")?))?;
    let storage = Storage { db, files };
    match args.as_slice() {
        [command, source] if command == "import-synthetic" => {
            let source = PathBuf::from(source);
            let bytes = fs::read(source.join("synthetic.json"))?;
            let input: Synthetic = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
            if !input.synthetic || input.files.is_empty() || input.files.len() > 200 {
                return Err(Error::Invalid);
            }
            let mut stage = storage.files.stage()?;
            let result = (|| {
                for file in &input.files {
                    if !omo_evidence_storage::files::safe_path(&file.path) {
                        return Err(Error::Invalid);
                    }
                    let path = source.join(&file.path);
                    for ancestor in path.ancestors() {
                        if fs::symlink_metadata(ancestor)?.file_type().is_symlink() {
                            return Err(Error::Invalid);
                        }
                    }
                    if !fs::symlink_metadata(&path)?.is_file() {
                        return Err(Error::Invalid);
                    }
                    if file.bytes > omo_evidence_storage::files::MAX_BYTES as u64
                        || file.bytes
                            > (omo_evidence_storage::files::MAX_BYTES - stage.bytes) as u64
                    {
                        return Err(Error::TooLarge);
                    }
                    let mut bytes = Vec::new();
                    fs::File::open(path)?
                        .take(file.bytes + 1)
                        .read_to_end(&mut bytes)?;
                    if bytes.len() as u64 != file.bytes
                        || omo_evidence_storage::hash(&bytes) != file.sha256
                    {
                        return Err(Error::Invalid);
                    }
                    stage.add(file.path.clone(), &bytes)?;
                }
                Ok(())
            })();
            if let Err(error) = result {
                storage.files.abort(stage)?;
                return Err(error);
            }
            let entry = storage
                .import(
                    &input.slug,
                    input.owner_user_id.as_deref(),
                    input.metadata,
                    stage,
                    &omo_evidence_storage::hash(&bytes),
                )
                .await?;
            println!(
                "{}",
                serde_json::json!({"slug":entry.slug,"revision":entry.revision,"digest":entry.digest,"provenance":"legacy_import_author_unknown"})
            );
        }
        [command, path] if command == "backup" => {
            println!(
                "EVIDENCE_BACKUP {}",
                backup::create(&storage, Path::new(path)).await?
            );
        }
        [command, path] if command == "restore" => {
            backup::restore(&storage, Path::new(path)).await?;
            println!("EVIDENCE_RESTORED");
        }
        [] => {
            let secret = protected(Path::new(&variable("AUTH_EVIDENCE_SERVICE_SECRET_FILE")?))?;
            let authority = Authority::new(
                variable("AUTH_PRIVATE_ORIGIN")?
                    .parse()
                    .map_err(|_| Error::Invalid)?,
                secret,
            )?;
            let origin = variable("REVIEW_ORIGIN")?;
            let bind = variable("EVIDENCE_BIND")?;
            let listener = tokio::net::TcpListener::bind(bind).await?;
            println!("EVIDENCE_LISTENING {}", listener.local_addr()?);
            let app = router(App {
                storage: storage.clone(),
                authority,
                origin,
            });
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    let _signal = tokio::signal::ctrl_c().await;
                })
                .await?;
        }
        _ => return Err(Error::Invalid),
    }
    storage.db.close().await?;
    Ok(())
}
