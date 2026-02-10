// ABOUTME: SQLite-backed index for fast spec and card queries without replaying events.
// ABOUTME: Provides upsert, delete, list, and rebuild operations synchronized with the event log.

use std::path::Path;

use rusqlite::{Connection, params};
use specd_core::card::Card;
use specd_core::event::{Event, EventPayload};
use specd_core::model::SpecCore;
use thiserror::Error;
use ulid::Ulid;

/// Errors that can occur during SQLite index operations.
#[derive(Debug, Error)]
pub enum SqliteError {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

/// Summary of a spec for list queries, matching the API's SpecSummary shape.
#[derive(Debug, Clone)]
pub struct SpecSummary {
    pub spec_id: String,
    pub title: String,
    pub one_liner: String,
    pub goal: String,
    pub updated_at: String,
}

/// A SQLite-backed index that mirrors spec and card data for fast reads.
/// This index is always rebuildable from the event log and serves as a
/// queryable cache, not the source of truth.
pub struct SqliteIndex {
    conn: Connection,
}

impl SqliteIndex {
    /// Open or create a SQLite index database at the given path.
    /// Runs migrations to ensure the schema is up to date.
    pub fn open(path: &Path) -> Result<Self, SqliteError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS specs (
                spec_id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                one_liner TEXT NOT NULL,
                goal TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cards (
                card_id TEXT PRIMARY KEY,
                spec_id TEXT NOT NULL,
                card_type TEXT NOT NULL,
                title TEXT NOT NULL,
                body TEXT,
                lane TEXT NOT NULL,
                sort_order REAL NOT NULL,
                created_by TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                FOREIGN KEY (spec_id) REFERENCES specs(spec_id)
            );

            CREATE TABLE IF NOT EXISTS meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );",
        )?;

