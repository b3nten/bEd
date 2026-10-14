use super::*;
use std::path::Path;
pub fn seed_databases(root: &Path) -> io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(root)?;
    let mut paths = Vec::new();
    for (name, count) in [("first.sqlite", 625), ("second.sqlite", 3)] {
        let path = root.join(name);
        let mut connection = rusqlite::Connection::open(&path).map_err(io::Error::other)?;
        let transaction = connection.transaction().map_err(io::Error::other)?;
        transaction.execute_batch("CREATE TABLE IF NOT EXISTS categories(id INTEGER PRIMARY KEY, name TEXT); CREATE TABLE IF NOT EXISTS items(id INTEGER PRIMARY KEY, category INTEGER REFERENCES categories(id), name TEXT, note TEXT, payload BLOB); CREATE VIEW IF NOT EXISTS names AS SELECT id, name FROM items; DELETE FROM items; DELETE FROM categories; INSERT INTO categories VALUES(1, 'Things');").map_err(io::Error::other)?;
        for row in 1..=count {
            transaction
                .execute(
                    "INSERT INTO items VALUES (?1, 1, ?2, NULL, x'0001FF')",
                    rusqlite::params![row, format!("Item {row} · café")],
                )
                .map_err(io::Error::other)?;
        }
        transaction.commit().map_err(io::Error::other)?;
        paths.push(path);
    }
    Ok(paths)
}
