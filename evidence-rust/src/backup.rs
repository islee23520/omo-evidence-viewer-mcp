//! Consistent PostgreSQL snapshot plus verified immutable files. Restore is offline into an empty content DB.
use crate::{
    Error, Result,
    files::{Files, Manifest},
    storage::{Storage, statement},
};
use sea_orm::{ConnectionTrait, TransactionTrait};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

#[derive(Serialize, Deserialize)]
pub struct Backup {
    pub schema_version: u32,
    pub migration_sha256: String,
    pub tables: BTreeMap<String, Vec<Value>>,
    pub files: BTreeMap<String, Manifest>,
}
const TABLES: [&str; 4] = [
    "evidence_entry",
    "evidence_revision",
    "evidence_asset",
    "evidence_audit",
];

pub async fn create(storage: &Storage, destination: &Path) -> Result<String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::create_dir(destination)?;
    fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    let tx = storage.db.begin().await?;
    tx.execute_unprepared("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .await?;
    let mut tables = BTreeMap::new();
    for table in TABLES {
        let rows = tx
            .query_all_raw(statement(
                &format!("SELECT to_jsonb(t) AS record FROM evidence.{table} t"),
                vec![],
            ))
            .await?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.try_get::<Value>("", "record")?);
        }
        records.sort_by_key(Value::to_string);
        tables.insert(table.to_owned(), records);
    }
    let mut manifests = BTreeMap::new();
    for revision in tables.get("evidence_revision").ok_or(Error::Unavailable)? {
        let digest = revision
            .get("digest")
            .and_then(Value::as_str)
            .ok_or(Error::Unavailable)?;
        let manifest: Manifest =
            serde_json::from_value(revision.get("manifest").ok_or(Error::Unavailable)?.clone())
                .map_err(|_| Error::Unavailable)?;
        storage.files.verify(digest, &manifest)?;
        manifests.insert(digest.to_owned(), manifest);
    }
    let files_root = destination.join("content");
    fs::create_dir(&files_root)?;
    fs::set_permissions(&files_root, fs::Permissions::from_mode(0o700))?;
    let backup_files = Files::open(&files_root)?;
    for (digest, manifest) in &manifests {
        let mut stage = backup_files.stage()?;
        for asset in &manifest.assets {
            stage.add(
                asset.path.clone(),
                &storage.files.read(digest, &asset.path)?,
            )?;
        }
        let (id, restored, _) = backup_files.seal(stage)?;
        if restored != *digest {
            return Err(Error::Unavailable);
        }
        backup_files.record(id, "backup", Some(digest))?;
    }
    let backup = Backup {
        schema_version: 1,
        migration_sha256: crate::hash(include_bytes!("../migrations/001_content.sql")),
        tables,
        files: manifests,
    };
    let bytes = serde_json::to_vec(&backup).map_err(|_| Error::Unavailable)?;
    let digest = crate::hash(&bytes);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination.join("backup.json"))?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    let mut sum = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(destination.join("backup.sha256"))?;
    sum.write_all(digest.as_bytes())?;
    sum.sync_all()?;
    File::open(destination)?.sync_all()?;
    tx.commit().await?;
    Ok(digest)
}
pub async fn restore(storage: &Storage, source: &Path) -> Result<()> {
    let bytes = fs::read(source.join("backup.json"))?;
    if crate::hash(&bytes) != fs::read_to_string(source.join("backup.sha256"))? {
        return Err(Error::Invalid);
    }
    let backup: Backup = serde_json::from_slice(&bytes).map_err(|_| Error::Invalid)?;
    if backup.schema_version != 1
        || backup.migration_sha256 != crate::hash(include_bytes!("../migrations/001_content.sql"))
        || backup.tables.len() != 4
        || TABLES.iter().any(|name| !backup.tables.contains_key(*name))
    {
        return Err(Error::Invalid);
    }
    let source_files = Files::open(&source.join("content"))?;
    for (digest, manifest) in &backup.files {
        source_files.verify(digest, manifest)?;
    }
    for revision in backup
        .tables
        .get("evidence_revision")
        .ok_or(Error::Invalid)?
    {
        let digest = revision
            .get("digest")
            .and_then(Value::as_str)
            .ok_or(Error::Invalid)?;
        let manifest: Manifest =
            serde_json::from_value(revision.get("manifest").ok_or(Error::Invalid)?.clone())
                .map_err(|_| Error::Invalid)?;
        if backup.files.get(digest) != Some(&manifest) {
            return Err(Error::Invalid);
        }
    }
    let tx = storage.db.begin().await?;
    tx.execute_unprepared("LOCK TABLE evidence.evidence_entry,evidence.evidence_revision,evidence.evidence_asset,evidence.evidence_audit IN ACCESS EXCLUSIVE MODE").await?;
    for table in TABLES {
        let row = tx
            .query_one_raw(statement(
                &format!("SELECT count(*) AS n FROM evidence.{table}"),
                vec![],
            ))
            .await?
            .ok_or(Error::Unavailable)?;
        if row.try_get::<i64>("", "n")? != 0 {
            return Err(Error::Conflict);
        }
    }
    for (digest, manifest) in &backup.files {
        let mut stage = storage.files.stage()?;
        for asset in &manifest.assets {
            stage.add(asset.path.clone(), &source_files.read(digest, &asset.path)?)?;
        }
        let (id, restored, _) = storage.files.seal(stage)?;
        if restored != *digest {
            return Err(Error::Invalid);
        }
        storage.files.record(id, "restore_durable", Some(digest))?;
    }
    for table in TABLES {
        for original in backup.tables.get(table).ok_or(Error::Invalid)? {
            let mut row = original.clone();
            if table == "evidence_entry" {
                row.as_object_mut()
                    .ok_or(Error::Invalid)?
                    .insert("current_revision_id".into(), Value::Null);
            }
            tx.execute_raw(statement(&format!("INSERT INTO evidence.{table} SELECT * FROM jsonb_populate_record(NULL::evidence.{table},$1)"),vec![row.into()])).await?;
        }
    }
    for entry in backup.tables.get("evidence_entry").ok_or(Error::Invalid)? {
        tx.execute_raw(statement(
            "UPDATE evidence.evidence_entry SET current_revision_id=$1::uuid WHERE id=$2::uuid",
            vec![
                entry
                    .get("current_revision_id")
                    .and_then(Value::as_str)
                    .ok_or(Error::Invalid)?
                    .into(),
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or(Error::Invalid)?
                    .into(),
            ],
        ))
        .await?;
    }
    tx.commit().await?;
    Ok(())
}
pub fn linkage(backup: &Backup) -> Value {
    json!({"schemaVersion":backup.schema_version,"migrationSha256":backup.migration_sha256,"files":backup.files})
}