        Ok(Self { conn })
    }

    /// Upsert a spec row from a SpecCore.
    pub fn update_spec(&self, spec: &SpecCore) -> Result<(), SqliteError> {
        self.conn.execute(
            "INSERT INTO specs (spec_id, title, one_liner, goal, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(spec_id) DO UPDATE SET
                title = excluded.title,
                one_liner = excluded.one_liner,
                goal = excluded.goal,
                updated_at = excluded.updated_at",
            params![
                spec.spec_id.to_string(),
                spec.title,
                spec.one_liner,
                spec.goal,
                spec.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Upsert a card row.
    pub fn update_card(&self, spec_id: &Ulid, card: &Card) -> Result<(), SqliteError> {
        self.conn.execute(
            "INSERT INTO cards (card_id, spec_id, card_type, title, body, lane, sort_order, created_by, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(card_id) DO UPDATE SET
                card_type = excluded.card_type,
                title = excluded.title,
                body = excluded.body,
                lane = excluded.lane,
                sort_order = excluded.sort_order,
                updated_at = excluded.updated_at",
            params![
                card.card_id.to_string(),
                spec_id.to_string(),
                card.card_type,
                card.title,
                card.body,
                card.lane,
                card.order,
                card.created_by,
                card.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Delete a card row by card_id.
    pub fn delete_card(&self, card_id: &Ulid) -> Result<(), SqliteError> {
        self.conn.execute(
            "DELETE FROM cards WHERE card_id = ?1",
            params![card_id.to_string()],
        )?;
        Ok(())
    }

    /// List all specs as summaries.
    pub fn list_specs(&self) -> Result<Vec<SpecSummary>, SqliteError> {
        let mut stmt = self
            .conn
            .prepare("SELECT spec_id, title, one_liner, goal, updated_at FROM specs ORDER BY updated_at DESC")?;

        let rows = stmt.query_map([], |row| {
            Ok(SpecSummary {
                spec_id: row.get(0)?,
                title: row.get(1)?,
                one_liner: row.get(2)?,
                goal: row.get(3)?,
                updated_at: row.get(4)?,
            })
        })?;

        let mut specs = Vec::new();
        for row in rows {
            specs.push(row?);
        }
        Ok(specs)
    }

    /// List all cards for a given spec, ordered by sort_order.
    pub fn list_cards(&self, spec_id: &Ulid) -> Result<Vec<CardRow>, SqliteError> {
        let mut stmt = self.conn.prepare(
            "SELECT card_id, spec_id, card_type, title, body, lane, sort_order, created_by, updated_at
             FROM cards WHERE spec_id = ?1 ORDER BY sort_order ASC",
        )?;

        let rows = stmt.query_map(params![spec_id.to_string()], |row| {
            Ok(CardRow {
                card_id: row.get(0)?,
                spec_id: row.get(1)?,
                card_type: row.get(2)?,
                title: row.get(3)?,
                body: row.get(4)?,
                lane: row.get(5)?,
                sort_order: row.get(6)?,
                created_by: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;

        let mut cards = Vec::new();
        for row in rows {
            cards.push(row?);
        }
        Ok(cards)
    }

    /// Get the last event ID that was indexed, from the meta table.
    pub fn get_last_event_id(&self) -> Result<Option<u64>, SqliteError> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM meta WHERE key = 'last_event_id'")?;

        let result = stmt.query_row([], |row| {
            let val: String = row.get(0)?;
            Ok(val)
        });

        match result {
            Ok(val) => Ok(val.parse::<u64>().ok()),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(SqliteError::Sqlite(e)),
        }
    }

    /// Set the last event ID in the meta table.
    pub fn set_last_event_id(&self, event_id: u64) -> Result<(), SqliteError> {
        self.conn.execute(
            "INSERT INTO meta (key, value) VALUES ('last_event_id', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![event_id.to_string()],
        )?;
        Ok(())
    }

    /// Clear all data and rebuild from a list of events.
    pub fn rebuild_from_events(&self, events: &[Event]) -> Result<(), SqliteError> {
        self.conn.execute("DELETE FROM cards", [])?;
        self.conn.execute("DELETE FROM specs", [])?;
        self.conn.execute("DELETE FROM meta", [])?;

        for event in events {
            self.apply_event(event)?;
        }

        Ok(())
    }

    /// Incrementally apply a single event to update the index.
    pub fn apply_event(&self, event: &Event) -> Result<(), SqliteError> {
        let spec_id = event.spec_id;

        match &event.payload {
            EventPayload::SpecCreated {
                title,
                one_liner,
                goal,
            } => {
                self.conn.execute(
                    "INSERT INTO specs (spec_id, title, one_liner, goal, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(spec_id) DO UPDATE SET
                        title = excluded.title,
                        one_liner = excluded.one_liner,
                        goal = excluded.goal,
                        updated_at = excluded.updated_at",
                    params![
                        spec_id.to_string(),
                        title,
                        one_liner,
                        goal,
                        event.timestamp.to_rfc3339(),
                    ],
                )?;
            }

            EventPayload::SpecCoreUpdated {
                title,
                one_liner,
                goal,
                ..
            } => {
                // Only update fields that are Some
                if let Some(t) = title {
                    self.conn.execute(
                        "UPDATE specs SET title = ?1, updated_at = ?2 WHERE spec_id = ?3",
                        params![t, event.timestamp.to_rfc3339(), spec_id.to_string()],
                    )?;
                }
                if let Some(o) = one_liner {
                    self.conn.execute(
                        "UPDATE specs SET one_liner = ?1, updated_at = ?2 WHERE spec_id = ?3",
                        params![o, event.timestamp.to_rfc3339(), spec_id.to_string()],
                    )?;
                }
                if let Some(g) = goal {
                    self.conn.execute(
                        "UPDATE specs SET goal = ?1, updated_at = ?2 WHERE spec_id = ?3",
                        params![g, event.timestamp.to_rfc3339(), spec_id.to_string()],
                    )?;
                }
                // Always update the updated_at timestamp
                self.conn.execute(
                    "UPDATE specs SET updated_at = ?1 WHERE spec_id = ?2",
                    params![event.timestamp.to_rfc3339(), spec_id.to_string()],
                )?;
            }

            EventPayload::CardCreated { card } => {
                self.update_card(&spec_id, card)?;
            }

            EventPayload::CardUpdated {
                card_id,
                title,
                body,
                card_type,
                ..
            } => {
                if let Some(t) = title {
                    self.conn.execute(
                        "UPDATE cards SET title = ?1, updated_at = ?2 WHERE card_id = ?3",
                        params![t, event.timestamp.to_rfc3339(), card_id.to_string()],
                    )?;
                }
                if let Some(b) = body {
                    self.conn.execute(
                        "UPDATE cards SET body = ?1, updated_at = ?2 WHERE card_id = ?3",
                        params![
                            b.as_deref(),
                            event.timestamp.to_rfc3339(),
                            card_id.to_string()
                        ],
                    )?;
                }
                if let Some(ct) = card_type {
                    self.conn.execute(
                        "UPDATE cards SET card_type = ?1, updated_at = ?2 WHERE card_id = ?3",
                        params![ct, event.timestamp.to_rfc3339(), card_id.to_string()],
                    )?;
                }
                self.conn.execute(
                    "UPDATE cards SET updated_at = ?1 WHERE card_id = ?2",
                    params![event.timestamp.to_rfc3339(), card_id.to_string()],
                )?;
            }

            EventPayload::CardMoved {
                card_id,
                lane,
                order,
            } => {
                self.conn.execute(
                    "UPDATE cards SET lane = ?1, sort_order = ?2, updated_at = ?3 WHERE card_id = ?4",
                    params![lane, order, event.timestamp.to_rfc3339(), card_id.to_string()],
                )?;
            }

            EventPayload::CardDeleted { card_id } => {
                self.delete_card(card_id)?;
            }

            EventPayload::UndoApplied { inverse_events, .. } => {
                // Apply inverse events to the index
                for inverse_payload in inverse_events {
                    let synthetic = Event {
                        event_id: event.event_id,
                        spec_id: event.spec_id,
                        timestamp: event.timestamp,
                        payload: inverse_payload.clone(),
                    };
                    self.apply_event(&synthetic)?;
                }
            }

            // Other event types don't affect the index
            _ => {}
        }

        self.set_last_event_id(event.event_id)?;

        Ok(())
    }
}

/// A row from the cards table for list query results.
#[derive(Debug, Clone)]
pub struct CardRow {
    pub card_id: String,
    pub spec_id: String,
    pub card_type: String,
    pub title: String,
    pub body: Option<String>,
    pub lane: String,
    pub sort_order: f64,
    pub created_by: String,
    pub updated_at: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use specd_core::card::Card;
    use specd_core::model::SpecCore;
    use tempfile::TempDir;

    fn make_spec() -> SpecCore {
        SpecCore::new(
            "Test Spec".to_string(),
            "A test".to_string(),
            "Build it".to_string(),
        )
    }

    fn make_card(created_by: &str) -> Card {
        Card::new(
            "idea".to_string(),
            "Test Card".to_string(),
            created_by.to_string(),
        )
    }

    fn make_event(event_id: u64, spec_id: Ulid, payload: EventPayload) -> Event {
        Event {
            event_id,
            spec_id,
            timestamp: Utc::now(),
            payload,
        }
    }

    #[test]
    fn sqlite_spec_upsert_and_list() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = SqliteIndex::open(&db_path).unwrap();

        let spec = make_spec();
        idx.update_spec(&spec).unwrap();

        let specs = idx.list_specs().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].title, "Test Spec");
        assert_eq!(specs[0].one_liner, "A test");
        assert_eq!(specs[0].goal, "Build it");
        assert_eq!(specs[0].spec_id, spec.spec_id.to_string());

        // Upsert with changed title
        let mut updated_spec = spec.clone();
        updated_spec.title = "Updated Spec".to_string();
        updated_spec.updated_at = Utc::now();
        idx.update_spec(&updated_spec).unwrap();

        let specs = idx.list_specs().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].title, "Updated Spec");
    }

    #[test]
    fn sqlite_card_crud() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = SqliteIndex::open(&db_path).unwrap();

        let spec = make_spec();
        idx.update_spec(&spec).unwrap();

        // Create card
        let card = make_card("human");
        idx.update_card(&spec.spec_id, &card).unwrap();

        let cards = idx.list_cards(&spec.spec_id).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Test Card");
        assert_eq!(cards[0].card_type, "idea");
        assert_eq!(cards[0].created_by, "human");

        // Update card (upsert)
        let mut updated_card = card.clone();
        updated_card.title = "Updated Card".to_string();
        updated_card.updated_at = Utc::now();
        idx.update_card(&spec.spec_id, &updated_card).unwrap();

        let cards = idx.list_cards(&spec.spec_id).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Updated Card");

        // Delete card
        idx.delete_card(&card.card_id).unwrap();

        let cards = idx.list_cards(&spec.spec_id).unwrap();
        assert_eq!(cards.len(), 0);
    }

    #[test]
    fn sqlite_rebuild_from_events() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = SqliteIndex::open(&db_path).unwrap();

        let spec_id = Ulid::new();
        let card = Card::new(
            "idea".to_string(),
            "Event Card".to_string(),
            "agent".to_string(),
        );
        let card_id = card.card_id;

        let events = vec![
            make_event(
                1,
                spec_id,
                EventPayload::SpecCreated {
                    title: "Rebuilt Spec".to_string(),
                    one_liner: "From events".to_string(),
                    goal: "Test rebuild".to_string(),
                },
            ),
            make_event(2, spec_id, EventPayload::CardCreated { card }),
            make_event(
                3,
                spec_id,
                EventPayload::CardMoved {
                    card_id,
                    lane: "Plan".to_string(),
                    order: 2.0,
                },
            ),
        ];

        idx.rebuild_from_events(&events).unwrap();

        let specs = idx.list_specs().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].title, "Rebuilt Spec");

        let cards = idx.list_cards(&spec_id).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Event Card");
        assert_eq!(cards[0].lane, "Plan");
        assert_eq!(cards[0].sort_order, 2.0);

        let last_id = idx.get_last_event_id().unwrap();
        assert_eq!(last_id, Some(3));
    }

    #[test]
    fn sqlite_apply_event_incrementally() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = SqliteIndex::open(&db_path).unwrap();

        let spec_id = Ulid::new();

        // Apply SpecCreated
        idx.apply_event(&make_event(
            1,
            spec_id,
            EventPayload::SpecCreated {
                title: "Incremental".to_string(),
                one_liner: "Step by step".to_string(),
                goal: "Test incremental".to_string(),
            },
        ))
        .unwrap();

        let specs = idx.list_specs().unwrap();
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].title, "Incremental");

        // Apply CardCreated
        let card = Card::new(
            "task".to_string(),
            "Do Thing".to_string(),
            "human".to_string(),
        );
        let card_id = card.card_id;
        idx.apply_event(&make_event(2, spec_id, EventPayload::CardCreated { card }))
            .unwrap();

        let cards = idx.list_cards(&spec_id).unwrap();
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].title, "Do Thing");

        // Apply CardUpdated
        idx.apply_event(&make_event(
            3,
            spec_id,
            EventPayload::CardUpdated {
                card_id,
                title: Some("Done Thing".to_string()),
                body: Some(Some("With a body".to_string())),
                card_type: None,
                refs: None,
            },
        ))
        .unwrap();

        let cards = idx.list_cards(&spec_id).unwrap();
        assert_eq!(cards[0].title, "Done Thing");
        assert_eq!(cards[0].body.as_deref(), Some("With a body"));

        // Apply CardDeleted
        idx.apply_event(&make_event(
            4,
            spec_id,
            EventPayload::CardDeleted { card_id },
        ))
        .unwrap();

        let cards = idx.list_cards(&spec_id).unwrap();
        assert_eq!(cards.len(), 0);

        // Verify last event id
        assert_eq!(idx.get_last_event_id().unwrap(), Some(4));
    }

    #[test]
    fn sqlite_last_event_id_tracking() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("index.db");
        let idx = SqliteIndex::open(&db_path).unwrap();

        // Initially no last event id
        assert_eq!(idx.get_last_event_id().unwrap(), None);

        // Set it
        idx.set_last_event_id(42).unwrap();
        assert_eq!(idx.get_last_event_id().unwrap(), Some(42));

        // Update it
        idx.set_last_event_id(100).unwrap();
        assert_eq!(idx.get_last_event_id().unwrap(), Some(100));
    }
}
