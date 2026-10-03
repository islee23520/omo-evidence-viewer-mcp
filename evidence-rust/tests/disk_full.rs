//! A separately mounted, bounded owned filesystem supplies a real ENOSPC, never host-root exhaustion.
use omo_evidence_storage::files::Files;
use std::{
    fs,
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
#[test]
fn disk_full_preserves_owned_stage_and_ledger() -> Result {
    let mount = std::env::var("EVIDENCE_DISK_FULL_QA_MOUNT")?;
    let mount = Path::new(&mount);
    let root = mount.join("content");
    fs::create_dir(&root)?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
    let files = Files::open(&root)?;
    let mut stage = files.stage()?;
    let id = stage.id;
    let mut writer = stage.file("partial.bin")?;
    let filler = mount.join("owned-fill.bin");
    let mut fill = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&filler)?;
    let chunk = vec![0x5a_u8; 1024 * 1024];
    let mut written = 0_usize;
    let errno = loop {
        match fill.write_all(&chunk) {
            Ok(()) => {
                written += chunk.len();
                if written > 128 * 1024 * 1024 {
                    return Err("owned filesystem bound exceeded".into());
                }
            }
            Err(error) => break error.raw_os_error(),
        }
    };
    assert_eq!(errno, Some(28));
    let disk_error = writer
        .write_all(&vec![0x33_u8; 4 * 1024 * 1024])
        .err()
        .ok_or("real disk-full storage failure required")?;
    assert_eq!(disk_error.raw_os_error(), Some(28));
    drop(writer);
    drop(fill);
    fs::remove_file(&filler)?;
    files.abort(stage)?;
    assert_eq!(fs::read_dir(root.join("staging"))?.count(), 0);
    assert!(fs::read_to_string(root.join("ownership.jsonl"))?.contains(&id.to_string()));
    let archive = std::env::var("EVIDENCE_STORAGE_QA_ARCHIVE")?;
    let proof = serde_json::json!({"filesystemCapacityBoundBytes":128*1024*1024,"filledBytes":written,"fillErrno":errno,"storageWriteErrno":disk_error.raw_os_error(),"stage":id,"removed":true});
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(Path::new(&archive).join("disk-full.json"))?;
    serde_json::to_writer(&mut file, &proof)?;
    file.sync_all()?;
    fs::remove_dir_all(root)?;
    Ok(())
}
