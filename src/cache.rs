use crate::feeds::{Feed, FeedEntry};
use rusqlite::{params, Connection, Result};
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

    // Create index for faster feed lookups
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
  /// - Entries that already exist in the DB are updated with fresh content
  ///   fields (text, links, media) but their `read` flag is left untouched.
  /// - Entries that have aged out of the remote feed are kept in the DB so
  ///   the user never loses history or read state.
  pub fn save_feed(&self, feed: &Feed, position: usize) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let tags_json = feed
      .tags
      .as_ref()
      .map(|t| serde_json::to_string(t).unwrap_or_default());

    // Update the feed row without replacing it.  INSERT OR REPLACE would
    // delete + re-insert, assigning a new primary key and cascade-deleting
    // every entry for this feed — exactly the bug we're fixing.
    self.conn.execute(
      "INSERT INTO feeds (url, title, last_fetched, tags, position)
       VALUES (?1, ?2, ?3, ?4, ?5)
       ON CONFLICT(url) DO UPDATE SET
         title        = excluded.title,
         last_fetched = excluded.last_fetched,
         tags         = excluded.tags,
         position     = excluded.position",
      params![feed.url, feed.title, now, tags_json, position as i64],
    )?;

    let feed_id: i64 = self.conn.query_row(
      "SELECT id FROM feeds WHERE url = ?1",
      params![feed.url],
      |row| row.get(0),
    )?;

    // Upsert each entry from the freshly fetched feed:
    //   • New entries are inserted as unread.
    //   • Existing entries (matched by feed_id + title + published) have their
    //     content refreshed but their `read` flag is never modified.
    for entry in &feed.entries {
      let links_json = serde_json::to_string(&entry.links).unwrap_or_default();
      self.conn.execute(
        "INSERT INTO entries (feed_id, title, published, text, links, media, read)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
         ON CONFLICT(feed_id, title, COALESCE(published, '')) DO UPDATE SET
           text  = excluded.text,
           links = excluded.links,
           media = excluded.media
           -- `read` is intentionally omitted: never reset on re-fetch",
        params![
          feed_id,
          entry.title,
          entry.published,
          entry.text,
          links_json,
          entry.media,
        ],
      )?;
    }

    Ok(())
  }

  /// Mark an entry as read by feed URL + title + published date
  pub fn mark_entry_read(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET read = 1
             WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)
               AND title = ?2
               AND (published = ?3 OR (published IS NULL AND ?3 IS NULL))",
      params![feed_url, entry_title, published],
    )?;
    Ok(())
  }

  pub fn mark_entry_unread(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET read = 0
             WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)
               AND title = ?2
               AND (published = ?3 OR (published IS NULL AND ?3 IS NULL))",
      params![feed_url, entry_title, published],
    )?;
    Ok(())
  }

  /// Load a feed from cache by URL
  pub fn load_feed(&self, url: &str) -> Result<Option<Feed>> {
    let feed_result: Result<(i64, String, String, Option<String>)> = self.conn.query_row(
      "SELECT id, title, url, tags FROM feeds WHERE url = ?1",
      params![url],
      |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    );

    let (feed_id, title, url, tags_json) = match feed_result {
      Ok(data) => data,
      Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
      Err(e) => return Err(e),
    };

    let tags = tags_json.and_then(|json| serde_json::from_str(&json).ok());

    let mut stmt = self.conn.prepare(
      "SELECT title, published, text, links, media, read
       FROM entries
       WHERE feed_id = ?1
       ORDER BY published DESC",
    )?;

    let entries = stmt
      .query_map(params![feed_id], |row| {
        let title: String = row.get(0)?;
        let published: Option<String> = row.get(1)?;
        let text: String = row.get(2)?;
        let links_json: String = row.get(3)?;
        let media: String = row.get(4)?;
        let read: i64 = row.get(5)?;

        let links: Vec<String> = serde_json::from_str(&links_json).unwrap_or_default();

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

    Ok(Some(Feed {
      url,
      title,
      entries,
      tags,
    }))
  }

  /// Load all cached feeds
  pub fn load_all_feeds(&self) -> Result<Vec<Feed>> {
    let mut stmt = self
      .conn
      .prepare("SELECT id, url, title, tags FROM feeds ORDER BY position")?;

    let feed_data = stmt
      .query_map([], |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, String>(2)?,
          row.get::<_, Option<String>>(3)?,
        ))
      })?
      .collect::<Result<Vec<_>>>()?;

    let mut feeds = Vec::new();

    for (feed_id, url, title, tags_json) in feed_data {
      let tags = tags_json.and_then(|json| serde_json::from_str(&json).ok());

      let mut entry_stmt = self.conn.prepare(
        "SELECT title, published, text, links, media, read
         FROM entries
         WHERE feed_id = ?1
         ORDER BY published DESC",
      )?;

      let entries = entry_stmt
        .query_map(params![feed_id], |row| {
          let title: String = row.get(0)?;
          let published: Option<String> = row.get(1)?;
          let text: String = row.get(2)?;
          let links_json: String = row.get(3)?;
          let media: String = row.get(4)?;
          let read: i64 = row.get(5)?;

          let links: Vec<String> = serde_json::from_str(&links_json).unwrap_or_default();

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

  /// Get the last fetch timestamp for a feed
  pub fn get_last_fetch(&self, url: &str) -> Result<Option<i64>> {
    let result: Result<i64> = self.conn.query_row(
      "SELECT last_fetched FROM feeds WHERE url = ?1",
      params![url],
      |row| row.get(0),
    );

    match result {
      Ok(timestamp) => Ok(Some(timestamp)),
      Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
      Err(e) => Err(e),
    }
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

  /// Delete a feed and its entries from cache
  pub fn delete_feed(&self, url: &str) -> Result<()> {
    self
      .conn
      .execute("DELETE FROM feeds WHERE url = ?1", params![url])?;
    Ok(())
  }

  /// Clear all cached data
  pub fn clear_all(&self) -> Result<()> {
    self.conn.execute("DELETE FROM entries", [])?;
    self.conn.execute("DELETE FROM feeds", [])?;
    Ok(())
  }
}
