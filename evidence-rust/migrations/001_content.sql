CREATE SCHEMA IF NOT EXISTS evidence;
CREATE TABLE evidence.evidence_entry (
    id uuid PRIMARY KEY,
    slug text NOT NULL UNIQUE,
    owner_user_id text,
    visibility text NOT NULL CHECK (visibility IN ('private','public')),
    current_revision_id uuid,
    version bigint NOT NULL CHECK (version > 0),
    created_at bigint NOT NULL,
    updated_at bigint NOT NULL
);
CREATE TABLE evidence.evidence_revision (
    id uuid PRIMARY KEY,
    entry_id uuid NOT NULL REFERENCES evidence.evidence_entry(id),
    digest text NOT NULL CHECK (digest ~ '^[a-f0-9]{64}$'),
    manifest jsonb NOT NULL,
    metadata jsonb NOT NULL,
    provenance jsonb NOT NULL,
    created_at bigint NOT NULL,
    UNIQUE (entry_id,id)
);
ALTER TABLE evidence.evidence_entry ADD CONSTRAINT current_revision_owned
    FOREIGN KEY (id,current_revision_id) REFERENCES evidence.evidence_revision(entry_id,id);
CREATE TABLE evidence.evidence_asset (
    revision_id uuid NOT NULL REFERENCES evidence.evidence_revision(id),
    path text NOT NULL,
    sha256 text NOT NULL CHECK (sha256 ~ '^[a-f0-9]{64}$'),
    bytes bigint NOT NULL CHECK (bytes > 0),
    PRIMARY KEY (revision_id,path)
);
CREATE TABLE evidence.evidence_audit (
    id uuid PRIMARY KEY,
    entry_id uuid REFERENCES evidence.evidence_entry(id),
    revision_id uuid REFERENCES evidence.evidence_revision(id),
    actor_user_id text,
    action text NOT NULL,
    payload jsonb NOT NULL,
    created_at bigint NOT NULL
);
CREATE INDEX evidence_review_read ON evidence.evidence_audit(entry_id,revision_id,created_at,id)
    WHERE action='review.save';
