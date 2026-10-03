# Rust content storage candidate

This crate owns only Evidence content. The operating Bun server and MCP consumers
are not replaced by this source candidate. It has four content tables and an
additive migration ledger; it never imports the management schema or identity data.
Task 23 owns the later explicit ACL and digest-bound publication fields.

The service rechecks the fixed private `/internal/v1/authorize` protocol for each
controlled request. The existing service cookie and machine key are credentials,
not author proof. Browser mutation CSRF uses the existing service-secret HMAC and
configured review Origin. Caller-supplied identity, scope, administrator and author
headers do not participate in authorization. Owner or current administrator is
required for a private entry; unmapped legacy entries are administrator-only.

## Authority blocker

Current authority source returns user ID, current administrator status, lane and
scopes, but no verified GitHub binding. Its public `/api/identity/github` route is
PIN-session-bound, not a private service identity protocol. New upload requests are
fully streamed and validated, then refused with `author_identity_required`, and
their owned staging directory is removed. They do not fabricate attribution.
The exact proposed authority-owned extension is in `docs/private-author-contract.md`.
Until that contract exists and real source-owned integration passes, this is not
Task 19 completion and cannot be activated as an upload replacement.

## Filesystem and PostgreSQL protocol

The exclusive stage UUID is fsynced in `ownership.jsonl` before creation. Asset
bytes are hashed while received; names have the existing Windows-safe validation,
case-folded conflict detection and 200-file cap. The entire encoded multipart body
is bounded to 100 MiB while streaming. Metadata is separately bounded to 16 KiB.
File data, each directory and the canonical immutable manifest are flushed before
the directory is renamed without replacement. The manifest hash addresses a
revision directory. PostgreSQL commits its current pointer only after that
directory is durable. The append-only ownership ledger retains orphan and uncertain
commit states. There is no claim of a cross-filesystem transaction and no automatic
GC, retention deletion or cleanup of another process's stage.

Each stage retains an exclusive lease through immutable preparation and pointer
commit. The fsynced ledger event is emitted only after durability, allowing an
operator to observe a precise publication boundary. Cancellation and streamed
disconnect clean only that live stage. `recover-orphans` reconciles existing
owned UUIDs, live leases and every retained DB revision without deleting files;
an inactive unreferenced revision remains an explicit orphan. Recovery never
turns lease expiry into authorization to replay publication or remove assets.

Raw and download bytes are checked against their actual asset hashes before
serving. Access and visibility are evaluated before ETag, conditional and Range
responses. Private responses use `private, no-store`; public responses use
`no-store`. A same-origin review appends a revision-bound decision to the content
audit transaction, preserving the legacy wire decision shape.

## Offline tools and restore

`migrate` requires the content owner role; normal service operation never runs DDL.
`import-synthetic <directory>` accepts only a reviewed synthetic copy containing
`synthetic.json` with `synthetic: true`, slug, approved owner mapping, metadata and
an exact path/hash/byte manifest. Originals are never rewritten. Legacy provenance
explicitly records unknown author attribution, not a made-up verified handle.

`backup <new-directory>` exports a repeatable-read PostgreSQL snapshot and every
immutable file referenced by every retained revision, with migration digest,
canonical linkage manifest and backup hash. `restore <directory>` verifies all
hashes first, locks an empty destination content database, prepares durable files,
then restores rows and current pointers transactionally. Restore must run offline
with separately owned database/files. It never overwrites a healthy populated DB.

Configuration uses protected files `EVIDENCE_DATABASE_URL_FILE` and
`AUTH_EVIDENCE_SERVICE_SECRET_FILE`, temporary isolated `EVIDENCE_CONTENT_ROOT`,
fixed `AUTH_PRIVATE_ORIGIN`, `REVIEW_ORIGIN`, and `EVIDENCE_BIND`. No credentials
are printed. Acceptance uses synthetic data only; no personal evidence root is
read or mounted.

## Isolated acceptance runner

`node evidence-rust/tests/qa/storage.mjs` requires `EVIDENCE_QA_AUTH_SOURCE`
pointing at the approved CI authority source checkout, `EVIDENCE_QA_REFERENCE`
pointing at a protected existing QA reference, and `EVIDENCE_STORAGE_QA_ARCHIVE`
pointing at a new private 0700 archive. It uses only a separately owned loopback
15440 forward prepared after read-only PG/relay inventory. It never starts or
alters the existing remote containers. It creates uniquely prefixed databases and
owner/runtime roles, runs the source authority and real binary, saves complete
response bytes and headers, and compares the original database catalog after
dropping its own resources. A ready `gateway_qa` source binary and the authority's
existing dependencies are prerequisites. The runner is intentionally explicit
about its legacy session fixture: it does not prove the current Access PIN author
integration. The operator must close the separately owned forward afterward.
`EVIDENCE_DISK_FULL_QA_MOUNT` must designate an exclusively owned bounded mounted
filesystem (the accepted Mac fixture is a new 64 MiB RAM disk). The disk test
fills only that filesystem to actual ENOSPC, exercises a failed stage write and
removes its own filler. It never fills the host volume. The operator detaches the
owned fixture after the crate-wide run. QA fixture grant preparation uses actual
source-resolved legacy principals; it never seeds a GitHub binding or claims
current PIN/author-ready acceptance.
