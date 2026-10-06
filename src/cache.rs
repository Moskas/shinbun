use crate::feeds::{Feed, FeedEntry, RemoteFeedRef};
use rusqlite::{params, Connection, OptionalExtension, Result};
use std::collections::HashSet;
use std::path::PathBuf;

pub struct CacheStats {
  pub feed_count: usize,
  pub entry_count: usize,
  pub read_count: usize,
  pub unread_count: usize,
}

pub struct FeedCache {
  conn: Connection,
}

impl FeedCache {
  /// Create a new cache instance and initialize the database
  pub fn new(db_path: PathBuf) -> Result<Self> {
    let conn = Connection::open(db_path)?;
    // Enable foreign key enforcement for ON DELETE CASCADE to work.
    conn.execute("PRAGMA foreign_keys = ON", [])?;
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
        position INTEGER NOT NULL DEFAULT 0,
        refresh_interval_secs INTEGER
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
        scroll_position INTEGER NOT NULL DEFAULT 0,
        deleted INTEGER NOT NULL DEFAULT 0,
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

    // Migrate existing databases that may lack the scroll_position column
    let has_scroll_col: bool = conn
      .query_row(
        "SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name = 'scroll_position'",
        [],
        |row| row.get::<_, i64>(0),
      )
      .unwrap_or(0)
      > 0;

    if !has_scroll_col {
      conn.execute(
        "ALTER TABLE entries ADD COLUMN scroll_position INTEGER NOT NULL DEFAULT 0",
        [],
      )?;
    }

    // Migrate existing databases that may lack the deleted column
    let has_deleted_col: bool = conn
      .query_row(
        "SELECT COUNT(*) FROM pragma_table_info('entries') WHERE name = 'deleted'",
        [],
        |row| row.get::<_, i64>(0),
      )
      .unwrap_or(0)
      > 0;

    if !has_deleted_col {
      conn.execute(
        "ALTER TABLE entries ADD COLUMN deleted INTEGER NOT NULL DEFAULT 0",
        [],
      )?;
    }

    // Migrate existing databases that may lack the refresh_interval_secs column
    let has_refresh_col: bool = conn
      .query_row(
        "SELECT COUNT(*) FROM pragma_table_info('feeds') WHERE name = 'refresh_interval_secs'",
        [],
        |row| row.get::<_, i64>(0),
      )
      .unwrap_or(0)
      > 0;

    if !has_refresh_col {
      conn.execute(
        "ALTER TABLE feeds ADD COLUMN refresh_interval_secs INTEGER",
        [],
      )?;
    }

    // Migrate existing databases that may lack the remote-backend columns.
    // `backend_id` holds the configured [[backends]] `id` this feed came
    // from (e.g. "home-miniflux"); NULL means a plain feeds.toml feed.
    // `remote_feed_id` is that backend's own id for the feed.
    for (table, column, decl) in [
      ("feeds", "backend_id", "TEXT"),
      ("feeds", "remote_feed_id", "TEXT"),
      ("entries", "remote_id", "TEXT"),
    ] {
      let has_col: bool = conn
        .query_row(
          &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"),
          [],
          |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;
      if !has_col {
        conn.execute(
          &format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"),
          [],
        )?;
      }
    }

    conn.execute("CREATE INDEX IF NOT EXISTS idx_feed_url ON feeds(url)", [])?;

    // Unique index that drives incremental upserts in save_feed for local
    // (non-remote-id-bearing) entries. COALESCE maps NULL published dates to
    // '' so the uniqueness check works correctly even when published is absent.
    conn.execute(
      "CREATE UNIQUE INDEX IF NOT EXISTS idx_entry_unique
       ON entries(feed_id, title, COALESCE(published, ''))",
      [],
    )?;

    // Separate unique index for remote-backend entries, keyed on the stable
    // remote id instead of title/published (which Miniflux etc. may not
    // give a reliable published date for, and which can change on the
    // remote side without the entry's identity changing).
    conn.execute(
      "CREATE UNIQUE INDEX IF NOT EXISTS idx_entry_remote
       ON entries(feed_id, remote_id) WHERE remote_id IS NOT NULL",
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
  /// - `refresh_interval_secs`: when `Some`, stored so startup can decide
  ///   whether the feed is due for re-fetching.
  pub fn save_feed(
    &self,
    feed: &Feed,
    position: usize,
    refresh_interval_secs: Option<u64>,
  ) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    let tags_json = feed
      .tags
      .as_ref()
      .map(|t| serde_json::to_string(t).unwrap_or_default());
    let interval = refresh_interval_secs.map(|s| s as i64);
    let backend_id = feed.backend.as_ref().map(|b| b.backend_id.as_str());
    let remote_feed_id = feed.backend.as_ref().map(|b| b.remote_feed_id.as_str());

    // Upsert the feed row and retrieve its id in a single round-trip using
    // RETURNING (available since SQLite 3.35.0, released March 2021).
    let feed_id: i64 = self.conn.query_row(
      "INSERT INTO feeds (url, title, last_fetched, tags, position, refresh_interval_secs, backend_id, remote_feed_id)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
       ON CONFLICT(url) DO UPDATE SET
         title                 = excluded.title,
         last_fetched          = excluded.last_fetched,
         tags                  = excluded.tags,
         position              = excluded.position,
         refresh_interval_secs = excluded.refresh_interval_secs,
         backend_id            = excluded.backend_id,
         remote_feed_id        = excluded.remote_feed_id
       RETURNING id",
      params![
        feed.url,
        feed.title,
        now,
        tags_json,
        position as i64,
        interval,
        backend_id,
        remote_feed_id,
      ],
      |row| row.get(0),
    )?;

    // Wrap all entry upserts in one explicit transaction.
    // Without this each execute() starts and commits its own implicit
    // transaction, which requires a full fsync per entry — very slow for
    // feeds with many items (e.g. planet.emacslife.com).
    let tx = self.conn.unchecked_transaction()?;

    // Remote-backend entries (have a stable `remote_id`) upsert on
    // (feed_id, remote_id) instead of (feed_id, title, published) — title or
    // published date can be edited/missing upstream without the entry's
    // underlying identity changing.
    let mut stmt_by_remote_id = tx.prepare_cached(
      "INSERT INTO entries (feed_id, title, published, text, links, media, remote_id, read, scroll_position)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0)
       ON CONFLICT(feed_id, remote_id) WHERE remote_id IS NOT NULL DO UPDATE SET
         title    = excluded.title,
         published = excluded.published,
         text     = excluded.text,
         links    = excluded.links,
         media    = excluded.media
         -- `read` and `scroll_position` are intentionally omitted: never reset on re-fetch",
    )?;

    let mut stmt_by_title = tx.prepare_cached(
      "INSERT INTO entries (feed_id, title, published, text, links, media, read, scroll_position)
       VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, 0)
       ON CONFLICT(feed_id, title, COALESCE(published, '')) DO UPDATE SET
         text  = excluded.text,
         links = excluded.links,
         media = excluded.media
         -- `read` and `scroll_position` are intentionally omitted: never reset on re-fetch",
    )?;

    for entry in &feed.entries {
      let links_json = serde_json::to_string(&entry.links).unwrap_or_default();
      let media_str = entry.media.as_deref().unwrap_or("");
      if let Some(remote_id) = entry.remote_id.as_deref() {
        stmt_by_remote_id.execute(params![
          feed_id,
          entry.title,
          entry.published,
          entry.text,
          links_json,
          media_str,
          remote_id,
        ])?;
      } else {
        stmt_by_title.execute(params![
          feed_id,
          entry.title,
          entry.published,
          entry.text,
          links_json,
          media_str,
        ])?;
      }
    }

    drop(stmt_by_remote_id);
    drop(stmt_by_title); // release borrows on tx before commit
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

  /// Mark every entry of a feed as read in a single statement.
  pub fn mark_feed_read(&self, feed_url: &str) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET read = 1
       WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)",
      params![feed_url],
    )?;
    Ok(())
  }

  pub fn mark_entry_unread(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.set_entry_read(feed_url, entry_title, published, false)
  }

  /// Soft-delete a single entry: marks it `deleted` rather than removing the
  /// row outright, so that re-fetching the feed (which upserts on the same
  /// feed_id + title + published key) never resurrects it. `load_all_feeds`
  /// and `get_stats` filter out `deleted = 1` rows.
  pub fn delete_entry(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
  ) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET deleted = 1
       WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)
         AND title = ?2
         AND (published = ?3 OR (published IS NULL AND ?3 IS NULL))",
      params![feed_url, entry_title, published],
    )?;
    Ok(())
  }

  /// Persist the scroll position (in rows) for a specific entry.
  pub fn set_entry_scroll_position(
    &self,
    feed_url: &str,
    entry_title: &str,
    published: Option<&str>,
    position: usize,
  ) -> Result<()> {
    self.conn.execute(
      "UPDATE entries SET scroll_position = ?4
       WHERE feed_id = (SELECT id FROM feeds WHERE url = ?1)
         AND title = ?2
         AND (published = ?3 OR (published IS NULL AND ?3 IS NULL))",
      params![feed_url, entry_title, published, position as i64],
    )?;
    Ok(())
  }

  /// Load all cached feeds ordered by position.
  ///
  /// The entry statement is prepared once outside the per-feed loop so the
  /// query is compiled only a single time regardless of how many feeds exist.
  pub fn load_all_feeds(&self) -> Result<Vec<Feed>> {
    let mut feed_stmt = self.conn.prepare(
      "SELECT id, url, title, tags, backend_id, remote_feed_id FROM feeds ORDER BY position",
    )?;

    // Prepare the entry query once — reused for every feed in the loop below.
    let mut entry_stmt = self.conn.prepare(
      "SELECT title, published, text, links, media, read, scroll_position, remote_id
       FROM entries
       WHERE feed_id = ?1 AND deleted = 0
       ORDER BY published DESC",
    )?;

    let feed_data = feed_stmt
      .query_map([], |row| {
        Ok((
          row.get::<_, i64>(0)?,
          row.get::<_, String>(1)?,
          row.get::<_, String>(2)?,
          row.get::<_, Option<String>>(3)?,
          row.get::<_, Option<String>>(4)?,
          row.get::<_, Option<String>>(5)?,
        ))
      })?
      .collect::<Result<Vec<_>>>()?;

    let mut feeds = Vec::with_capacity(feed_data.len());

    for (feed_id, url, title, tags_json, backend_id, remote_feed_id) in feed_data {
      let tags = tags_json.and_then(|json| serde_json::from_str(&json).ok());
      let backend = backend_id
        .zip(remote_feed_id)
        .map(|(backend_id, remote_feed_id)| RemoteFeedRef {
          backend_id,
          remote_feed_id,
        });

      let entries = entry_stmt
        .query_map(params![feed_id], |row| {
          let title: String = row.get(0)?;
          let published: Option<String> = row.get(1)?;
          let text: String = row.get(2)?;
          let links_json: String = row.get(3)?;
          let media_str: String = row.get(4)?;
          let read: i64 = row.get(5)?;
          let scroll_position: i64 = row.get(6)?;
          let remote_id: Option<String> = row.get(7)?;

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
            read: read != 0,
            scroll_position: scroll_position.max(0) as usize,
            remote_id,
            ..Default::default()
          })
        })?
        .collect::<Result<Vec<_>>>()?;

      feeds.push(Feed {
        url,
        title,
        entries,
        tags,
        backend,
      });
    }

    Ok(feeds)
  }

  /// Load a single feed by its URL.
  pub fn load_feed_by_url(&self, url: &str) -> Result<Option<Feed>> {
    let feed_row = self
      .conn
      .query_row(
        "SELECT id, url, title, tags, backend_id, remote_feed_id FROM feeds WHERE url = ?1",
        params![url],
        |row| {
          Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
          ))
        },
      )
      .optional()?;

    let Some((feed_id, url, title, tags_json, backend_id, remote_feed_id)) = feed_row else {
      return Ok(None);
    };

    let tags = tags_json.and_then(|json| serde_json::from_str(&json).ok());
    let backend = backend_id
      .zip(remote_feed_id)
      .map(|(backend_id, remote_feed_id)| RemoteFeedRef {
        backend_id,
        remote_feed_id,
      });

    let mut entry_stmt = self.conn.prepare(
      "SELECT title, published, text, links, media, read, scroll_position, remote_id
       FROM entries
       WHERE feed_id = ?1 AND deleted = 0
       ORDER BY published DESC",
    )?;

    let entries = entry_stmt
      .query_map(params![feed_id], |row| {
        let title: String = row.get(0)?;
        let published: Option<String> = row.get(1)?;
        let text: String = row.get(2)?;
        let links_json: String = row.get(3)?;
        let media_str: String = row.get(4)?;
        let read: i64 = row.get(5)?;
        let scroll_position: i64 = row.get(6)?;
        let remote_id: Option<String> = row.get(7)?;

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
          read: read != 0,
          scroll_position: scroll_position.max(0) as usize,
          remote_id,
          ..Default::default()
        })
      })?
      .collect::<Result<Vec<_>>>()?;

    Ok(Some(Feed {
      url,
      title,
      entries,
      tags,
      backend,
    }))
  }

  /// Return the unix timestamp of the last successful fetch for `url`,
  /// or `None` if the feed is not in the cache.
  pub fn get_last_fetched(&self, url: &str) -> Result<Option<i64>> {
    self
      .conn
      .query_row(
        "SELECT last_fetched FROM feeds WHERE url = ?1",
        params![url],
        |row| row.get(0),
      )
      .optional()
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

  /// Remove every **local** feed (i.e. `backend_id IS NULL`) from the cache
  /// whose URL is **not** present in `active_urls` (feeds that have been
  /// deleted from `feeds.toml`). Remote-backend feeds are left untouched —
  /// they're pruned separately by `remove_dead_backend_feeds`, since
  /// `feeds.toml` has no knowledge of them and would otherwise wipe every
  /// synced backend feed (and, if `feeds.toml` is empty, the entire cache)
  /// on every normal startup.
  ///
  /// Entries belonging to removed feeds are deleted automatically thanks to
  /// the `ON DELETE CASCADE` constraint defined in `init_schema`.
  ///
  /// Returns the number of feeds that were pruned.
  ///
  /// Uses a single `DELETE … WHERE url NOT IN (…)` statement instead of the
  /// previous SELECT-then-loop approach, eliminating the round-trip and all
  /// intermediate allocations.
  pub fn remove_dead_feeds(&self, active_urls: &[&str]) -> Result<usize> {
    if active_urls.is_empty() {
      // No active local feeds at all — drop every local feed, but keep
      // remote-backend ones.
      return self
        .conn
        .execute("DELETE FROM feeds WHERE backend_id IS NULL", []);
    }

    // Build: DELETE FROM feeds WHERE backend_id IS NULL AND url NOT IN (?1, ?2, …, ?N)
    // rusqlite doesn't support variable-length IN lists natively, so we
    // generate the placeholder string dynamically.  The input is trusted
    // (URLs from feeds.toml), but we still use bound parameters so that
    // URL strings with special chars are handled safely.
    let placeholders = (1..=active_urls.len())
      .map(|i| format!("?{i}"))
      .collect::<Vec<_>>()
      .join(", ");
    let sql = format!("DELETE FROM feeds WHERE backend_id IS NULL AND url NOT IN ({placeholders})");

    self
      .conn
      .execute(&sql, rusqlite::params_from_iter(active_urls))
  }

  /// Remove every feed belonging to the given configured backend instance
  /// (`backend_id`) whose remote feed id is **not** present in
  /// `active_remote_feed_ids` — i.e. a feed that was removed on the remote
  /// server. Scoped to one `backend_id` so one backend's sync can never
  /// prune another backend's (or a local feed's) rows.
  pub fn remove_dead_backend_feeds(
    &self,
    backend_id: &str,
    active_remote_feed_ids: &[&str],
  ) -> Result<usize> {
    if active_remote_feed_ids.is_empty() {
      return self.conn.execute(
        "DELETE FROM feeds WHERE backend_id = ?1",
        params![backend_id],
      );
    }

    let placeholders = (2..=active_remote_feed_ids.len() + 1)
      .map(|i| format!("?{i}"))
      .collect::<Vec<_>>()
      .join(", ");
    let sql =
      format!("DELETE FROM feeds WHERE backend_id = ?1 AND remote_feed_id NOT IN ({placeholders})");
    let mut all_params: Vec<&dyn rusqlite::ToSql> = vec![&backend_id];
    all_params.extend(
      active_remote_feed_ids
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql),
    );
    self.conn.execute(&sql, all_params.as_slice())
  }

  /// Return `(url, title)` for every **local** feed in the cache that is
  /// **not** present in `active_urls`. Does not modify the database.
  /// Remote-backend feeds are excluded — see `remove_dead_feeds` for why.
  pub fn list_dead_feeds(&self, active_urls: &[&str]) -> Result<Vec<(String, String)>> {
    let mut stmt = self
      .conn
      .prepare("SELECT url, title FROM feeds WHERE backend_id IS NULL")?;
    let active: HashSet<&str> = active_urls.iter().copied().collect();
    let dead = stmt
      .query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
      })?
      .filter_map(|r| r.ok())
      .filter(|(url, _)| !active.contains(url.as_str()))
      .collect();
    Ok(dead)
  }

  /// Aggregate statistics about the current cache contents.
  pub fn get_stats(&self) -> Result<CacheStats> {
    let feed_count: i64 = self
      .conn
      .query_row("SELECT COUNT(*) FROM feeds", [], |r| r.get(0))?;
    let entry_count: i64 =
      self
        .conn
        .query_row("SELECT COUNT(*) FROM entries WHERE deleted = 0", [], |r| {
          r.get(0)
        })?;
    let read_count: i64 = self.conn.query_row(
      "SELECT COUNT(*) FROM entries WHERE deleted = 0 AND read = 1",
      [],
      |r| r.get(0),
    )?;
    Ok(CacheStats {
      feed_count: feed_count as usize,
      entry_count: entry_count as usize,
      read_count: read_count as usize,
      unread_count: (entry_count - read_count) as usize,
    })
  }

  /// Create a cache backed by an in-memory SQLite database (for testing).
  #[cfg(test)]
  pub fn new_in_memory() -> Result<Self> {
    let conn = Connection::open_in_memory()?;
    conn.execute("PRAGMA foreign_keys = ON", [])?;
    Self::init_schema(&conn)?;
    Ok(Self { conn })
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn make_feed(url: &str, title: &str, entries: Vec<FeedEntry>) -> Feed {
    Feed {
      url: url.to_string(),
      title: title.to_string(),
      entries,
      tags: None,
      ..Default::default()
    }
  }

  fn make_entry(title: &str, published: Option<&str>) -> FeedEntry {
    FeedEntry {
      title: title.to_string(),
      published: published.map(|s| s.to_string()),
      text: format!("Content of {}", title),
      links: vec![format!("https://example.com/{}", title)],
      media: None,
      read: false,
      scroll_position: 0,
      ..Default::default()
    }
  }

  fn make_remote_entry(title: &str, remote_id: &str) -> FeedEntry {
    FeedEntry {
      title: title.to_string(),
      published: None,
      text: format!("Content of {}", title),
      links: vec![],
      media: None,
      read: false,
      scroll_position: 0,
      remote_id: Some(remote_id.to_string()),
      ..Default::default()
    }
  }

  fn make_remote_feed(
    url: &str,
    title: &str,
    backend_id: &str,
    remote_feed_id: &str,
    entries: Vec<FeedEntry>,
  ) -> Feed {
    Feed {
      url: url.to_string(),
      title: title.to_string(),
      entries,
      tags: None,
      backend: Some(crate::feeds::RemoteFeedRef {
        backend_id: backend_id.to_string(),
        remote_feed_id: remote_feed_id.to_string(),
      }),
    }
  }

  #[test]
  fn test_create_in_memory_cache() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds.is_empty());
  }

  #[test]
  fn test_save_and_load_feed() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Example Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z")),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z")),
      ],
    );

    cache.save_feed(&feed, 0, None).unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].title, "Example Feed");
    assert_eq!(feeds[0].url, "https://example.com/rss");
    assert_eq!(feeds[0].entries.len(), 2);
    // Entries should be ordered by published DESC
    assert_eq!(feeds[0].entries[0].title, "Post 2");
    assert_eq!(feeds[0].entries[1].title, "Post 1");
  }

  #[test]
  fn test_deleted_entry_is_not_resurrected_by_refetch() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Example Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z")),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z")),
      ],
    );

    cache.save_feed(&feed, 0, None).unwrap();
    cache
      .delete_entry(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
      )
      .unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries.len(), 1);
    assert_eq!(feeds[0].entries[0].title, "Post 2");

    // Re-fetching the feed (same upsert path main.rs uses on refresh) must
    // not bring the deleted entry back, even though it's still present in
    // the "remote" feed data being saved.
    cache.save_feed(&feed, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries.len(), 1);
    assert_eq!(feeds[0].entries[0].title, "Post 2");
  }

  #[test]
  fn test_save_feed_with_tags() {
    let cache = FeedCache::new_in_memory().unwrap();
    let mut feed = make_feed("https://example.com/rss", "Tagged Feed", vec![]);
    feed.tags = Some(vec!["blog".to_string(), "tech".to_string()]);

    cache.save_feed(&feed, 0, None).unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].tags.as_ref().unwrap().len(), 2);
    assert!(feeds[0]
      .tags
      .as_ref()
      .unwrap()
      .contains(&"blog".to_string()));
    assert!(feeds[0]
      .tags
      .as_ref()
      .unwrap()
      .contains(&"tech".to_string()));
  }

  #[test]
  fn test_save_feed_upsert_updates_title() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed1 = make_feed("https://example.com/rss", "Old Title", vec![]);
    cache.save_feed(&feed1, 0, None).unwrap();

    let feed2 = make_feed("https://example.com/rss", "New Title", vec![]);
    cache.save_feed(&feed2, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].title, "New Title");
  }

  #[test]
  fn test_entry_upsert_preserves_read_state() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", Some("2024-01-01T00:00:00Z"))],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    // Mark the entry as read
    cache
      .mark_entry_read(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
      )
      .unwrap();

    // Re-save the feed (simulating a refresh)
    cache.save_feed(&feed, 0, None).unwrap();

    // Entry should still be read
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds[0].entries[0].read);
  }

  #[test]
  fn test_mark_entry_read_and_unread() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", Some("2024-01-01T00:00:00Z"))],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    // Initially unread
    let feeds = cache.load_all_feeds().unwrap();
    assert!(!feeds[0].entries[0].read);

    // Mark read
    cache
      .mark_entry_read(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
      )
      .unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds[0].entries[0].read);

    // Mark unread
    cache
      .mark_entry_unread(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
      )
      .unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert!(!feeds[0].entries[0].read);
  }

  #[test]
  fn test_set_entry_scroll_position_roundtrip() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", Some("2024-01-01T00:00:00Z"))],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    // Defaults to 0
    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries[0].scroll_position, 0);

    cache
      .set_entry_scroll_position(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
        42,
      )
      .unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries[0].scroll_position, 42);
  }

  #[test]
  fn test_entry_upsert_preserves_scroll_position() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", Some("2024-01-01T00:00:00Z"))],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    cache
      .set_entry_scroll_position(
        "https://example.com/rss",
        "Post 1",
        Some("2024-01-01T00:00:00Z"),
        17,
      )
      .unwrap();

    // Re-save the feed (simulating a refresh)
    cache.save_feed(&feed, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries[0].scroll_position, 17);
  }

  #[test]
  fn test_mark_entry_read_with_null_published() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post No Date", None)],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    cache
      .mark_entry_read("https://example.com/rss", "Post No Date", None)
      .unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds[0].entries[0].read);
  }

  #[test]
  fn test_has_feed() {
    let cache = FeedCache::new_in_memory().unwrap();
    assert!(!cache.has_feed("https://example.com/rss").unwrap());

    let feed = make_feed("https://example.com/rss", "Feed", vec![]);
    cache.save_feed(&feed, 0, None).unwrap();

    assert!(cache.has_feed("https://example.com/rss").unwrap());
    assert!(!cache.has_feed("https://other.com/rss").unwrap());
  }

  #[test]
  fn test_remove_dead_feeds() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed_a = make_feed("https://a.com/rss", "Feed A", vec![make_entry("A1", None)]);
    let feed_b = make_feed("https://b.com/rss", "Feed B", vec![make_entry("B1", None)]);
    let feed_c = make_feed("https://c.com/rss", "Feed C", vec![]);
    cache.save_feed(&feed_a, 0, None).unwrap();
    cache.save_feed(&feed_b, 1, None).unwrap();
    cache.save_feed(&feed_c, 2, None).unwrap();

    // Keep only A and C
    let removed = cache
      .remove_dead_feeds(&["https://a.com/rss", "https://c.com/rss"])
      .unwrap();
    assert_eq!(removed, 1);

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 2);
    let urls: Vec<&str> = feeds.iter().map(|f| f.url.as_str()).collect();
    assert!(urls.contains(&"https://a.com/rss"));
    assert!(urls.contains(&"https://c.com/rss"));
    assert!(!urls.contains(&"https://b.com/rss"));
  }

  #[test]
  fn test_remove_dead_feeds_cascades_entries() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", None)],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    let removed = cache.remove_dead_feeds(&[]).unwrap();
    assert_eq!(removed, 1);

    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds.is_empty());
  }

  #[test]
  fn test_feed_ordering_by_position() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed_a = make_feed("https://a.com/rss", "Feed A", vec![]);
    let feed_b = make_feed("https://b.com/rss", "Feed B", vec![]);
    let feed_c = make_feed("https://c.com/rss", "Feed C", vec![]);

    // Save in reverse position order
    cache.save_feed(&feed_c, 2, None).unwrap();
    cache.save_feed(&feed_a, 0, None).unwrap();
    cache.save_feed(&feed_b, 1, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].title, "Feed A");
    assert_eq!(feeds[1].title, "Feed B");
    assert_eq!(feeds[2].title, "Feed C");
  }

  #[test]
  fn test_entry_media_roundtrip() {
    let cache = FeedCache::new_in_memory().unwrap();
    let mut entry = make_entry("Post with media", Some("2024-01-01T00:00:00Z"));
    entry.media = Some("https://example.com/podcast.mp3".to_string());

    let feed = make_feed("https://example.com/rss", "Feed", vec![entry]);
    cache.save_feed(&feed, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(
      feeds[0].entries[0].media.as_deref(),
      Some("https://example.com/podcast.mp3")
    );
  }

  #[test]
  fn test_entry_links_roundtrip() {
    let cache = FeedCache::new_in_memory().unwrap();
    let mut entry = make_entry("Post with links", None);
    entry.links = vec![
      "https://example.com/1".to_string(),
      "https://example.com/2".to_string(),
    ];

    let feed = make_feed("https://example.com/rss", "Feed", vec![entry]);
    cache.save_feed(&feed, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries[0].links.len(), 2);
    assert_eq!(feeds[0].entries[0].links[0], "https://example.com/1");
    assert_eq!(feeds[0].entries[0].links[1], "https://example.com/2");
  }

  #[test]
  fn test_incremental_save_adds_new_entries() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed1 = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 1", Some("2024-01-01T00:00:00Z"))],
    );
    cache.save_feed(&feed1, 0, None).unwrap();

    // Second save adds a new entry, keeps the old one
    let feed2 = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z")),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z")),
      ],
    );
    cache.save_feed(&feed2, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries.len(), 2);
  }

  #[test]
  fn test_old_entries_preserved_when_gone_from_remote() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed1 = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z")),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z")),
      ],
    );
    cache.save_feed(&feed1, 0, None).unwrap();

    // Second save only has Post 2 (Post 1 aged out of remote feed)
    let feed2 = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![make_entry("Post 2", Some("2024-02-01T00:00:00Z"))],
    );
    cache.save_feed(&feed2, 0, None).unwrap();

    // Post 1 should still be in the cache
    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries.len(), 2);
  }

  #[test]
  fn test_mark_feed_read() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_feed(
      "https://example.com/rss",
      "Feed",
      vec![
        make_entry("Post 1", Some("2024-01-01T00:00:00Z")),
        make_entry("Post 2", Some("2024-02-01T00:00:00Z")),
        make_entry("Post 3", None),
      ],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    // Initially all unread
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds[0].entries.iter().all(|e| !e.read));

    // Mark the whole feed as read
    cache.mark_feed_read("https://example.com/rss").unwrap();
    let feeds = cache.load_all_feeds().unwrap();
    assert!(feeds[0].entries.iter().all(|e| e.read));
  }

  #[test]
  fn test_mark_feed_read_nonexistent_feed() {
    let cache = FeedCache::new_in_memory().unwrap();
    // Should not error, just affect zero rows
    cache.mark_feed_read("https://nonexistent.com/rss").unwrap();
  }

  #[test]
  fn test_remote_feed_roundtrips_backend_ref() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed = make_remote_feed(
      "https://miniflux.example.com/feed/1",
      "Remote Feed",
      "home-miniflux",
      "1",
      vec![make_remote_entry("Post 1", "101")],
    );
    cache.save_feed(&feed, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    let backend = feeds[0].backend.as_ref().unwrap();
    assert_eq!(backend.backend_id, "home-miniflux");
    assert_eq!(backend.remote_feed_id, "1");
    assert_eq!(feeds[0].entries[0].remote_id.as_deref(), Some("101"));
  }

  #[test]
  fn test_remote_entry_upsert_keyed_on_remote_id_not_title() {
    let cache = FeedCache::new_in_memory().unwrap();
    let feed1 = make_remote_feed(
      "https://miniflux.example.com/feed/1",
      "Remote Feed",
      "home-miniflux",
      "1",
      vec![make_remote_entry("Original Title", "101")],
    );
    cache.save_feed(&feed1, 0, None).unwrap();
    cache
      .mark_entry_read(
        "https://miniflux.example.com/feed/1",
        "Original Title",
        None,
      )
      .unwrap();

    // Same remote_id, title changed upstream — must update in place (not
    // insert a duplicate) and must not reset the read flag.
    let feed2 = make_remote_feed(
      "https://miniflux.example.com/feed/1",
      "Remote Feed",
      "home-miniflux",
      "1",
      vec![make_remote_entry("Edited Title", "101")],
    );
    cache.save_feed(&feed2, 0, None).unwrap();

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds[0].entries.len(), 1);
    assert_eq!(feeds[0].entries[0].title, "Edited Title");
    assert!(feeds[0].entries[0].read);
  }

  #[test]
  fn test_remove_dead_feeds_does_not_prune_remote_feeds() {
    let cache = FeedCache::new_in_memory().unwrap();
    let local = make_feed("https://a.com/rss", "Local Feed", vec![]);
    let remote = make_remote_feed(
      "https://miniflux.example.com/feed/1",
      "Remote Feed",
      "home-miniflux",
      "1",
      vec![],
    );
    cache.save_feed(&local, 0, None).unwrap();
    cache.save_feed(&remote, 1, None).unwrap();

    // Local feed no longer in feeds.toml — remote feed must survive, even
    // though it's never present in `active_urls` either.
    let removed = cache.remove_dead_feeds(&[]).unwrap();
    assert_eq!(removed, 1);

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].url, "https://miniflux.example.com/feed/1");
  }

  #[test]
  fn test_remove_dead_backend_feeds_scoped_to_backend_id() {
    let cache = FeedCache::new_in_memory().unwrap();
    let remote_a = make_remote_feed(
      "https://a.example.com/feed",
      "Feed A",
      "home-miniflux",
      "1",
      vec![],
    );
    let remote_b = make_remote_feed(
      "https://b.example.com/feed",
      "Feed B",
      "other-miniflux",
      "1",
      vec![],
    );
    cache.save_feed(&remote_a, 0, None).unwrap();
    cache.save_feed(&remote_b, 1, None).unwrap();

    // Feed "1" no longer returned by home-miniflux's feed list — must prune
    // remote_a only, leaving remote_b (a different backend_id) untouched.
    let removed = cache
      .remove_dead_backend_feeds("home-miniflux", &[])
      .unwrap();
    assert_eq!(removed, 1);

    let feeds = cache.load_all_feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].url, "https://b.example.com/feed");
  }
}
