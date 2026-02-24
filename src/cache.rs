use crate::feeds::{Feed, FeedEntry};
use rusqlite::{params, Connection, Result};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct FeedCache {
  conn: Connection,
}

impl FeedCache {
  /// Create a new cache instance and initialize the database
  pub fn new(db_path: PathBuf) -> Result<Self> {
    let conn = Connection::open(db_path)?;
    Self::init_schema(&conn)?;
    Ok(Self { conn })
  }

  /// Initialize the database schema
  fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute(
      "CREATE TABLE IF NOT EXISTS feeds (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL UNIQUE,
        title TEXT NOT NULL,
        last_fetched INTEGER NOT NULL,
        tags TEXT,
        position INTEGER NOT NULL DEFAULT 0
      )",
      [],
    )?;

    conn.execute(
      "CREATE TABLE IF NOT EXISTS entries (
        id INTEGER PRIMARY KEY,
        feed_id INTEGER NOT NULL,
        title TEXT NOT NULL,
        published TEXT,
        text TEXT NOT NULL,
        links TEXT NOT NULL,
        media TEXT NOT NULL,
        read INTEGER NOT NULL DEFAULT 0,
        FOREIGN KEY(feed_id) REFERENCES feeds(id) ON DELETE CASCADE
      )",
      [],
    )?;

    // Migrate existing databases that may lack the read column
    let has_read_col: bool = conn
      .query_row(
        "SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name = 'read'",
        [],
        |row| row.get::<_, i64>(0),
      )
      .unwrap_or(0)
      > 0;

    if !has_read_col {
      conn.execute(
        "ALTER TABLE entries ADD COLUMN read INTEGER NOT NULL DEFAULT 0",
        [],
      )?;
    }

    conn.execute("CREATE INDEX IF NOT EXISTS idx_feed_url ON feeds(url)", [])?;

    // Unique index that drives incremental upserts in save_feed.
    // COALESCE maps NULL published dates to '' so the uniqueness check
    // works correctly even when published is absent.
    conn.execute(
      "CREATE UNIQUE INDEX IF NOT EXISTS idx_entry_unique
       ON entries(feed_id, title, COALESCE(published, ''))",
      [],
    )?;

    Ok(())
  }

  /// Save or update a feed and its entries incrementally.
  ///
  /// - The feed row itself is updated in-place (keeping its primary key so
  ///   the ON DELETE CASCADE on entries is never accidentally triggered).
  /// - Entries that already exist in the DB have their content refreshed but
  ///   their `read` flag is left untouched.
  /// - Entries that have aged out of the remote feed are kept in the DB so
  ///   the user never loses history or read state.
  /// - All entry upserts are wrapped in a single transaction so the operation
  ///   is both atomic and fast (one fsync instead of one per entry).
  pub fn save_feed(&self, feed: &Feed, position: usize) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let tags_json = feed
      .tags
      .as_ref()
      .map(|t| serde_json::to_string(t).unwrap_or_default());

    // Upsert the feed row and retrieve its id in a single round-trip using
    // RETURNING (available since SQLite 3.35.0, released March 2021).
    let feed_id: i64 = self.conn.query_row(
      "INSERT INTO feeds (url, title, last_fetched, tags, position)
       VALUES (?1, ?2, ?3, ?4, ?5)
       ON CONFLICT(url) DO UPDATE SET
         title        = excluded.title,
         last_fetched = excluded.last_fetched,
         tags         = excluded.tags,
         position     = excluded.position
       RETURNING id",
      params![feed.url, feed.title, now, tags_json, position as i64],
      |row| row.get(0),
    )?;

    // Wrap all entry upserts in one explicit transaction.
    // Without this each execute() starts and commits its own implicit
    // transaction, which requires a full fsync per entry — very slow for
    // feeds with many items (e.g. planet.emacslife.com).
    let tx = self.conn.unchecked_transaction()?;

    let mut stmt = tx.prepare_cached(
      "INSERT INTO entries (feed_id, title, published, text, links, media, read)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
       ON CONFLICT(feed_id, title, COALESCE(published, '')) DO UPDATE SET
         text  = excluded.text,
         links = excluded.links,
         media = excluded.media
         -- `read` is intentionally omitted: never reset on re-fetch",
    )?;

    for entry in &feed.entries {
      let links_json = serde_json::to_string(&entry.links).unwrap_or_default();
      let media_str = entry.media.as_deref().unwrap_or("");
      stmt.execute(params![
        feed_id,
        entry.title,
        entry.published,
        entry.text,
        links_json,
        media_str,
      ])?;
    }

    drop(stmt); // release borrow on tx before commit
    tx.commit()
  }

  /// Internal helper — sets `read = 1` or `read = 0` for a specific entry.
  fn set_entry_read(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
    read: bool,
  ) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET read = ?4
       WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)
         AND title = ?2
         AND (published = ?3 OR (published IS NULL AND ?3 IS NULL))",
      params![feed_url, entry_title, published, read as i64],
    )?;
    Ok(())
  }

  pub fn mark_entry_read(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.set_entry_read(feed_url, entry_title, published, true)
  }

  pub fn mark_entry_unread(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.set_entry_read(feed_url, entry_title, published, false)
  }

  /// Load all cached feeds ordered by position.
  ///
  /// The entry statement is prepared once outside the per-feed loop so the
  /// query is compiled only a single time regardless of how many feeds exist.
  pub fn load_all_feeds(&self) -> Result<Vec<Feed>> {
    let mut feed_stmt = self
      .conn
      .prepare("SELECT id, url, title, tags FROM feeds ORDER BY position")?;

    // Prepare the entry query once — reused for every feed in the loop below.
    let mut entry_stmt = self.conn.prepare(
      "SELECT title, published, text, links, media, read
       FROM entries
       WHERE feed_id = ?1
       ORDER BY published DESC",
    )?;

    let feed_data = feed_stmt
      .query_map([], |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, String>(2)?,
          row.get::<_, Option<String>>(3)?,
        ))
      })?
      .collect::<Result<Vec<_>>>()?;

    let mut feeds = Vec::with_capacity(feed_data.len());

    for (feed_id, url, title, tags_json) in feed_data {
      let tags = tags_json.and_then(|json| serde_json::from_str(&json).ok());

      let entries = entry_stmt
        .query_map(params![feed_id], |row| {
          let title: String = row.get(0)?;
          let published: Option<String> = row.get(1)?;
          let text: String = row.get(2)?;
          let links_json: String = row.get(3)?;
          let media_str: String = row.get(4)?;
          let read: i64 = row.get(5)?;

          let links: Vec<String> = serde_json::from_str(&links_json).unwrap_or_default();
          let media = if media_str.is_empty() {
            None
          } else {
            Some(media_str)
          };

          Ok(FeedEntry {
            title,
            published,
            text,
            links,
            media,
            feed_title: None,
            read: read != 0,
          })
        })?
        .collect::<Result<Vec<_>>>()?;

      feeds.push(Feed {
        url,
        title,
        entries,
        tags,
      });
    }

    Ok(feeds)
  }

  /// Check if a feed exists in cache
  pub fn has_feed(&self, url: &str) -> Result<bool> {
    let count: i64 = self.conn.query_row(
      "SELECT COUNT(*) FROM feeds WHERE url = ?1",
      params![url],
      |row| row.get(0),
    )?;
    Ok(count > 0)
  }

  /// Remove every feed from the cache whose URL is **not** present in
  /// `active_urls` (i.e. feeds that have been deleted from `feeds.toml`).
  ///
  /// Entries belonging to removed feeds are deleted automatically thanks to
  /// the `ON DELETE CASCADE` constraint defined in `init_schema`.
  ///
  /// Returns the number of feeds that were pruned.
  pub fn remove_dead_feeds(&self, active_urls: &[&str]) -> Result<usize> {
    // Foreign-key enforcement must be ON for ON DELETE CASCADE to fire.
    self.conn.execute("PRAGMA foreign_keys = ON", [])?;

    // Load every URL currently stored in the cache.
    let mut stmt = self.conn.prepare("SELECT url FROM feeds")?;
    let cached: Vec<String> = stmt
      .query_map([], |row| row.get(0))?
      .collect::<Result<Vec<_>>>()?;

    let active: HashSet<&str> = active_urls.iter().copied().collect();

    let mut removed = 0usize;
    for url in cached {
      if !active.contains(url.as_str()) {
        self
          .conn
          .execute("DELETE FROM feeds WHERE url = ?1", params![url])?;
        removed += 1;
      }
    }

    Ok(removed)
  }
}
