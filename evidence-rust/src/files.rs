//! Durable immutable revision directories and an ownership ledger, not a filesystem transaction.
use crate::{Error, Result, hash, now};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub const MAX_BYTES: usize = 100 * 1024 * 1024;
pub const MAX_FILES: usize = 200;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Asset {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema_version: u32,
    pub assets: Vec<Asset>,
}
#[derive(Clone)]
pub struct Files {
    root: PathBuf,
    ledger: Arc<Mutex<File>>,
}
pub struct Stage {
    pub id: Uuid,
    pub directory: PathBuf,
    pub assets: Vec<Asset>,
    names: BTreeSet<String>,
    pub bytes: usize,
}

pub fn safe_path(path: &str) -> bool {
    path.len() <= 240
        && !path.is_empty()
        && path.split('/').all(|part| {
            let stem = part.split('.').next().unwrap_or("").to_ascii_lowercase();
            !part.is_empty()
                && !part.starts_with('.')
                && !part.ends_with(['.', ' '])
                && !part
                    .chars()
                    .any(|c| c.is_control() || "\\:<>\"|?*".contains(c))
                && !matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                && !(stem.len() == 4
                    && (stem.starts_with("com") || stem.starts_with("lpt"))
                    && stem
                        .as_bytes()
                        .last()
                        .is_some_and(|n| (b'1'..=b'9').contains(n)))
        })
}

pub fn protected(path: &Path) -> Result<String> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
        .open(path)?;
    let info = file.metadata()?;
    if !info.is_file() || info.permissions().mode() & 0o077 != 0 || info.len() > 4096 {
        return Err(Error::Invalid);
    }
    let mut value = String::new();
    file.read_to_string(&mut value)?;
    let value = value.trim();
    if value.is_empty() || value.bytes().any(|b| b <= 32 || b == 127) {
        return Err(Error::Invalid);
    }
    Ok(value.to_owned())
}

fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}
fn exclusive(path: &Path) -> Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}
fn no_links(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor)?.file_type().is_symlink() {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

impl Files {
    pub fn open(root: &Path) -> Result<Self> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        no_links(root)?;
        if !root.is_dir() || fs::metadata(root)?.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid);
        }
        for name in ["staging", "revisions"] {
            let dir = root.join(name);
            if !dir.exists() {
                fs::create_dir(&dir)?;
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
            }
            no_links(&dir)?;
        }
        let ledger = OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32)
            .open(root.join("ownership.jsonl"))?;
        if !ledger.metadata()?.is_file() || ledger.metadata()?.permissions().mode() & 0o077 != 0 {
            return Err(Error::Invalid);
        }
        sync_directory(root)?;
        Ok(Self {
            root: fs::canonicalize(root)?,
            ledger: Arc::new(Mutex::new(ledger)),
        })
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn record(&self, id: Uuid, state: &str, digest: Option<&str>) -> Result<()> {
        let mut ledger = self.ledger.lock().map_err(|_| Error::Unavailable)?;
        rustix::fs::flock(&*ledger, rustix::fs::FlockOperation::LockExclusive)
            .map_err(|_| Error::Unavailable)?;
        serde_json::to_writer(
            &mut *ledger,
            &serde_json::json!({"stage":id,"state":state,"digest":digest,"time":now()}),
        )
        .map_err(|_| Error::Unavailable)?;
        ledger.write_all(b"\n")?;
        ledger.sync_all()?;
        rustix::fs::flock(&*ledger, rustix::fs::FlockOperation::Unlock)
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    pub fn stage(&self) -> Result<Stage> {
        use std::os::unix::fs::PermissionsExt;
        let id = Uuid::new_v4();
        self.record(id, "allocated", None)?;
        let directory = self.root.join("staging").join(id.to_string());
        fs::create_dir(&directory)?;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        sync_directory(&self.root.join("staging"))?;
        Ok(Stage {
            id,
            directory,
            assets: Vec::new(),
            names: BTreeSet::new(),
            bytes: 0,
        })
    }
    pub fn abort(&self, stage: Stage) -> Result<()> {
        // Only the exclusive directory created by this live Stage can be removed.
        self.record(stage.id, "aborted", None)?;
        no_links(&stage.directory)?;
        fs::remove_dir_all(&stage.directory)?;
        sync_directory(&self.root.join("staging"))?;
        self.record(stage.id, "removed", None)
    }
    pub fn seal(&self, mut stage: Stage) -> Result<(Uuid, String, Manifest)> {
        use std::os::unix::fs::PermissionsExt;
        // Recheck actual durable files, not a caller's hash claims or cached buffers.
        for asset in &stage.assets {
            let path = stage.directory.join(&asset.path);
            no_links(&path)?;
            let bytes = fs::read(&path)?;
            if bytes.len() as u64 != asset.bytes || hash(&bytes) != asset.sha256 {
                return Err(Error::Invalid);
            }
        }
        stage.assets.sort_by(|a, b| a.path.cmp(&b.path));
        let manifest = Manifest {
            schema_version: 1,
            assets: stage.assets.clone(),
        };
        let bytes = serde_json::to_vec(&manifest).map_err(|_| Error::Invalid)?;
        let digest = hash(&bytes);
        let mut file = exclusive(&stage.directory.join(".manifest.json"))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fn freeze(path: &Path) -> Result<()> {
            for child in fs::read_dir(path)? {
                let child = child?;
                let info = child.file_type()?;
                if info.is_dir() {
                    freeze(&child.path())?;
                } else if info.is_file() {
                    fs::set_permissions(child.path(), fs::Permissions::from_mode(0o400))?;
                } else {
                    return Err(Error::Invalid);
                }
            }
            sync_directory(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o500))?;
            Ok(())
        }
        freeze(&stage.directory)?;
        // Darwin's exclusive directory rename requires owner write on its source root.
        // Asset bytes are already read-only and flushed; seal the root after publication.
        fs::set_permissions(&stage.directory, fs::Permissions::from_mode(0o700))?;
        self.record(stage.id, "durable", Some(&digest))?;
        let destination = self.root.join("revisions").join(&digest);
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            &stage.directory,
            rustix::fs::CWD,
            &destination,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => {}
            Err(rustix::io::Errno::EXIST) => {
                self.verify(&digest, &manifest)?;
                self.record(stage.id, "duplicate_orphan", Some(&digest))?;
            }
            Err(_) => return Err(Error::Unavailable),
        }
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o500))?;
        sync_directory(&self.root.join("revisions"))?;
        sync_directory(&self.root.join("staging"))?;
        self.record(stage.id, "immutable", Some(&digest))?;
        Ok((stage.id, digest, manifest))
    }
    pub fn read(&self, digest: &str, path: &str) -> Result<Vec<u8>> {
        if digest.len() != 64
            || !digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || !safe_path(path)
        {
            return Err(Error::NotFound);
        }
        let mut fd = rustix::fs::open(
            self.root.join("revisions"),
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::DIRECTORY
                | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| Error::NotFound)?;
        for component in std::iter::once(digest).chain(
            path.split('/')
                .take(path.split('/').count().saturating_sub(1)),
        ) {
            fd = rustix::fs::openat(
                &fd,
                component,
                rustix::fs::OFlags::RDONLY
                    | rustix::fs::OFlags::DIRECTORY
                    | rustix::fs::OFlags::NOFOLLOW,
                rustix::fs::Mode::empty(),
            )
            .map_err(|_| Error::NotFound)?;
        }
        let name = path.rsplit('/').next().ok_or(Error::NotFound)?;
        let fd = rustix::fs::openat(
            &fd,
            name,
            rustix::fs::OFlags::RDONLY | rustix::fs::OFlags::NOFOLLOW,
            rustix::fs::Mode::empty(),
        )
        .map_err(|_| Error::NotFound)?;
        let file = File::from(fd);
        if !file.metadata()?.is_file() {
            return Err(Error::NotFound);
        }
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        if bytes.len() > MAX_BYTES {
            return Err(Error::Unavailable);
        }
        Ok(bytes)
    }
    pub fn verify(&self, digest: &str, manifest: &Manifest) -> Result<()> {
        if hash(&serde_json::to_vec(manifest).map_err(|_| Error::Invalid)?) != digest {
            return Err(Error::Unavailable);
        }
        for asset in &manifest.assets {
            let bytes = self.read(digest, &asset.path)?;
            if bytes.len() as u64 != asset.bytes || hash(&bytes) != asset.sha256 {
                return Err(Error::Unavailable);
            }
        }
        Ok(())
    }
}
impl Stage {
    pub fn file(&mut self, name: &str) -> Result<File> {
        let folded = name.to_lowercase();
        if !safe_path(name)
            || self.names.contains(&folded)
            || self.names.iter().any(|other| {
                other.starts_with(&format!("{folded}/")) || folded.starts_with(&format!("{other}/"))
            })
        {
            return Err(Error::Invalid);
        }
        if self.names.len() >= MAX_FILES {
            return Err(Error::Invalid);
        }
        self.names.insert(folded);
        let path = self.directory.join(name);
        let parent = path.parent().ok_or(Error::Invalid)?;
        fs::create_dir_all(parent)?;
        no_links(parent)?;
        exclusive(&path)
    }
    pub fn add(&mut self, path: String, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Err(Error::Invalid);
        }
        if bytes.len() > MAX_BYTES.saturating_sub(self.bytes) {
            return Err(Error::TooLarge);
        }
        let mut file = self.file(&path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        self.bytes += bytes.len();
        self.assets.push(Asset {
            path,
            sha256: hash(bytes),
            bytes: bytes.len() as u64,
        });
        Ok(())
    }
}
