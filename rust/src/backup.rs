//! Consistent SQLite snapshots, published only after validation and fsync.
use crate::schema::{self, Result};
use std::path::{Path, PathBuf};

/// Only these private, randomly named files are removed on failure. Never
/// overwrite an operator's destination, including a dangling symlink.
pub(crate) struct StagedDatabase {
    pub path: PathBuf,
    destination: PathBuf,
}

impl StagedDatabase {
    pub fn new(destination: &Path) -> Result<Self> {
        if destination.symlink_metadata().is_ok() {
            return Err("output already exists; refusing to overwrite it".into());
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .canonicalize()?;
        let name = destination
            .file_name()
            .ok_or("output must name a new database file")?;
        let path = parent.join(format!(
            ".telesink-staging-{}-{}.sqlite3",
            std::process::id(),
            rand::random::<u64>()
        ));
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        options.open(&path)?.sync_all()?;
        Ok(Self {
            path,
            destination: parent.join(name),
        })
    }

    pub fn publish(&self) -> Result<()> {
        std::fs::File::open(&self.path)?.sync_all()?;
        // A same-directory link atomically creates a new name without the
        // overwrite race that rename() would introduce.
        std::fs::hard_link(&self.path, &self.destination)?;
        std::fs::File::open(self.destination.parent().unwrap())?.sync_all()?;
        Ok(())
    }
}

impl Drop for StagedDatabase {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut path = self.path.as_os_str().to_owned();
            path.push(suffix);
            let _ = std::fs::remove_file(path);
        }
    }
}

pub async fn create(source: &str, destination: &Path) -> Result<()> {
    let staging = StagedDatabase::new(destination)?;
    let source = schema::connect_read_only(source).await?;
    let result = async {
        schema::check(&source).await?;
        // VACUUM INTO reads a consistent snapshot, including committed WAL
        // pages. It leaves the live source file and its writers untouched.
        sqlx::query("VACUUM INTO $1")
            .bind(staging.path.to_str().ok_or("backup path must be UTF-8")?)
            .execute(&source)
            .await?;
        let backup =
            schema::connect_read_only(&format!("sqlite://{}", staging.path.display())).await?;
        let checked = async {
            schema::check(&backup).await?;
            let integrity: Vec<String> = sqlx::query_scalar("PRAGMA integrity_check")
                .fetch_all(&backup)
                .await?;
            if integrity != ["ok"] {
                return Err("backup integrity check failed".into());
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        }
        .await;
        backup.close().await;
        checked?;
        staging.publish()
    }
    .await;
    source.close().await;
    result
}
