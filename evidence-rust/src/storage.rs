//! Pointer commit occurs only after immutable files are durable. No authority DDL.
use crate::{
    Error, Result,
    files::{Files, Manifest, Stage},
    now,
};
use sea_orm::{
    ConnectionTrait, DatabaseConnection, DbBackend, Statement, TransactionTrait, Value as SqlValue,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

#[derive(Clone)]
pub struct Storage {
    pub db: DatabaseConnection,
    pub files: Files,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub slug: String,
    pub owner: Option<String>,
    pub visibility: String,
    pub revision: String,
    pub digest: String,
    pub manifest: Manifest,
    pub metadata: Value,
    pub provenance: Value,
    pub version: i64,
}
pub fn statement(sql: &str, values: Vec<SqlValue>) -> Statement {
    Statement::from_sql_and_values(DbBackend::Postgres, sql, values)
}

impl Storage {
    pub async fn migrate(db: &DatabaseConnection) -> Result<()> {
        let tx = db.begin().await?;
        tx.execute_unprepared("SELECT pg_advisory_xact_lock(191920261003)")
            .await?;
        tx.execute_unprepared("CREATE SCHEMA IF NOT EXISTS evidence; CREATE TABLE IF NOT EXISTS evidence.migration_ledger(name text PRIMARY KEY,sha256 text NOT NULL)").await?;
        let source = include_str!("../migrations/001_content.sql");
        let digest = crate::hash(source.as_bytes());
        let applied = tx
            .query_one_raw(statement(
                "SELECT sha256 FROM evidence.migration_ledger WHERE name=$1",
                vec!["001_content".into()],
            ))
            .await?;
        if let Some(row) = applied {
            if row.try_get::<String>("", "sha256")? != digest {
                return Err(Error::Conflict);
            }
        } else {
            tx.execute_unprepared(source).await?;
            tx.execute_raw(statement(
                "INSERT INTO evidence.migration_ledger(name,sha256) VALUES($1,$2)",
                vec!["001_content".into(), digest.into()],
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
    pub async fn get(&self, slug: &str) -> Result<Entry> {
        let row=self.db.query_one_raw(statement("SELECT e.id::text,e.slug,e.owner_user_id,e.visibility,e.version,r.id::text AS revision,r.digest,r.manifest,r.metadata,r.provenance FROM evidence.evidence_entry e JOIN evidence.evidence_revision r ON r.id=e.current_revision_id WHERE e.slug=$1",vec![slug.into()])).await?.ok_or(Error::NotFound)?;
        Ok(Entry {
            id: row.try_get("", "id")?,
            slug: row.try_get("", "slug")?,
            owner: row.try_get("", "owner_user_id")?,
            visibility: row.try_get("", "visibility")?,
            version: row.try_get("", "version")?,
            revision: row.try_get("", "revision")?,
            digest: row.try_get("", "digest")?,
            manifest: serde_json::from_value(row.try_get("", "manifest")?)
                .map_err(|_| Error::Unavailable)?,
            metadata: row.try_get("", "metadata")?,
            provenance: row.try_get("", "provenance")?,
        })
    }
    pub async fn list(&self) -> Result<Vec<Entry>> {
        let rows = self
            .db
            .query_all_raw(statement(
                "SELECT slug FROM evidence.evidence_entry ORDER BY created_at DESC,slug",
                vec![],
            ))
            .await?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(self.get(&row.try_get::<String>("", "slug")?).await?);
        }
        Ok(entries)
    }
    /// Read-only legacy source imports carry explicit unknown authorship. The owner mapping is separately approved.
    pub async fn import(
        &self,
        slug: &str,
        owner: Option<&str>,
        metadata: Value,
        stage: Stage,
        source_digest: &str,
    ) -> Result<Entry> {
        if !valid_slug(slug, 240)
            || !matches!(
                metadata.get("visibility").and_then(Value::as_str),
                Some("private" | "public")
            )
        {
            self.files.abort(stage)?;
            return Err(Error::Invalid);
        }
        let prepared = self.files.seal(stage)?;
        let stage_id = prepared.id;
        let digest = prepared.digest.clone();
        let manifest = prepared.manifest.clone();
        let provenance =
            json!({"kind":"legacy_import","author":"unknown","sourceManifestSha256":source_digest});
        let id = Uuid::new_v4();
        let revision = Uuid::new_v4();
        let timestamp = now();
        let tx = self.db.begin().await?;
        let result=async {
            tx.execute_raw(statement("INSERT INTO evidence.evidence_entry(id,slug,owner_user_id,visibility,version,created_at,updated_at) VALUES($1::uuid,$2,$3,$4,1,$5,$5)",vec![id.to_string().into(),slug.into(),owner.map(str::to_owned).into(),metadata.get("visibility").and_then(Value::as_str).ok_or(Error::Invalid)?.into(),timestamp.into()])).await?;
            tx.execute_raw(statement("INSERT INTO evidence.evidence_revision(id,entry_id,digest,manifest,metadata,provenance,created_at) VALUES($1::uuid,$2::uuid,$3,$4,$5,$6,$7)",vec![revision.to_string().into(),id.to_string().into(),digest.clone().into(),json!(manifest).into(),metadata.clone().into(),provenance.into(),timestamp.into()])).await?;
            for asset in &manifest.assets {tx.execute_raw(statement("INSERT INTO evidence.evidence_asset(revision_id,path,sha256,bytes) VALUES($1::uuid,$2,$3,$4)",vec![revision.to_string().into(),asset.path.clone().into(),asset.sha256.clone().into(),i64::try_from(asset.bytes).map_err(|_|Error::Invalid)?.into()])).await?;}
            tx.execute_raw(statement("UPDATE evidence.evidence_entry SET current_revision_id=$1::uuid WHERE id=$2::uuid",vec![revision.to_string().into(),id.to_string().into()])).await?;
            tx.execute_raw(statement("INSERT INTO evidence.evidence_audit(id,entry_id,revision_id,action,payload,created_at) VALUES($1::uuid,$2::uuid,$3::uuid,'legacy.import',$4,$5)",vec![Uuid::new_v4().to_string().into(),id.to_string().into(),revision.to_string().into(),json!({"stage":stage_id,"digest":digest}).into(),timestamp.into()])).await?;
            Ok::<(),Error>(())
        }.await;
        if let Err(error) = result {
            tx.rollback().await?;
            self.files
                .record(stage_id, "db_failed_orphan", Some(&digest))?;
            return Err(error);
        }
        if tx.commit().await.is_err() {
            self.files
                .record(stage_id, "db_commit_unknown", Some(&digest))?;
            return Err(Error::Unavailable);
        }
        self.files.record(stage_id, "committed", Some(&digest))?;
        self.get(slug).await
    }
    pub async fn visibility(&self, entry: &Entry, actor: &str, visibility: &str) -> Result<Entry> {
        let tx = self.db.begin().await?;
        let result=tx.execute_raw(statement("UPDATE evidence.evidence_entry SET visibility=$1,version=version+1,updated_at=$2 WHERE id=$3::uuid AND version=$4 AND current_revision_id=$5::uuid",vec![visibility.into(),now().into(),entry.id.clone().into(),entry.version.into(),entry.revision.clone().into()])).await?;
        if result.rows_affected() != 1 {
            return Err(Error::Conflict);
        }
        tx.execute_raw(statement("INSERT INTO evidence.evidence_audit(id,entry_id,revision_id,actor_user_id,action,payload,created_at) VALUES($1::uuid,$2::uuid,$3::uuid,$4,'visibility.change',$5,$6)",vec![Uuid::new_v4().to_string().into(),entry.id.clone().into(),entry.revision.clone().into(),actor.into(),json!({"visibility":visibility,"previousVersion":entry.version}).into(),now().into()])).await?;
        tx.commit().await?;
        self.get(&entry.slug).await
    }
    pub async fn reviews(&self, entry: &Entry) -> Result<Value> {
        let rows=self.db.query_all_raw(statement("SELECT DISTINCT ON (payload->>'sha256') payload FROM evidence.evidence_audit WHERE entry_id=$1::uuid AND revision_id=$2::uuid AND action='review.save' ORDER BY payload->>'sha256',created_at DESC,id DESC",vec![entry.id.clone().into(),entry.revision.clone().into()])).await?;
        let mut decisions = serde_json::Map::new();
        for row in rows {
            let payload: Value = row.try_get("", "payload")?;
            decisions.insert(
                payload
                    .get("sha256")
                    .and_then(Value::as_str)
                    .ok_or(Error::Unavailable)?
                    .to_owned(),
                payload.get("decision").ok_or(Error::Unavailable)?.clone(),
            );
        }
        Ok(json!({"schemaVersion":1,"decisions":decisions}))
    }
    pub async fn review(&self, entry: &Entry, actor: &str, payload: Value) -> Result<()> {
        let tx = self.db.begin().await?;
        let row=tx.query_one_raw(statement("SELECT current_revision_id::text FROM evidence.evidence_entry WHERE id=$1::uuid FOR UPDATE",vec![entry.id.clone().into()])).await?.ok_or(Error::NotFound)?;
        if row.try_get::<String>("", "current_revision_id")? != entry.revision {
            return Err(Error::Conflict);
        }
        tx.execute_raw(statement("INSERT INTO evidence.evidence_audit(id,entry_id,revision_id,actor_user_id,action,payload,created_at) VALUES($1::uuid,$2::uuid,$3::uuid,$4,'review.save',$5,$6)",vec![Uuid::new_v4().to_string().into(),entry.id.clone().into(),entry.revision.clone().into(),actor.into(),payload.into(),now().into()])).await?;
        tx.commit().await?;
        Ok(())
    }
}
pub fn valid_slug(slug: &str, max: usize) -> bool {
    !slug.is_empty()
        && slug.len() <= max
        && slug.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}
