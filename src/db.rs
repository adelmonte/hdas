use anyhow::Result;
use rusqlite::{Connection, params};
use std::path::PathBuf;
use std::os::unix::fs::chown;

pub struct Database {
    conn: Connection,
}

pub fn get_user_info() -> (PathBuf, Option<u32>, Option<u32>) {
    if let Ok(sudo_user) = std::env::var("SUDO_USER") {
        if let Ok(passwd) = std::fs::read_to_string("/etc/passwd") {
            for line in passwd.lines() {
                let fields: Vec<&str> = line.split(':').collect();
                if fields.len() >= 6 && fields[0] == sudo_user {
                    let uid = fields[2].parse().ok();
                    let gid = fields[3].parse().ok();
                    return (PathBuf::from(fields[5]), uid, gid);
                }
            }
        }
    }
    match dirs::home_dir() {
        Some(home) => (home, None, None),
        None => {
            eprintln!("Warning: could not determine home directory, using /tmp");
            (PathBuf::from("/tmp"), None, None)
        }
    }
}

pub fn get_user_home() -> PathBuf {
    get_user_info().0
}

pub fn create_dir_all_with_owner(path: &std::path::Path, uid: Option<u32>, gid: Option<u32>) -> Result<()> {
    let mut to_create = Vec::new();
    let mut current = path.to_path_buf();

    while !current.exists() {
        to_create.push(current.clone());
        if let Some(parent) = current.parent() {
            current = parent.to_path_buf();
        } else {
            break;
        }
    }

    for dir in to_create.into_iter().rev() {
        std::fs::create_dir(&dir)?;
        if let (Some(u), Some(g)) = (uid, gid) {
            if let Err(e) = chown(&dir, Some(u), Some(g)) {
                eprintln!("Warning: failed to chown {}: {}", dir.display(), e);
            }
        }
    }

    Ok(())
}

impl Database {
    pub fn new() -> Result<Self> {
        let (home, uid, gid) = get_user_info();
        let mut db_dir = home;
        db_dir.push(".local/share/hdas");
        create_dir_all_with_owner(&db_dir, uid, gid)?;

        let db_path = db_dir.join("attributions.db");
        let conn = Connection::open(&db_path)?;
        Self::migrate(&conn)?;

        if let (Some(uid), Some(gid)) = (uid, gid) {
            if let Err(e) = chown(&db_path, Some(uid), Some(gid)) {
                eprintln!("Warning: failed to chown {}: {}", db_path.display(), e);
            }
        }

        Ok(Self { conn })
    }

