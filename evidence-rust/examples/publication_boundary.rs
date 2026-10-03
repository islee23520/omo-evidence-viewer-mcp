//! Owned QA process uses the real stage/seal protocol, not an alternate storage implementation.
use omo_evidence_storage::{Error, Result, files::Files};
use std::{
    io::{Read, Write},
    path::Path,
};

fn main() -> Result<()> {
    let root = std::env::var("EVIDENCE_CONTENT_ROOT").map_err(|_| Error::Invalid)?;
    let files = Files::open(Path::new(&root))?;
    let mut stage = files.stage()?;
    stage.add(
        "crash-proof.txt".into(),
        b"owned durable publication boundary",
    )?;
    let prepared = files.seal(stage)?;
    println!(
        "{}",
        serde_json::json!({"event":"publication_prepared","stage":prepared.id,"digest":prepared.digest,"manifest":prepared.manifest})
    );
    std::io::stdout().flush()?;
    // Explicit continuation, never a timing sleep. The parent kills this owned
    // process only after observing the durable immutable publication event.
    let mut byte = [0_u8; 1];
    std::io::stdin().read_exact(&mut byte)?;
    files.record(prepared.id, "qa_continued", Some(&prepared.digest))?;
    Ok(())
}
