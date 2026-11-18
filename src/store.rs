use rmcp::schemars;
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    List,
    Read,
    CreateSpace,
    UpdateSpace,
    DeleteSpace,
    CreatePanel,
    UpdatePanel,
    DeletePanel,
    SetData,
    DeleteData,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct Command {
    pub action: Action,
    pub space_id: Option<String>,
    pub panel_id: Option<String>,
    pub name: Option<String>,
    pub specification: Option<String>,
    pub html: Option<String>,
    pub key: Option<String>,
    pub value: Option<Value>,
    /// Supply the revision from read to reject a stale write rather than overwrite newer work.
    pub expected_revision: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Panel {
    pub id: String,
    pub name: String,
    pub specification: String,
    pub html: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Space {
    pub id: String,
    pub name: String,
    pub revision: u64,
    pub panels: Vec<Panel>,
    pub data: serde_json::Map<String, Value>,
}
#[derive(Clone)]
pub struct Store(Arc<Mutex<Connection>>);
fn required(value: &Option<String>, label: &str) -> Result<String, String> {
    value
        .as_ref()
        .filter(|s| !s.trim().is_empty())
        .cloned()
        .ok_or_else(|| format!("{label} is required"))
}
impl Store {
    pub fn open(path: &str) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS spaces (id TEXT PRIMARY KEY, document TEXT NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(db))))
    }
    pub fn execute(&self, c: Command) -> Result<Value, String> {
        let mut db = self.0.lock().map_err(|_| "Database lock failed")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let result = (|| -> Result<Value, String> {
            if matches!(c.action, Action::List) {
                let mut stmt = tx
                    .prepare("SELECT document FROM spaces ORDER BY rowid")
                    .map_err(|e| e.to_string())?;
                let rows = stmt
                    .query_map([], |row| row.get::<_, String>(0))
                    .map_err(|e| e.to_string())?;
                let mut spaces = Vec::new();
                for row in rows {
                    let s: Space = serde_json::from_str(&row.map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                    spaces.push(json!({"id":s.id,"name":s.name,"revision":s.revision,"panel_count":s.panels.len()}));
                }
                return Ok(json!(spaces));
            }
            if matches!(c.action, Action::CreateSpace) {
                let s = Space {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: required(&c.name, "name")?,
                    revision: 1,
                    panels: vec![],
                    data: Default::default(),
                };
                tx.execute(
                    "INSERT INTO spaces VALUES (?1, ?2)",
                    params![s.id, serde_json::to_string(&s).unwrap()],
                )
                .map_err(|e| e.to_string())?;
                return Ok(json!(s));
            }
            let id = required(&c.space_id, "space_id")?;
            let raw: String = tx
                .query_row("SELECT document FROM spaces WHERE id=?1", [&id], |row| {
                    row.get(0)
                })
                .map_err(|_| "Space not found")?;
            let mut s: Space = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            if matches!(c.action, Action::Read) {
                return Ok(json!(s));
            }
            if c.expected_revision.is_some_and(|r| r != s.revision) {
                return Err("Conflict: space changed; read it again before retrying".into());
            }
            match c.action {
                Action::DeleteSpace => {
                    tx.execute("DELETE FROM spaces WHERE id=?1", [&id])
                        .map_err(|e| e.to_string())?;
                    return Ok(json!({"deleted":id}));
                }
                Action::UpdateSpace => s.name = required(&c.name, "name")?,
                Action::CreatePanel => s.panels.push(Panel {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: required(&c.name, "name")?,
                    specification: required(&c.specification, "specification")?,
                    html: c.html.ok_or("html is required")?,
                }),
                Action::UpdatePanel | Action::DeletePanel => {
                    let panel_id = required(&c.panel_id, "panel_id")?;
                    let pos = s
                        .panels
                        .iter()
                        .position(|p| p.id == panel_id)
                        .ok_or("Panel not found in this space")?;
                    if matches!(c.action, Action::DeletePanel) {
                        s.panels.remove(pos);
                    } else {
                        if c.name.is_some() {
                            s.panels[pos].name = required(&c.name, "name")?;
                        }
                        if c.specification.is_some() {
                            s.panels[pos].specification =
                                required(&c.specification, "specification")?;
                        }
                        if let Some(html) = c.html {
                            s.panels[pos].html = html;
                        }
                    }
                }
                Action::SetData => {
                    s.data
                        .insert(required(&c.key, "key")?, c.value.unwrap_or(Value::Null));
                }
                Action::DeleteData => {
                    s.data.remove(&required(&c.key, "key")?);
                }
                _ => unreachable!(),
            }
            s.revision += 1;
            tx.execute(
                "UPDATE spaces SET document=?1 WHERE id=?2",
                params![serde_json::to_string(&s).unwrap(), id],
            )
            .map_err(|e| e.to_string())?;
            Ok(json!(s))
        })();
        if result.is_ok() {
            tx.commit().map_err(|e| e.to_string())?;
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cmd(v: Value) -> Command {
        serde_json::from_value(v).unwrap()
    }
    #[test]
    fn isolation_and_stale_writes() {
        let db = Store::open(":memory:").unwrap();
        let a = db
            .execute(cmd(json!({"action":"create_space","name":"A"})))
            .unwrap();
        let b = db
            .execute(cmd(json!({"action":"create_space","name":"B"})))
            .unwrap();
        let a = db.execute(cmd(json!({"action":"create_panel","space_id":a["id"],"name":"Panel","specification":"Purpose","html":"Hi"}))).unwrap();
        assert!(db.execute(cmd(json!({"action":"update_panel","space_id":b["id"],"panel_id":a["panels"][0]["id"],"html":"Cross-space"}))).is_err());
        db.execute(cmd(json!({"action":"set_data","space_id":a["id"],"key":"score","value":2,"expected_revision":2}))).unwrap();
        assert!(db.execute(cmd(json!({"action":"set_data","space_id":a["id"],"key":"score","value":3,"expected_revision":2}))).is_err());
        let b = db
            .execute(cmd(json!({"action":"read","space_id":b["id"]})))
            .unwrap();
        assert_eq!(b["data"], json!({}));
    }
    #[test]
    fn persistence_and_delete() {
        let path = std::env::temp_dir().join(format!("panelcraft-{}.db", uuid::Uuid::new_v4()));
        let id;
        {
            let db = Store::open(path.to_str().unwrap()).unwrap();
            id = db
                .execute(cmd(json!({"action":"create_space","name":"Saved"})))
                .unwrap()["id"]
                .clone();
        }
        {
            let db = Store::open(path.to_str().unwrap()).unwrap();
            assert_eq!(
                db.execute(cmd(json!({"action":"read","space_id":id})))
                    .unwrap()["name"],
                "Saved"
            );
            db.execute(cmd(json!({"action":"delete_space","space_id":id})))
                .unwrap();
            assert!(
                db.execute(cmd(json!({"action":"read","space_id":id})))
                    .is_err()
            );
        }
        std::fs::remove_file(path).unwrap();
    }
}
