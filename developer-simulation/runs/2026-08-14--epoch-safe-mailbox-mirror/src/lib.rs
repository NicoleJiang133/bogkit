use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use serde::{Deserialize, Serialize};

pub const SEEN: u8 = 1;
/// Inclusive upper bound for every unsigned value stored in a `SQLite` `INTEGER`.
pub const SQLITE_INTEGER_MAX: u64 = 9_223_372_036_854_775_807;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub uid: u64,
    pub fingerprint: String,
    pub internal_date: i64,
    pub size: u64,
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    CreateMailbox {
        display_name: String,
        uid_validity: u64,
    },
    Add {
        uid_validity: u64,
        message: Message,
        cursor: u64,
    },
    ReplaceFlags {
        uid_validity: u64,
        uid: u64,
        flags: u8,
        cursor: u64,
    },
    Expunge {
        uid_validity: u64,
        uid: u64,
        cursor: u64,
    },
    Rename {
        display_name: String,
    },
    Delete,
    SnapshotBegin {
        uid_validity: u64,
    },
    SnapshotMessage {
        uid_validity: u64,
        message: Message,
    },
    SnapshotCommit {
        uid_validity: u64,
        cursor: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Batch {
    pub batch_id: String,
    pub mailbox_id: String,
    pub sequence: u64,
    pub events: Vec<Event>,
}

impl Batch {
    #[must_use]
    pub fn new(
        batch_id: impl Into<String>,
        mailbox_id: impl Into<String>,
        sequence: u64,
        events: Vec<Event>,
    ) -> Self {
        Self {
            batch_id: batch_id.into(),
            mailbox_id: mailbox_id.into(),
            sequence,
            events,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorStatus {
    Synced,
    NeedsRepair,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MirrorError(String);

impl MirrorError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl Display for MirrorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MirrorError {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectedMessage {
    pub uid: u64,
    pub fingerprint: String,
    pub flags: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub mailbox_id: String,
    pub display_name: String,
    pub uid_validity: u64,
    pub cursor: u64,
    pub status: MirrorStatus,
    pub total: usize,
    pub unread: usize,
    pub selected: Vec<SelectedMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Staging {
    uid_validity: u64,
    messages: BTreeMap<u64, Message>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Mailbox {
    display_name: String,
    uid_validity: u64,
    cursor: u64,
    status: MirrorStatus,
    messages: BTreeMap<u64, Message>,
    staging: Option<Staging>,
}

#[derive(Debug, Clone, Default)]
pub struct ReferenceMirror {
    mailboxes: BTreeMap<String, Mailbox>,
    accepted: BTreeMap<String, String>,
}

impl ReferenceMirror {
    /// Applies one already-decoded batch atomically.
    ///
    /// # Errors
    ///
    /// Returns an error when replay content differs or an event violates mirror invariants.
    pub fn apply(&mut self, batch: &Batch) -> Result<ApplyOutcome, MirrorError> {
        validate_batch_integer_range(batch)?;
        let canonical = serde_json::to_string(batch)
            .map_err(|error| MirrorError::new(format!("cannot encode batch: {error}")))?;
        if let Some(previous) = self.accepted.get(&batch.batch_id) {
            return if previous == &canonical {
                Ok(ApplyOutcome::Duplicate)
            } else {
                Err(MirrorError::new("batch id reused with different content"))
            };
        }

        let before = self.mailboxes.get(&batch.mailbox_id).cloned();
        for event in &batch.events {
            if let Err(error) = self.apply_event(&batch.mailbox_id, event) {
                if let Some(mailbox) = before {
                    self.mailboxes.insert(batch.mailbox_id.clone(), mailbox);
                } else {
                    self.mailboxes.remove(&batch.mailbox_id);
                }
                return Err(error);
            }
        }
        self.accepted.insert(batch.batch_id.clone(), canonical);
        Ok(ApplyOutcome::Applied)
    }

    /// Reads one canonical mailbox checkpoint.
    ///
    /// # Errors
    ///
    /// Returns an error when the stable mailbox identity is unknown.
    pub fn checkpoint(
        &self,
        mailbox_id: &str,
        selected_uids: &[u64],
    ) -> Result<Checkpoint, MirrorError> {
        let mailbox = self
            .mailboxes
            .get(mailbox_id)
            .ok_or_else(|| MirrorError::new("unknown mailbox"))?;
        let requested: BTreeSet<u64> = selected_uids.iter().copied().collect();
        let selected = mailbox
            .messages
            .iter()
            .filter(|(uid, _)| requested.contains(uid))
            .map(|(_, message)| SelectedMessage {
                uid: message.uid,
                fingerprint: message.fingerprint.clone(),
                flags: message.flags,
            })
            .collect();
        let unread = mailbox
            .messages
            .values()
            .filter(|message| message.flags & SEEN == 0)
            .count();
        Ok(Checkpoint {
            mailbox_id: mailbox_id.to_owned(),
            display_name: mailbox.display_name.clone(),
            uid_validity: mailbox.uid_validity,
            cursor: mailbox.cursor,
            status: mailbox.status.clone(),
            total: mailbox.messages.len(),
            unread,
            selected,
        })
    }

    #[must_use]
    pub fn manifest(&self) -> Manifest {
        let mailboxes = self
            .mailboxes
            .iter()
            .map(|(mailbox_id, mailbox)| MailboxManifest {
                mailbox_id: mailbox_id.clone(),
                display_name: mailbox.display_name.clone(),
                uid_validity: mailbox.uid_validity,
                cursor: mailbox.cursor,
                status: mailbox.status.clone(),
                messages: mailbox.messages.values().cloned().collect(),
            })
            .collect();
        Manifest { mailboxes }
    }

    fn apply_event(&mut self, mailbox_id: &str, event: &Event) -> Result<(), MirrorError> {
        match event {
            Event::CreateMailbox {
                display_name,
                uid_validity,
            } => {
                if self.mailboxes.contains_key(mailbox_id) {
                    return Err(MirrorError::new("mailbox already exists"));
                }
                self.mailboxes.insert(
                    mailbox_id.to_owned(),
                    Mailbox {
                        display_name: display_name.clone(),
                        uid_validity: *uid_validity,
                        cursor: 0,
                        status: MirrorStatus::Synced,
                        messages: BTreeMap::new(),
                        staging: None,
                    },
                );
                Ok(())
            }
            Event::Delete => {
                self.mailboxes
                    .remove(mailbox_id)
                    .ok_or_else(|| MirrorError::new("unknown mailbox"))?;
                Ok(())
            }
            _ => {
                let mailbox = self
                    .mailboxes
                    .get_mut(mailbox_id)
                    .ok_or_else(|| MirrorError::new("unknown mailbox"))?;
                Self::apply_mailbox_event(mailbox, event)
            }
        }
    }

    fn apply_mailbox_event(mailbox: &mut Mailbox, event: &Event) -> Result<(), MirrorError> {
        match event {
            Event::Add {
                uid_validity,
                message,
                cursor,
            } => {
                Self::require_epoch(mailbox, *uid_validity)?;
                Self::require_cursor(mailbox, *cursor)?;
                Self::validate_message(message)?;
                mailbox.messages.insert(message.uid, message.clone());
                mailbox.cursor = *cursor;
            }
            Event::SnapshotBegin { uid_validity } => {
                if *uid_validity == mailbox.uid_validity {
                    return Err(MirrorError::new("snapshot must replace the current epoch"));
                }
                mailbox.status = MirrorStatus::NeedsRepair;
                mailbox.staging = Some(Staging {
                    uid_validity: *uid_validity,
                    messages: BTreeMap::new(),
                });
            }
            Event::SnapshotMessage {
                uid_validity,
                message,
            } => {
                Self::validate_message(message)?;
                let staging = mailbox
                    .staging
                    .as_mut()
                    .ok_or_else(|| MirrorError::new("snapshot not started"))?;
                if staging.uid_validity != *uid_validity {
                    return Err(MirrorError::new("snapshot epoch mismatch"));
                }
                staging.messages.insert(message.uid, message.clone());
            }
            Event::SnapshotCommit {
                uid_validity,
                cursor,
            } => {
                Self::require_cursor(mailbox, *cursor)?;
                let staging = mailbox
                    .staging
                    .take()
                    .ok_or_else(|| MirrorError::new("snapshot not started"))?;
                if staging.uid_validity != *uid_validity {
                    return Err(MirrorError::new("snapshot epoch mismatch"));
                }
                mailbox.uid_validity = *uid_validity;
                mailbox.cursor = *cursor;
                mailbox.messages = staging.messages;
                mailbox.status = MirrorStatus::Synced;
            }
            Event::ReplaceFlags {
                uid_validity,
                uid,
                flags,
                cursor,
            } => {
                Self::require_epoch(mailbox, *uid_validity)?;
                Self::require_cursor(mailbox, *cursor)?;
                if *flags > 0x0f {
                    return Err(MirrorError::new("unsupported flag bits"));
                }
                let message = mailbox
                    .messages
                    .get_mut(uid)
                    .ok_or_else(|| MirrorError::new("unknown message"))?;
                message.flags = *flags;
                mailbox.cursor = *cursor;
            }
            Event::Expunge {
                uid_validity,
                uid,
                cursor,
            } => {
                Self::require_epoch(mailbox, *uid_validity)?;
                Self::require_cursor(mailbox, *cursor)?;
                mailbox
                    .messages
                    .remove(uid)
                    .ok_or_else(|| MirrorError::new("unknown message"))?;
                mailbox.cursor = *cursor;
            }
            Event::Rename { display_name } => mailbox.display_name.clone_from(display_name),
            Event::CreateMailbox { .. } | Event::Delete => {
                return Err(MirrorError::new("invalid mailbox event"));
            }
        }
        Ok(())
    }

    fn require_epoch(mailbox: &Mailbox, uid_validity: u64) -> Result<(), MirrorError> {
        if mailbox.uid_validity == uid_validity && mailbox.status == MirrorStatus::Synced {
            Ok(())
        } else {
            Err(MirrorError::new("incremental event for inactive epoch"))
        }
    }

    fn require_cursor(mailbox: &Mailbox, cursor: u64) -> Result<(), MirrorError> {
        if cursor >= mailbox.cursor {
            Ok(())
        } else {
            Err(MirrorError::new("decreasing cursor"))
        }
    }

    fn validate_message(message: &Message) -> Result<(), MirrorError> {
        if message.flags > 0x0f {
            return Err(MirrorError::new("unsupported flag bits"));
        }
        if message.fingerprint.len() != 32
            || !message
                .fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(MirrorError::new("fingerprint must be 16-byte hex"));
        }
        Ok(())
    }
}

pub struct SqliteBaseline {
    path: PathBuf,
}

impl SqliteBaseline {
    /// Reopens an existing mirror database.
    ///
    /// # Errors
    ///
    /// Returns an error when the database path does not exist.
    pub fn open(path: &Path) -> Result<Self, MirrorError> {
        if !path.is_file() {
            return Err(MirrorError::new("mirror database does not exist"));
        }
        Ok(Self {
            path: path.to_owned(),
        })
    }

    /// Creates or initializes the faithful `SQLite` schema.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot initialize the database.
    pub fn create(path: &Path) -> Result<Self, MirrorError> {
        let baseline = Self {
            path: path.to_owned(),
        };
        baseline.run_sql(
            ".bail on\n\
             PRAGMA journal_mode=WAL;\n\
             PRAGMA synchronous=FULL;\n\
             PRAGMA foreign_keys=ON;\n\
             CREATE TABLE IF NOT EXISTS mailboxes (\n\
               mailbox_id TEXT PRIMARY KEY,\n\
               display_name TEXT NOT NULL,\n\
               uid_validity INTEGER NOT NULL,\n\
               cursor INTEGER NOT NULL,\n\
               status TEXT NOT NULL CHECK(status IN ('synced','needs_repair'))\n\
             );\n\
             CREATE TABLE IF NOT EXISTS messages (\n\
               mailbox_id TEXT NOT NULL REFERENCES mailboxes(mailbox_id) ON DELETE CASCADE,\n\
               uid_validity INTEGER NOT NULL,\n\
               uid INTEGER NOT NULL,\n\
               fingerprint TEXT NOT NULL CHECK(length(fingerprint)=32),\n\
               internal_date INTEGER NOT NULL,\n\
               byte_size INTEGER NOT NULL,\n\
               flags INTEGER NOT NULL CHECK(flags BETWEEN 0 AND 15),\n\
               PRIMARY KEY(mailbox_id, uid_validity, uid)\n\
             );\n\
             CREATE INDEX IF NOT EXISTS message_counts\n\
               ON messages(mailbox_id, uid_validity, flags);\n\
             CREATE TABLE IF NOT EXISTS staging_meta (\n\
               mailbox_id TEXT PRIMARY KEY REFERENCES mailboxes(mailbox_id) ON DELETE CASCADE,\n\
               uid_validity INTEGER NOT NULL\n\
             );\n\
             CREATE TABLE IF NOT EXISTS staging_messages (\n\
               mailbox_id TEXT NOT NULL,\n\
               uid_validity INTEGER NOT NULL,\n\
               uid INTEGER NOT NULL,\n\
               fingerprint TEXT NOT NULL CHECK(length(fingerprint)=32),\n\
               internal_date INTEGER NOT NULL,\n\
               byte_size INTEGER NOT NULL,\n\
               flags INTEGER NOT NULL CHECK(flags BETWEEN 0 AND 15),\n\
               PRIMARY KEY(mailbox_id, uid_validity, uid)\n\
             );\n\
             CREATE TABLE IF NOT EXISTS accepted_batches (\n\
               batch_id TEXT PRIMARY KEY,\n\
               canonical TEXT NOT NULL,\n\
               mailbox_id TEXT NOT NULL,\n\
               sequence INTEGER NOT NULL\n\
             );",
        )?;
        Ok(baseline)
    }

    /// Applies one batch in one explicit `SQLite` transaction.
    ///
    /// # Errors
    ///
    /// Returns an error when the batch violates an invariant or `SQLite` rejects the transaction.
    #[allow(clippy::format_push_string)]
    pub fn apply(&mut self, batch: &Batch) -> Result<ApplyOutcome, MirrorError> {
        validate_batch_integer_range(batch)?;
        let canonical = serde_json::to_string(batch)
            .map_err(|error| MirrorError::new(format!("cannot encode batch: {error}")))?;
        let batch_id = sql_text(&batch.batch_id);
        let canonical = sql_text(&canonical);
        let mailbox_id = sql_text(&batch.mailbox_id);
        let mut sql = format!(
            ".bail on\n.timeout 5000\nPRAGMA foreign_keys=ON;\nBEGIN IMMEDIATE;\n\
             CREATE TEMP TABLE admission(apply INTEGER NOT NULL);\n\
             INSERT INTO admission SELECT CASE WHEN EXISTS(SELECT 1 FROM accepted_batches WHERE batch_id={batch_id}) THEN 0 ELSE 1 END;\n\
             CREATE TEMP TABLE assertion(ok INTEGER NOT NULL CHECK(ok=1));\n\
             INSERT INTO assertion VALUES(CASE WHEN NOT EXISTS(SELECT 1 FROM accepted_batches WHERE batch_id={batch_id}) OR EXISTS(SELECT 1 FROM accepted_batches WHERE batch_id={batch_id} AND canonical={canonical}) THEN 1 ELSE 0 END);\n"
        );
        for event in &batch.events {
            append_event_sql(&mut sql, &mailbox_id, event);
        }
        sql.push_str(&format!(
            "INSERT INTO accepted_batches(batch_id,canonical,mailbox_id,sequence) SELECT {batch_id},{canonical},{mailbox_id},{} WHERE (SELECT apply FROM admission)=1;\nCOMMIT;\nSELECT apply FROM admission;\n",
            batch.sequence
        ));
        let output = self.run_sql(&sql)?;
        match output.lines().last().map(str::trim) {
            Some("1") => Ok(ApplyOutcome::Applied),
            Some("0") => Ok(ApplyOutcome::Duplicate),
            _ => Err(MirrorError::new("sqlite admission result missing")),
        }
    }

    /// Reads metadata, counts, and selected messages from one `SQLite` snapshot.
    ///
    /// # Errors
    ///
    /// Returns an error for an unknown mailbox or failed `SQLite` query.
    pub fn checkpoint(
        &self,
        mailbox_id: &str,
        selected_uids: &[u64],
    ) -> Result<Checkpoint, MirrorError> {
        let mailbox_id_sql = sql_text(mailbox_id);
        let selected_predicate = if selected_uids.is_empty() {
            "0".to_owned()
        } else {
            format!(
                "x.uid IN ({})",
                selected_uids
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        let rows: Vec<SqliteCheckpointRow> = self.query_json(&format!(
            "SELECT mailbox_id,display_name,uid_validity,cursor,status,\
             (SELECT count(*) FROM messages x WHERE x.mailbox_id=m.mailbox_id AND x.uid_validity=m.uid_validity) AS total,\
             (SELECT count(*) FROM messages x WHERE x.mailbox_id=m.mailbox_id AND x.uid_validity=m.uid_validity AND (x.flags & 1)=0) AS unread,\
             (SELECT json_group_array(json_object('uid',picked.uid,'fingerprint',picked.fingerprint,'flags',picked.flags)) \
                FROM (SELECT x.uid,x.fingerprint,x.flags FROM messages x \
                      WHERE x.mailbox_id=m.mailbox_id AND x.uid_validity=m.uid_validity AND {selected_predicate} ORDER BY x.uid) picked) AS selected_json \
             FROM mailboxes m WHERE mailbox_id={mailbox_id_sql};"
        ))?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| MirrorError::new("unknown mailbox"))?;
        let selected = serde_json::from_str(&row.selected_json)
            .map_err(|error| MirrorError::new(format!("invalid selected-message JSON: {error}")))?;
        Ok(Checkpoint {
            mailbox_id: row.mailbox_id,
            display_name: row.display_name,
            uid_validity: row.uid_validity,
            cursor: row.cursor,
            status: row.status,
            total: row.total,
            unread: row.unread,
            selected,
        })
    }

    /// Produces the canonical visible manifest.
    ///
    /// # Errors
    ///
    /// Returns an error when `SQLite` cannot enumerate visible state.
    pub fn manifest(&self) -> Result<Manifest, MirrorError> {
        let rows: Vec<SqliteMailboxRow> = self.query_json(
            "SELECT mailbox_id,display_name,uid_validity,cursor,status FROM mailboxes ORDER BY mailbox_id;",
        )?;
        let mut mailboxes = Vec::with_capacity(rows.len());
        for row in rows {
            let messages = self.query_json(&format!(
                "SELECT uid,fingerprint,internal_date,byte_size AS size,flags FROM messages \
                 WHERE mailbox_id={} AND uid_validity={} ORDER BY uid;",
                sql_text(&row.mailbox_id),
                row.uid_validity
            ))?;
            mailboxes.push(MailboxManifest {
                mailbox_id: row.mailbox_id,
                display_name: row.display_name,
                uid_validity: row.uid_validity,
                cursor: row.cursor,
                status: row.status,
                messages,
            });
        }
        Ok(Manifest { mailboxes })
    }

    fn query_json<T: for<'de> Deserialize<'de>>(&self, query: &str) -> Result<T, MirrorError> {
        let output = Command::new("/usr/bin/sqlite3")
            .arg("-json")
            .arg("-cmd")
            .arg(".timeout 5000")
            .arg(&self.path)
            .arg(query)
            .output()
            .map_err(|error| MirrorError::new(format!("cannot run sqlite3: {error}")))?;
        if !output.status.success() {
            return Err(MirrorError::new(format!(
                "sqlite query failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let json = if output.stdout.is_empty() {
            b"[]".as_slice()
        } else {
            output.stdout.as_slice()
        };
        serde_json::from_slice(json)
            .map_err(|error| MirrorError::new(format!("invalid sqlite JSON: {error}")))
    }

    fn run_sql(&self, sql: &str) -> Result<String, MirrorError> {
        let mut child = Command::new("/usr/bin/sqlite3")
            .arg(&self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| MirrorError::new(format!("cannot run sqlite3: {error}")))?;
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| MirrorError::new("sqlite stdin unavailable"))?;
        stdin
            .write_all(sql.as_bytes())
            .map_err(|error| MirrorError::new(format!("cannot write sqlite script: {error}")))?;
        drop(stdin);
        let output = child
            .wait_with_output()
            .map_err(|error| MirrorError::new(format!("cannot wait for sqlite3: {error}")))?;
        if !output.status.success() {
            return Err(MirrorError::new(format!(
                "sqlite transaction rejected: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        String::from_utf8(output.stdout)
            .map_err(|error| MirrorError::new(format!("sqlite emitted invalid UTF-8: {error}")))
    }
}

#[derive(Deserialize)]
struct SqliteCheckpointRow {
    mailbox_id: String,
    display_name: String,
    uid_validity: u64,
    cursor: u64,
    status: MirrorStatus,
    total: usize,
    unread: usize,
    selected_json: String,
}

#[derive(Deserialize)]
struct SqliteMailboxRow {
    mailbox_id: String,
    display_name: String,
    uid_validity: u64,
    cursor: u64,
    status: MirrorStatus,
}

#[allow(clippy::format_push_string)]
fn append_event_sql(sql: &mut String, mailbox_id: &str, event: &Event) {
    let admission = "(SELECT apply FROM admission)=1";
    match event {
        Event::CreateMailbox {
            display_name,
            uid_validity,
        } => sql.push_str(&format!(
            "INSERT INTO mailboxes(mailbox_id,display_name,uid_validity,cursor,status) \
             SELECT {mailbox_id},{},{uid_validity},0,'synced' WHERE {admission};\n",
            sql_text(display_name)
        )),
        Event::Add {
            uid_validity,
            message,
            cursor,
        } => {
            append_active_assertion(sql, mailbox_id, *uid_validity, *cursor);
            append_message_assertion(sql, message);
            sql.push_str(&format!(
                "INSERT INTO messages(mailbox_id,uid_validity,uid,fingerprint,internal_date,byte_size,flags) \
                 SELECT {mailbox_id},{uid_validity},{},{},{},{},{} WHERE {admission} \
                 ON CONFLICT(mailbox_id,uid_validity,uid) DO UPDATE SET fingerprint=excluded.fingerprint,internal_date=excluded.internal_date,byte_size=excluded.byte_size,flags=excluded.flags;\n\
                 UPDATE mailboxes SET cursor={cursor} WHERE mailbox_id={mailbox_id} AND {admission};\n",
                message.uid,
                sql_text(&message.fingerprint),
                message.internal_date,
                message.size,
                message.flags
            ));
        }
        Event::ReplaceFlags {
            uid_validity,
            uid,
            flags,
            cursor,
        } => {
            append_active_assertion(sql, mailbox_id, *uid_validity, *cursor);
            sql.push_str(&format!(
                "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR ({flags} BETWEEN 0 AND 15 AND EXISTS(SELECT 1 FROM messages WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity} AND uid={uid})) THEN 1 ELSE 0 END);\n\
                 UPDATE messages SET flags={flags} WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity} AND uid={uid} AND {admission};\n\
                 UPDATE mailboxes SET cursor={cursor} WHERE mailbox_id={mailbox_id} AND {admission};\n"
            ));
        }
        Event::Expunge {
            uid_validity,
            uid,
            cursor,
        } => {
            append_active_assertion(sql, mailbox_id, *uid_validity, *cursor);
            sql.push_str(&format!(
                "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM messages WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity} AND uid={uid}) THEN 1 ELSE 0 END);\n\
                 DELETE FROM messages WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity} AND uid={uid} AND {admission};\n\
                 UPDATE mailboxes SET cursor={cursor} WHERE mailbox_id={mailbox_id} AND {admission};\n"
            ));
        }
        Event::Rename { display_name } => sql.push_str(&format!(
            "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM mailboxes WHERE mailbox_id={mailbox_id}) THEN 1 ELSE 0 END);\n\
             UPDATE mailboxes SET display_name={} WHERE mailbox_id={mailbox_id} AND {admission};\n",
            sql_text(display_name)
        )),
        Event::Delete => append_delete_sql(sql, mailbox_id),
        Event::SnapshotBegin { uid_validity } => sql.push_str(&format!(
            "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM mailboxes WHERE mailbox_id={mailbox_id} AND uid_validity<>{uid_validity}) THEN 1 ELSE 0 END);\n\
             DELETE FROM staging_messages WHERE mailbox_id={mailbox_id} AND {admission};\n\
             INSERT INTO staging_meta(mailbox_id,uid_validity) SELECT {mailbox_id},{uid_validity} WHERE {admission} \
               ON CONFLICT(mailbox_id) DO UPDATE SET uid_validity=excluded.uid_validity;\n\
             UPDATE mailboxes SET status='needs_repair' WHERE mailbox_id={mailbox_id} AND {admission};\n"
        )),
        Event::SnapshotMessage {
            uid_validity,
            message,
        } => {
            append_message_assertion(sql, message);
            sql.push_str(&format!(
                "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM staging_meta WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity}) THEN 1 ELSE 0 END);\n\
                 INSERT INTO staging_messages(mailbox_id,uid_validity,uid,fingerprint,internal_date,byte_size,flags) \
                 SELECT {mailbox_id},{uid_validity},{},{},{},{},{} WHERE {admission} \
                 ON CONFLICT(mailbox_id,uid_validity,uid) DO UPDATE SET fingerprint=excluded.fingerprint,internal_date=excluded.internal_date,byte_size=excluded.byte_size,flags=excluded.flags;\n",
                message.uid,
                sql_text(&message.fingerprint),
                message.internal_date,
                message.size,
                message.flags
            ));
        }
        Event::SnapshotCommit {
            uid_validity,
            cursor,
        } => sql.push_str(&format!(
            "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM staging_meta s JOIN mailboxes m USING(mailbox_id) WHERE s.mailbox_id={mailbox_id} AND s.uid_validity={uid_validity} AND m.cursor<={cursor}) THEN 1 ELSE 0 END);\n\
             DELETE FROM messages WHERE mailbox_id={mailbox_id} AND {admission};\n\
             INSERT INTO messages(mailbox_id,uid_validity,uid,fingerprint,internal_date,byte_size,flags) \
               SELECT mailbox_id,uid_validity,uid,fingerprint,internal_date,byte_size,flags FROM staging_messages WHERE mailbox_id={mailbox_id} AND uid_validity={uid_validity} AND {admission};\n\
             UPDATE mailboxes SET uid_validity={uid_validity},cursor={cursor},status='synced' WHERE mailbox_id={mailbox_id} AND {admission};\n\
             DELETE FROM staging_messages WHERE mailbox_id={mailbox_id} AND {admission};\n\
             DELETE FROM staging_meta WHERE mailbox_id={mailbox_id} AND {admission};\n"
        )),
    }
}

#[allow(clippy::format_push_string)]
fn append_delete_sql(sql: &mut String, mailbox_id: &str) {
    let admission = "(SELECT apply FROM admission)=1";
    sql.push_str(&format!(
        "INSERT INTO assertion VALUES(CASE WHEN NOT ({admission}) OR EXISTS(SELECT 1 FROM mailboxes WHERE mailbox_id={mailbox_id}) THEN 1 ELSE 0 END);\n\
         DELETE FROM staging_messages WHERE mailbox_id={mailbox_id} AND {admission};\n\
         DELETE FROM mailboxes WHERE mailbox_id={mailbox_id} AND {admission};\n"
    ));
}

#[allow(clippy::format_push_string)]
fn append_active_assertion(sql: &mut String, mailbox_id: &str, epoch: u64, cursor: u64) {
    sql.push_str(&format!(
        "INSERT INTO assertion VALUES(CASE WHEN NOT ((SELECT apply FROM admission)=1) OR EXISTS(SELECT 1 FROM mailboxes WHERE mailbox_id={mailbox_id} AND uid_validity={epoch} AND status='synced' AND cursor<={cursor}) THEN 1 ELSE 0 END);\n"
    ));
}

#[allow(clippy::format_push_string)]
fn append_message_assertion(sql: &mut String, message: &Message) {
    let valid_hex = message.fingerprint.len() == 32
        && message
            .fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit());
    let valid = u8::from(valid_hex && message.flags <= 0x0f);
    sql.push_str(&format!(
        "INSERT INTO assertion VALUES(CASE WHEN NOT ((SELECT apply FROM admission)=1) OR {valid}=1 THEN 1 ELSE 0 END);\n"
    ));
}

fn sql_text(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn validate_batch_integer_range(batch: &Batch) -> Result<(), MirrorError> {
    validate_sqlite_integer(batch.sequence)?;
    for event in &batch.events {
        match event {
            Event::CreateMailbox { uid_validity, .. } | Event::SnapshotBegin { uid_validity } => {
                validate_sqlite_integer(*uid_validity)?;
            }
            Event::Add {
                uid_validity,
                message,
                cursor,
            } => {
                validate_sqlite_integer(*uid_validity)?;
                validate_message_integer_range(message)?;
                validate_sqlite_integer(*cursor)?;
            }
            Event::ReplaceFlags {
                uid_validity,
                uid,
                cursor,
                ..
            }
            | Event::Expunge {
                uid_validity,
                uid,
                cursor,
            } => {
                validate_sqlite_integer(*uid_validity)?;
                validate_sqlite_integer(*uid)?;
                validate_sqlite_integer(*cursor)?;
            }
            Event::SnapshotMessage {
                uid_validity,
                message,
            } => {
                validate_sqlite_integer(*uid_validity)?;
                validate_message_integer_range(message)?;
            }
            Event::SnapshotCommit {
                uid_validity,
                cursor,
            } => {
                validate_sqlite_integer(*uid_validity)?;
                validate_sqlite_integer(*cursor)?;
            }
            Event::Rename { .. } | Event::Delete => {}
        }
    }
    Ok(())
}

fn validate_message_integer_range(message: &Message) -> Result<(), MirrorError> {
    validate_sqlite_integer(message.uid)?;
    validate_sqlite_integer(message.size)
}

fn validate_sqlite_integer(value: u64) -> Result<(), MirrorError> {
    if value <= SQLITE_INTEGER_MAX {
        Ok(())
    } else {
        Err(MirrorError::new("integer outside SQLite INTEGER range"))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxManifest {
    pub mailbox_id: String,
    pub display_name: String,
    pub uid_validity: u64,
    pub cursor: u64,
    pub status: MirrorStatus,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub mailboxes: Vec<MailboxManifest>,
}

impl Manifest {
    #[must_use]
    pub fn live_messages(&self) -> usize {
        self.mailboxes
            .iter()
            .map(|mailbox| mailbox.messages.len())
            .sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratorConfig {
    pub seed: u64,
    pub mailboxes: usize,
    pub live_messages: usize,
    pub responses: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenerationSummary {
    pub seed: u64,
    pub mailboxes: usize,
    pub live_messages: usize,
    pub responses: usize,
    pub duplicate_responses: usize,
    pub epoch_changes: usize,
    pub renames: usize,
    pub deletions_and_recreations: usize,
    pub disconnects: usize,
    pub records: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationSummary {
    pub responses: usize,
    pub duplicate_responses: usize,
    pub batches_applied: usize,
    pub checkpoints: usize,
    pub disconnects: usize,
    pub elapsed_ms: u128,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplySummary {
    pub responses: usize,
    pub duplicate_responses: usize,
    pub batches_applied: usize,
    pub checkpoints: usize,
    pub disconnects: usize,
    pub elapsed_ms: u128,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptDiagnostic {
    pub mailbox_id: String,
    pub sequence: u64,
    pub code: String,
}

impl TranscriptDiagnostic {
    fn new(mailbox_id: impl Into<String>, sequence: u64, code: impl Into<String>) -> Self {
        Self {
            mailbox_id: mailbox_id.into(),
            sequence,
            code: code.into(),
        }
    }
}

impl Display for TranscriptDiagnostic {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{{\"mailbox_id\":{},\"sequence\":{},\"code\":{}}}",
            serde_json::to_string(&self.mailbox_id).unwrap_or_else(|_| "\"unknown\"".to_owned()),
            self.sequence,
            serde_json::to_string(&self.code).unwrap_or_else(|_| "\"diagnostic\"".to_owned())
        )
    }
}

impl std::error::Error for TranscriptDiagnostic {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum TranscriptRecord {
    Begin {
        batch_id: String,
        mailbox_id: String,
        sequence: u64,
    },
    Response {
        event: Event,
    },
    Commit {
        batch_id: String,
    },
    Checkpoint {
        id: String,
        mailbox_id: String,
        selected_uids: Vec<u64>,
    },
    Disconnect,
    End {
        responses: usize,
        records: usize,
    },
}

#[derive(Clone)]
struct GeneratedMailbox {
    id: String,
    name: String,
    epoch: u64,
    sequence: u64,
    messages: Vec<Message>,
}

struct TranscriptWriter<'writer, Writer: Write> {
    writer: &'writer mut Writer,
    records: usize,
    responses: usize,
    next_batch: u64,
}

impl<Writer: Write> TranscriptWriter<'_, Writer> {
    fn record(&mut self, record: &TranscriptRecord) -> Result<(), MirrorError> {
        serde_json::to_writer(&mut *self.writer, record)
            .map_err(|error| MirrorError::new(format!("cannot encode transcript: {error}")))?;
        self.writer
            .write_all(b"\n")
            .map_err(|error| MirrorError::new(format!("cannot write transcript: {error}")))?;
        self.records += 1;
        Ok(())
    }

    fn batch_id(&mut self) -> String {
        let id = format!("batch-{:08}", self.next_batch);
        self.next_batch += 1;
        id
    }

    fn batch(&mut self, batch: &Batch) -> Result<(), MirrorError> {
        self.record(&TranscriptRecord::Begin {
            batch_id: batch.batch_id.clone(),
            mailbox_id: batch.mailbox_id.clone(),
            sequence: batch.sequence,
        })?;
        for event in &batch.events {
            self.record(&TranscriptRecord::Response {
                event: event.clone(),
            })?;
            self.responses += 1;
        }
        self.record(&TranscriptRecord::Commit {
            batch_id: batch.batch_id.clone(),
        })
    }

    fn checkpoint(
        &mut self,
        id: impl Into<String>,
        mailbox: &GeneratedMailbox,
    ) -> Result<(), MirrorError> {
        self.record(&TranscriptRecord::Checkpoint {
            id: id.into(),
            mailbox_id: mailbox.id.clone(),
            selected_uids: mailbox
                .messages
                .iter()
                .take(3)
                .map(|message| message.uid)
                .collect(),
        })
    }
}

/// Writes a deterministic framed NDJSON workload.
///
/// # Errors
///
/// Returns an error for invalid dimensions or failed output.
#[allow(clippy::too_many_lines)]
pub fn generate_transcript<Writer: Write>(
    config: &GeneratorConfig,
    writer: &mut Writer,
) -> Result<GenerationSummary, MirrorError> {
    if config.mailboxes < 2 || config.live_messages == 0 || config.responses < 100 {
        return Err(MirrorError::new("generator dimensions are too small"));
    }
    let duplicate_target = config.responses / 20;
    let unique_target = config.responses - duplicate_target;
    let delete_count = 10.min(config.mailboxes / 2);
    let active_indices: Vec<usize> = (delete_count..config.mailboxes).collect();
    if active_indices.is_empty() {
        return Err(MirrorError::new("generator needs an active mailbox"));
    }
    let mut output = TranscriptWriter {
        writer,
        records: 0,
        responses: 0,
        next_batch: 1,
    };
    let mut mailboxes: Vec<GeneratedMailbox> = (0..config.mailboxes)
        .map(|index| GeneratedMailbox {
            id: format!("mailbox-{index:03}"),
            name: format!("Folder {index:03}"),
            epoch: 1,
            sequence: 0,
            messages: Vec::new(),
        })
        .collect();

    for mailbox in &mut mailboxes {
        mailbox.sequence += 1;
        let batch = Batch::new(
            output.batch_id(),
            mailbox.id.clone(),
            mailbox.sequence,
            vec![Event::CreateMailbox {
                display_name: mailbox.name.clone(),
                uid_validity: mailbox.epoch,
            }],
        );
        output.batch(&batch)?;
    }

    for ordinal in 0..config.live_messages {
        let mailbox_index = active_indices[ordinal % active_indices.len()];
        let mailbox = &mut mailboxes[mailbox_index];
        let uid = u64::try_from(mailbox.messages.len() + 1)
            .map_err(|_| MirrorError::new("message uid overflow"))?;
        mailbox.messages.push(synthetic_message(
            config.seed,
            mailbox_index,
            mailbox.epoch,
            uid,
        ));
    }
    for &mailbox_index in &active_indices {
        let chunks: Vec<Vec<Message>> = mailboxes[mailbox_index]
            .messages
            .chunks(100)
            .map(<[Message]>::to_vec)
            .collect();
        for messages in chunks {
            let mailbox = &mut mailboxes[mailbox_index];
            mailbox.sequence += 1;
            let events = messages
                .into_iter()
                .map(|message| Event::Add {
                    uid_validity: mailbox.epoch,
                    message,
                    cursor: mailbox.sequence,
                })
                .collect();
            let batch = Batch::new(
                output.batch_id(),
                mailbox.id.clone(),
                mailbox.sequence,
                events,
            );
            output.batch(&batch)?;
        }
    }
    output.checkpoint("after-initial-load", &mailboxes[active_indices[0]])?;

    let rename_count = 20.min(active_indices.len());
    for &mailbox_index in active_indices.iter().take(rename_count) {
        let mailbox = &mut mailboxes[mailbox_index];
        mailbox.sequence += 1;
        mailbox.name = format!("Renamed {mailbox_index:03}");
        let batch = Batch::new(
            output.batch_id(),
            mailbox.id.clone(),
            mailbox.sequence,
            vec![Event::Rename {
                display_name: mailbox.name.clone(),
            }],
        );
        output.batch(&batch)?;
    }

    for (index, mailbox) in mailboxes.iter_mut().enumerate().take(delete_count) {
        mailbox.sequence += 1;
        let delete = Batch::new(
            output.batch_id(),
            mailbox.id.clone(),
            mailbox.sequence,
            vec![Event::Delete],
        );
        output.batch(&delete)?;
        mailbox.id = format!("recreated-{index:03}");
        mailbox.epoch = 1;
        mailbox.sequence = 1;
        let create = Batch::new(
            output.batch_id(),
            mailbox.id.clone(),
            mailbox.sequence,
            vec![Event::CreateMailbox {
                display_name: mailbox.name.clone(),
                uid_validity: mailbox.epoch,
            }],
        );
        output.batch(&create)?;
    }

    let epoch_count = 12.min(active_indices.len());
    for (epoch_ordinal, &mailbox_index) in active_indices.iter().take(epoch_count).enumerate() {
        let new_epoch = mailboxes[mailbox_index].epoch + 1;
        {
            let mailbox = &mut mailboxes[mailbox_index];
            mailbox.sequence += 1;
            let begin = Batch::new(
                output.batch_id(),
                mailbox.id.clone(),
                mailbox.sequence,
                vec![Event::SnapshotBegin {
                    uid_validity: new_epoch,
                }],
            );
            output.batch(&begin)?;
        }
        let replacement: Vec<Message> = mailboxes[mailbox_index]
            .messages
            .iter()
            .map(|old| {
                let mut message = synthetic_message(
                    config.seed ^ 0xa5a5_a5a5_a5a5_a5a5,
                    mailbox_index,
                    new_epoch,
                    old.uid,
                );
                message.flags = old.flags ^ SEEN;
                message
            })
            .collect();
        for messages in replacement.chunks(100) {
            let mailbox = &mut mailboxes[mailbox_index];
            mailbox.sequence += 1;
            let events = messages
                .iter()
                .cloned()
                .map(|message| Event::SnapshotMessage {
                    uid_validity: new_epoch,
                    message,
                })
                .collect();
            let stage = Batch::new(
                output.batch_id(),
                mailbox.id.clone(),
                mailbox.sequence,
                events,
            );
            output.batch(&stage)?;
        }
        if epoch_ordinal == 0 {
            output.checkpoint("during-epoch-replacement", &mailboxes[mailbox_index])?;
        }
        {
            let mailbox = &mut mailboxes[mailbox_index];
            mailbox.sequence += 1;
            let commit = Batch::new(
                output.batch_id(),
                mailbox.id.clone(),
                mailbox.sequence,
                vec![Event::SnapshotCommit {
                    uid_validity: new_epoch,
                    cursor: mailbox.sequence,
                }],
            );
            output.batch(&commit)?;
            mailbox.epoch = new_epoch;
            mailbox.messages = replacement;
        }
    }
    output.checkpoint("after-epoch-replacement", &mailboxes[active_indices[0]])?;

    let disconnected_mailbox = &mut mailboxes[active_indices[0]];
    disconnected_mailbox.sequence += 1;
    let disconnected_batch_id = output.batch_id();
    output.record(&TranscriptRecord::Begin {
        batch_id: disconnected_batch_id,
        mailbox_id: disconnected_mailbox.id.clone(),
        sequence: disconnected_mailbox.sequence,
    })?;
    output.record(&TranscriptRecord::Response {
        event: Event::Rename {
            display_name: "must-not-be-visible".to_owned(),
        },
    })?;
    output.responses += 1;
    output.record(&TranscriptRecord::Disconnect)?;

    if output.responses > unique_target {
        return Err(MirrorError::new(
            "response budget is too small for required scenarios",
        ));
    }
    let mut duplicate_batches = Vec::new();
    let mut duplicate_reserved = 0_usize;
    let mut fill_ordinal = 0_usize;
    while output.responses < unique_target {
        let remaining = unique_target - output.responses;
        let duplicate_remaining = duplicate_target.saturating_sub(duplicate_reserved);
        let size = 100.min(remaining).min(if duplicate_remaining == 0 {
            100
        } else {
            duplicate_remaining
        });
        let mailbox_index = active_indices[fill_ordinal % active_indices.len()];
        let mailbox = &mut mailboxes[mailbox_index];
        mailbox.sequence += 1;
        let events = (0..size)
            .map(|offset| {
                let message_index = (fill_ordinal + offset) % mailbox.messages.len();
                let message = &mut mailbox.messages[message_index];
                message.flags ^= SEEN;
                Event::ReplaceFlags {
                    uid_validity: mailbox.epoch,
                    uid: message.uid,
                    flags: message.flags,
                    cursor: mailbox.sequence,
                }
            })
            .collect();
        let batch = Batch::new(
            output.batch_id(),
            mailbox.id.clone(),
            mailbox.sequence,
            events,
        );
        if duplicate_reserved < duplicate_target {
            duplicate_reserved += batch.events.len();
            duplicate_batches.push(batch.clone());
        }
        output.batch(&batch)?;
        fill_ordinal += 1;
    }
    for batch in &duplicate_batches {
        output.batch(batch)?;
    }
    output.checkpoint("final-a", &mailboxes[active_indices[0]])?;
    output.checkpoint(
        "final-b",
        &mailboxes[active_indices[active_indices.len() / 2]],
    )?;

    if output.responses != config.responses || duplicate_reserved != duplicate_target {
        return Err(MirrorError::new(
            "generator failed to meet exact response budget",
        ));
    }
    let final_record_count = output.records + 1;
    output.record(&TranscriptRecord::End {
        responses: output.responses,
        records: final_record_count,
    })?;
    Ok(GenerationSummary {
        seed: config.seed,
        mailboxes: config.mailboxes,
        live_messages: config.live_messages,
        responses: output.responses,
        duplicate_responses: duplicate_reserved,
        epoch_changes: epoch_count,
        renames: rename_count,
        deletions_and_recreations: delete_count,
        disconnects: 1,
        records: output.records,
    })
}

/// Applies a transcript to the reference and `SQLite` baseline and compares every checkpoint.
///
/// # Errors
///
/// Returns a privacy-safe diagnostic when framing, admission, or state diverges.
#[allow(clippy::too_many_lines)]
pub fn verify_transcript<Reader: BufRead>(
    reader: Reader,
    database: &Path,
) -> Result<VerificationSummary, TranscriptDiagnostic> {
    let started = Instant::now();
    let mut sqlite = SqliteBaseline::create(database)
        .map_err(|_| TranscriptDiagnostic::new("unknown", 0, "sqlite_open_failed"))?;
    let mut reference = ReferenceMirror::default();
    let mut pending: Option<Batch> = None;
    let mut responses = 0_usize;
    let mut duplicate_responses = 0_usize;
    let mut batches_applied = 0_usize;
    let mut checkpoints = 0_usize;
    let mut disconnects = 0_usize;
    let mut records_seen = 0_usize;
    let mut saw_end = false;
    for line in reader.lines() {
        if saw_end {
            return Err(TranscriptDiagnostic::new("unknown", 0, "record_after_end"));
        }
        let line = line.map_err(|_| pending_diagnostic(pending.as_ref(), "input_read_failed"))?;
        let record: TranscriptRecord = serde_json::from_str(&line)
            .map_err(|_| pending_diagnostic(pending.as_ref(), "invalid_json"))?;
        records_seen += 1;
        match record {
            TranscriptRecord::Begin {
                batch_id,
                mailbox_id,
                sequence,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(pending.as_ref(), "nested_batch"));
                }
                pending = Some(Batch::new(batch_id, mailbox_id, sequence, Vec::new()));
            }
            TranscriptRecord::Response { event } => {
                let batch = pending.as_mut().ok_or_else(|| {
                    TranscriptDiagnostic::new("unknown", 0, "response_without_batch")
                })?;
                batch.events.push(event);
                responses += 1;
            }
            TranscriptRecord::Commit { batch_id } => {
                let batch = pending.take().ok_or_else(|| {
                    TranscriptDiagnostic::new("unknown", 0, "commit_without_batch")
                })?;
                if batch.batch_id != batch_id {
                    return Err(TranscriptDiagnostic::new(
                        batch.mailbox_id,
                        batch.sequence,
                        "batch_id_mismatch",
                    ));
                }
                let reference_outcome = reference.apply(&batch).map_err(|_| {
                    TranscriptDiagnostic::new(
                        &batch.mailbox_id,
                        batch.sequence,
                        "reference_rejected_batch",
                    )
                })?;
                let sqlite_outcome = sqlite.apply(&batch).map_err(|_| {
                    TranscriptDiagnostic::new(
                        &batch.mailbox_id,
                        batch.sequence,
                        "sqlite_rejected_batch",
                    )
                })?;
                if reference_outcome != sqlite_outcome {
                    return Err(TranscriptDiagnostic::new(
                        batch.mailbox_id,
                        batch.sequence,
                        "admission_mismatch",
                    ));
                }
                if sqlite_outcome == ApplyOutcome::Duplicate {
                    duplicate_responses += batch.events.len();
                } else {
                    batches_applied += 1;
                }
            }
            TranscriptRecord::Checkpoint {
                id: _,
                mailbox_id,
                selected_uids,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(
                        pending.as_ref(),
                        "checkpoint_inside_batch",
                    ));
                }
                let expected = reference
                    .checkpoint(&mailbox_id, &selected_uids)
                    .map_err(|_| {
                        TranscriptDiagnostic::new(&mailbox_id, 0, "reference_checkpoint_failed")
                    })?;
                let actual = sqlite
                    .checkpoint(&mailbox_id, &selected_uids)
                    .map_err(|_| {
                        TranscriptDiagnostic::new(&mailbox_id, 0, "sqlite_checkpoint_failed")
                    })?;
                if actual != expected {
                    return Err(TranscriptDiagnostic::new(
                        mailbox_id,
                        0,
                        "checkpoint_mismatch",
                    ));
                }
                checkpoints += 1;
            }
            TranscriptRecord::Disconnect => {
                if pending.take().is_none() {
                    return Err(TranscriptDiagnostic::new(
                        "unknown",
                        0,
                        "disconnect_without_batch",
                    ));
                }
                disconnects += 1;
            }
            TranscriptRecord::End {
                responses: expected_responses,
                records: expected_records,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(pending.as_ref(), "end_inside_batch"));
                }
                if responses != expected_responses || records_seen != expected_records {
                    return Err(TranscriptDiagnostic::new(
                        "unknown",
                        0,
                        "transcript_count_mismatch",
                    ));
                }
                saw_end = true;
            }
        }
    }
    if pending.is_some() {
        return Err(pending_diagnostic(
            pending.as_ref(),
            "missing_batch_terminator",
        ));
    }
    if !saw_end {
        return Err(TranscriptDiagnostic::new("unknown", 0, "truncated_input"));
    }
    let expected = reference.manifest();
    let actual = sqlite
        .manifest()
        .map_err(|_| TranscriptDiagnostic::new("unknown", 0, "sqlite_manifest_failed"))?;
    if actual != expected {
        return Err(TranscriptDiagnostic::new("unknown", 0, "manifest_mismatch"));
    }
    Ok(VerificationSummary {
        responses,
        duplicate_responses,
        batches_applied,
        checkpoints,
        disconnects,
        elapsed_ms: started.elapsed().as_millis(),
        manifest: expected,
    })
}

/// Applies or resumes a transcript against the `SQLite` baseline.
///
/// # Errors
///
/// Returns a privacy-safe diagnostic when framing or `SQLite` admission fails.
#[allow(clippy::too_many_lines)]
pub fn apply_transcript<Reader: BufRead>(
    reader: Reader,
    database: &Path,
    resume: bool,
) -> Result<ApplySummary, TranscriptDiagnostic> {
    let started = Instant::now();
    let mut sqlite = if resume {
        SqliteBaseline::open(database)
    } else {
        SqliteBaseline::create(database)
    }
    .map_err(|_| TranscriptDiagnostic::new("unknown", 0, "sqlite_open_failed"))?;
    let mut pending: Option<Batch> = None;
    let mut responses = 0_usize;
    let mut duplicate_responses = 0_usize;
    let mut batches_applied = 0_usize;
    let mut checkpoints = 0_usize;
    let mut disconnects = 0_usize;
    let mut records_seen = 0_usize;
    let mut saw_end = false;
    for line in reader.lines() {
        if saw_end {
            return Err(TranscriptDiagnostic::new("unknown", 0, "record_after_end"));
        }
        let line = line.map_err(|_| pending_diagnostic(pending.as_ref(), "input_read_failed"))?;
        let record: TranscriptRecord = serde_json::from_str(&line)
            .map_err(|_| pending_diagnostic(pending.as_ref(), "invalid_json"))?;
        records_seen += 1;
        match record {
            TranscriptRecord::Begin {
                batch_id,
                mailbox_id,
                sequence,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(pending.as_ref(), "nested_batch"));
                }
                pending = Some(Batch::new(batch_id, mailbox_id, sequence, Vec::new()));
            }
            TranscriptRecord::Response { event } => {
                let batch = pending.as_mut().ok_or_else(|| {
                    TranscriptDiagnostic::new("unknown", 0, "response_without_batch")
                })?;
                batch.events.push(event);
                responses += 1;
            }
            TranscriptRecord::Commit { batch_id } => {
                let batch = pending.take().ok_or_else(|| {
                    TranscriptDiagnostic::new("unknown", 0, "commit_without_batch")
                })?;
                if batch.batch_id != batch_id {
                    return Err(TranscriptDiagnostic::new(
                        batch.mailbox_id,
                        batch.sequence,
                        "batch_id_mismatch",
                    ));
                }
                let outcome = sqlite.apply(&batch).map_err(|_| {
                    TranscriptDiagnostic::new(
                        &batch.mailbox_id,
                        batch.sequence,
                        "sqlite_rejected_batch",
                    )
                })?;
                if outcome == ApplyOutcome::Duplicate {
                    duplicate_responses += batch.events.len();
                } else {
                    batches_applied += 1;
                }
            }
            TranscriptRecord::Checkpoint {
                id: _,
                mailbox_id,
                selected_uids,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(
                        pending.as_ref(),
                        "checkpoint_inside_batch",
                    ));
                }
                sqlite
                    .checkpoint(&mailbox_id, &selected_uids)
                    .map_err(|_| {
                        TranscriptDiagnostic::new(&mailbox_id, 0, "sqlite_checkpoint_failed")
                    })?;
                checkpoints += 1;
            }
            TranscriptRecord::Disconnect => {
                if pending.take().is_none() {
                    return Err(TranscriptDiagnostic::new(
                        "unknown",
                        0,
                        "disconnect_without_batch",
                    ));
                }
                disconnects += 1;
            }
            TranscriptRecord::End {
                responses: expected_responses,
                records: expected_records,
            } => {
                if pending.is_some() {
                    return Err(pending_diagnostic(pending.as_ref(), "end_inside_batch"));
                }
                if responses != expected_responses || records_seen != expected_records {
                    return Err(TranscriptDiagnostic::new(
                        "unknown",
                        0,
                        "transcript_count_mismatch",
                    ));
                }
                saw_end = true;
            }
        }
    }
    if pending.is_some() {
        return Err(pending_diagnostic(
            pending.as_ref(),
            "missing_batch_terminator",
        ));
    }
    if !saw_end {
        return Err(TranscriptDiagnostic::new("unknown", 0, "truncated_input"));
    }
    let manifest = sqlite
        .manifest()
        .map_err(|_| TranscriptDiagnostic::new("unknown", 0, "sqlite_manifest_failed"))?;
    Ok(ApplySummary {
        responses,
        duplicate_responses,
        batches_applied,
        checkpoints,
        disconnects,
        elapsed_ms: started.elapsed().as_millis(),
        manifest,
    })
}

fn pending_diagnostic(pending: Option<&Batch>, code: &str) -> TranscriptDiagnostic {
    pending.map_or_else(
        || TranscriptDiagnostic::new("unknown", 0, code),
        |batch| TranscriptDiagnostic::new(&batch.mailbox_id, batch.sequence, code),
    )
}

fn synthetic_message(seed: u64, mailbox: usize, epoch: u64, uid: u64) -> Message {
    let mailbox = u64::try_from(mailbox).unwrap_or(u64::MAX);
    let left = mix(seed ^ mailbox.rotate_left(17) ^ epoch.rotate_left(31) ^ uid);
    let right = mix(left ^ 0x9e37_79b9_7f4a_7c15);
    Message {
        uid,
        fingerprint: format!("{left:016x}{right:016x}"),
        internal_date: 1_700_000_000 + i64::try_from(uid % 10_000_000).unwrap_or(0),
        size: 256 + (right % 65_536),
        flags: u8::from(left.trailing_zeros() >= 2),
    }
}

fn mix(mut value: u64) -> u64 {
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