    fn migrate(conn: &Connection) -> Result<()> {
        let has_new_schema: bool = conn
            .prepare("SELECT created_by_package FROM files LIMIT 1")
            .is_ok();

        if has_new_schema {
            return Ok(());
        }

        let old_table_exists: bool = conn
            .prepare("SELECT path FROM files LIMIT 1")
            .is_ok();

        if old_table_exists {
            conn.execute_batch(
                "
                CREATE TABLE files_new (
                    path TEXT PRIMARY KEY,
                    created_by_package TEXT,
                    created_by_process TEXT,
                    created_at INTEGER,
                    last_accessed_by_package TEXT,
                    last_accessed_by_process TEXT,
                    last_accessed_at INTEGER
                );

                INSERT INTO files_new (
                    path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
                )
                SELECT
                    path,
                    package, process, first_seen,
                    package, process, last_seen
                FROM files;

                DROP TABLE files;
                ALTER TABLE files_new RENAME TO files;

                CREATE INDEX idx_package ON files(created_by_package);
                CREATE INDEX idx_last_package ON files(last_accessed_by_package);
                "
            )?;
        } else {
            conn.execute(
                "CREATE TABLE IF NOT EXISTS files (
                    path TEXT PRIMARY KEY,
                    created_by_package TEXT,
                    created_by_process TEXT,
                    created_at INTEGER,
                    last_accessed_by_package TEXT,
                    last_accessed_by_process TEXT,
                    last_accessed_at INTEGER
                )",
                [],
            )?;

            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_package ON files(created_by_package)",
                [],
            )?;
            conn.execute(
                "CREATE INDEX IF NOT EXISTS idx_last_package ON files(last_accessed_by_package)",
                [],
            )?;
        }

        Ok(())
    }

    pub fn record_access(&self, path: &str, package: &str, process: &str, is_ignored: bool) -> Result<()> {
        let now = chrono::Utc::now().timestamp();

        if is_ignored {
            let exists: bool = self.conn.query_row(
                "SELECT 1 FROM files WHERE path = ?1",
                [path],
                |_| Ok(true)
            ).unwrap_or(false);

            if exists {
                self.conn.execute(
                    "UPDATE files SET
                        last_accessed_by_package = ?2,
                        last_accessed_by_process = ?3,
                        last_accessed_at = ?4
                     WHERE path = ?1",
                    params![path, package, process, now],
                )?;
            } else {
                self.conn.execute(
                    "INSERT INTO files (
                        path,
                        created_by_package, created_by_process, created_at,
                        last_accessed_by_package, last_accessed_by_process, last_accessed_at
                    ) VALUES (?1, 'unknown', ?3, ?4, ?2, ?3, ?4)",
                    params![path, package, process, now],
                )?;
            }
        } else {
            self.conn.execute(
                "INSERT INTO files (
                    path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
                ) VALUES (?1, ?2, ?3, ?4, ?2, ?3, ?4)
                ON CONFLICT(path) DO UPDATE SET
                    last_accessed_by_package = ?2,
                    last_accessed_by_process = ?3,
                    last_accessed_at = ?4,
                    created_by_package = CASE
                        WHEN created_by_package = 'unknown' THEN ?2
                        ELSE created_by_package
                    END,
                    created_by_process = CASE
                        WHEN created_by_package = 'unknown' THEN ?3
                        ELSE created_by_process
                    END,
                    created_at = CASE
                        WHEN created_by_package = 'unknown' THEN ?4
                        ELSE created_at
                    END",
                params![path, package, process, now],
            )?;
        }

        Ok(())
    }

    pub fn prune_deleted(&self) -> Result<usize> {
        let mut stmt = self.conn.prepare("SELECT path FROM files")?;
        let paths: Vec<String> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let mut pruned = 0;
        for path in paths {
            if !std::path::Path::new(&path).exists() {
                self.conn.execute("DELETE FROM files WHERE path = ?1", [&path])?;
                pruned += 1;
            }
        }

        Ok(pruned)
    }

    pub fn query_file(&self, pattern: &str) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
             FROM files WHERE path LIKE ?1"
        )?;

        let pattern = format!("%{}%", pattern);
        let records = stmt.query_map([pattern], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                created_by_package: row.get(1)?,
                created_by_process: row.get(2)?,
                created_at: row.get(3)?,
                last_accessed_by_package: row.get(4)?,
                last_accessed_by_process: row.get(5)?,
                last_accessed_at: row.get(6)?,
            })
        })?;

        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn query_package(&self, package: &str) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
             FROM files WHERE created_by_package = ?1 ORDER BY last_accessed_at DESC"
        )?;

        let records = stmt.query_map([package], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                created_by_package: row.get(1)?,
                created_by_process: row.get(2)?,
                created_at: row.get(3)?,
                last_accessed_by_package: row.get(4)?,
                last_accessed_by_process: row.get(5)?,
                last_accessed_at: row.get(6)?,
            })
        })?;

        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn query_directory(&self, dir: &str) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
             FROM files WHERE path LIKE ?1 ORDER BY path"
        )?;

        let pattern = format!("{}%", dir.trim_end_matches('/'));
        let records = stmt.query_map([pattern], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                created_by_package: row.get(1)?,
                created_by_process: row.get(2)?,
                created_at: row.get(3)?,
                last_accessed_by_package: row.get(4)?,
                last_accessed_by_process: row.get(5)?,
                last_accessed_at: row.get(6)?,
            })
        })?;

        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn list_all(&self) -> Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT path,
                    created_by_package, created_by_process, created_at,
                    last_accessed_by_package, last_accessed_by_process, last_accessed_at
             FROM files ORDER BY last_accessed_at DESC"
        )?;

        let records = stmt.query_map([], |row| {
            Ok(FileRecord {
                path: row.get(0)?,
                created_by_package: row.get(1)?,
                created_by_process: row.get(2)?,
                created_at: row.get(3)?,
                last_accessed_by_package: row.get(4)?,
                last_accessed_by_process: row.get(5)?,
                last_accessed_at: row.get(6)?,
            })
        })?;

        records.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_stats(&self) -> Result<(usize, usize, String)> {
        let file_count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM files", [], |row| row.get(0)
        )?;

        let package_count: usize = self.conn.query_row(
            "SELECT COUNT(DISTINCT created_by_package) FROM files", [], |row| row.get(0)
        )?;

        let db_path = get_user_home().join(".local/share/hdas/attributions.db");
        let db_location = db_path.to_string_lossy().to_string();

        Ok((file_count, package_count, db_location))
    }

    pub fn get_orphans(&self) -> Result<Vec<String>> {
        let output = std::process::Command::new("pacman")
            .arg("-Qq")
            .output()?;

        let installed: std::collections::HashSet<String> = String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect();

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT created_by_package FROM files WHERE created_by_package != 'unknown'"
        )?;

        let tracked: Vec<String> = stmt.query_map([], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(tracked.into_iter()
            .filter(|p| !installed.contains(p))
            .collect())
    }

    pub fn delete_file_records(&self, paths: &[String]) -> Result<usize> {
        let mut deleted = 0;
        for path in paths {
            deleted += self.conn.execute(
                "DELETE FROM files WHERE path = ?1",
                [path]
            )?;
        }
        Ok(deleted)
    }

    pub fn path_exists(&self, path: &str) -> bool {
        self.conn.query_row(
            "SELECT 1 FROM files WHERE path = ?1",
            [path],
            |_| Ok(true)
        ).unwrap_or(false)
    }

    pub fn path_has_known_creator(&self, path: &str) -> bool {
        self.conn.query_row(
            "SELECT 1 FROM files WHERE path = ?1 AND created_by_package != 'unknown'",
            [path],
            |_| Ok(true)
        ).unwrap_or(false)
    }
}

#[derive(Debug)]
pub struct FileRecord {
    pub path: String,
    pub created_by_package: String,
    pub created_by_process: String,
    pub created_at: i64,
    pub last_accessed_by_package: String,
    pub last_accessed_by_process: String,
    pub last_accessed_at: i64,
}
