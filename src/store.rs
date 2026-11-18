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
    SetLayout,
    ReorderSpaces,
    ReorderPanels,
    DeleteSpace,
    CreatePanel,
    UpdatePanel,
    DeletePanel,
    ApplyRefresh,
    SetData,
    DeleteData,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
pub struct Command {
    pub action: Action,
    pub layout: Option<Layout>,
    pub presentation: Option<Presentation>,
    /// Full ordered IDs before and after a reorder.
    pub previous_order: Option<Vec<String>>,
    pub order: Option<Vec<String>>,
    pub space_id: Option<String>,
    pub panel_id: Option<String>,
    pub name: Option<String>,
    pub specification: Option<String>,
    pub html: Option<String>,
    /// Optional async JS function body receiving data and html. An empty string removes it.
    pub update_script: Option<String>,
    pub key: Option<String>,
    pub value: Option<Value>,
    /// Supply the revision from read to reject a stale write rather than overwrite newer work.
    pub expected_revision: Option<u64>,
}

impl Command {
    fn checked_order(&self, current: &[String]) -> Result<&[String], String> {
        if self.previous_order.as_deref() != Some(current) {
            return Err("Conflict: order changed; try again".into());
        }
        let order = self.order.as_deref().ok_or("order is required")?;
        let unique: std::collections::HashSet<_> = order.iter().collect();
        if unique.len() != current.len()
            || order.len() != current.len()
            || !current.iter().all(|id| unique.contains(id))
        {
            return Err("Order must include every current ID exactly once".into());
        }
        Ok(order)
    }
}
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Shape { #[default] Auto, Landscape, Portrait }
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema, Debug)]
pub struct Presentation {
    #[serde(default)]
    pub shape: Shape,
    pub max_width: Option<u16>,
    pub max_height: Option<u16>,
}
#[derive(Clone, Default, Serialize, Deserialize, schemars::JsonSchema, Debug)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Layout {
    #[default] Auto,
    Balanced { percent: u8 },
    Right { panel_id: String, percent: u8 },
    Bottom { panel_id: String, percent: u8 },
}
impl Layout {
    fn feature(&self) -> Option<&str> {
        match self { Self::Right {panel_id,..} | Self::Bottom {panel_id,..} => Some(panel_id), _ => None }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Panel {
    #[serde(default)]
    pub presentation: Presentation,
    #[serde(default)]
    pub updated_revision: u64,
    pub id: String,
    pub name: String,
    pub specification: String,
    pub html: String,
    #[serde(default)]
    pub update_script: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Space {
    #[serde(default)]
    pub layout: Layout,
    pub id: String,
    pub name: String,
    pub revision: u64,
    #[serde(default)]
    pub data_revision: u64,
    #[serde(default)]
    pub structure_revision: u64,
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
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS spaces (id TEXT PRIMARY KEY, document TEXT NOT NULL); CREATE TABLE IF NOT EXISTS space_order (id TEXT PRIMARY KEY, position INTEGER NOT NULL);").map_err(|e|e.to_string())?;
        Ok(Self(Arc::new(Mutex::new(db))))
    }
    pub fn execute(&self, c: Command) -> Result<Value, String> {
        let mut db = self.0.lock().map_err(|_| "Database lock failed")?;
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let result = (|| -> Result<Value, String> {
            if matches!(c.action, Action::ReorderSpaces) {
                let mut stmt = tx.prepare("SELECT s.id FROM spaces s LEFT JOIN space_order o USING(id) ORDER BY COALESCE(o.position,s.rowid)").map_err(|e| e.to_string())?;
                let ids = stmt
                    .query_map([], |row| row.get(0))
                    .map_err(|e| e.to_string())?
                    .collect::<Result<Vec<String>, _>>()
                    .map_err(|e| e.to_string())?;
                for (index, id) in c.checked_order(&ids)?.iter().enumerate() {
                    tx.execute(
                        "INSERT OR REPLACE INTO space_order VALUES(?1,?2)",
                        params![id, index as i64 + 1],
                    )
                    .map_err(|e| e.to_string())?;
                }
                return Ok(json!({"saved":true}));
            }
            if matches!(c.action, Action::List) {
                let mut stmt = tx
                    .prepare("SELECT document FROM spaces s LEFT JOIN space_order o USING(id) ORDER BY COALESCE(o.position,s.rowid)")
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
                    layout: Layout::default(),
                    id: uuid::Uuid::new_v4().to_string(),
                    name: required(&c.name, "name")?,
                    revision: 1,
                    data_revision: 0,
                    structure_revision: 1,
                    panels: vec![],
                    data: Default::default(),
                };
                tx.execute(
                    "INSERT INTO spaces VALUES (?1, ?2)",
                    params![s.id, serde_json::to_string(&s).unwrap()],
                )
                .map_err(|e| e.to_string())?;
                tx.execute("INSERT INTO space_order SELECT ?1, COALESCE(MAX(COALESCE(o.position,s.rowid)),0)+1 FROM spaces s LEFT JOIN space_order o USING(id) WHERE s.id != ?1", [&s.id]).map_err(|e|e.to_string())?;
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
            if matches!(c.action, Action::ReorderPanels | Action::DeletePanel | Action::UpdateSpace) {
                s.structure_revision = s.revision + 1;
            }
            if let Some(p) = &c.presentation {
                if p.max_width.is_some_and(|w|w<240) || p.max_height.is_some_and(|h|h<240) {
                    return Err("Panel limits must be at least 240 pixels".into());
                }
            }
            match c.action {
                Action::SetLayout => {
                    let layout = c.layout.ok_or("layout is required")?;
                    if layout.feature().is_some_and(|id|!s.panels.iter().any(|p|p.id==id)) {
                        return Err("Featured panel not found in this space".into());
                    }
                    match &layout {
                        Layout::Right{percent,..}|Layout::Bottom{percent,..}|Layout::Balanced{percent} if !(20..=80).contains(percent) => return Err("Split must be between 20 and 80 percent".into()),
                        _ => {}
                    }
                    s.layout = layout;
                    s.structure_revision = s.revision + 1;
                }
                Action::ReorderPanels => {
                    let ids = s.panels.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
                    let order = c.checked_order(&ids)?;
                    s.panels
                        .sort_by_key(|p| order.iter().position(|id| id == &p.id));
                }
                Action::DeleteSpace => {
                    tx.execute("DELETE FROM space_order WHERE id=?1", [&id])
                        .map_err(|e| e.to_string())?;
                    tx.execute("DELETE FROM spaces WHERE id=?1", [&id])
                        .map_err(|e| e.to_string())?;
                    return Ok(json!({"deleted":id}));
                }
                Action::UpdateSpace => s.name = required(&c.name, "name")?,
                Action::CreatePanel => s.panels.push(Panel {
                    presentation: c.presentation.unwrap_or_default(),
                    updated_revision: s.revision + 1,
                    id: uuid::Uuid::new_v4().to_string(),
                    name: required(&c.name, "name")?,
                    specification: required(&c.specification, "specification")?,
                    html: c.html.ok_or("html is required")?,
                    update_script: c.update_script.unwrap_or_default(),
                }),
                Action::UpdatePanel | Action::DeletePanel => {
                    let panel_id = required(&c.panel_id, "panel_id")?;
                    let pos = s
                        .panels
                        .iter()
                        .position(|p| p.id == panel_id)
                        .ok_or("Panel not found in this space")?;
                    if matches!(c.action, Action::DeletePanel) {
                        if s.layout.feature() == Some(panel_id.as_str()) { s.layout = Layout::Auto; }
                        s.panels.remove(pos);
                    } else {
                        s.panels[pos].updated_revision = s.revision + 1;
                        if let Some(p) = c.presentation { s.panels[pos].presentation = p; }
                        if c.name.is_some() {
                            s.panels[pos].name = required(&c.name, "name")?;
                        }
                        if c.specification.is_some() {
                            s.panels[pos].specification =
                                required(&c.specification, "specification")?;
                        }
                        if let Some(script) = c.update_script {
                            s.panels[pos].update_script = script;
                        }
                        if let Some(html) = c.html {
                            s.panels[pos].html = html;
                        }
                    }
                }
                Action::ApplyRefresh => {
                    let id = required(&c.panel_id,"panel_id")?;
                    let panel = s.panels.iter_mut().find(|p|p.id==id).ok_or("Panel not found in this space")?;
                    if c.expected_revision.is_none() { return Err("A refresh requires expected_revision".into()); }
                    let patch = match c.value.as_ref() {
                        Some(v) => Some(v.as_object().ok_or("setData must be an object")?),
                        None => None,
                    };
                    if c.html.is_none() && patch.is_none() { return Err("Missing refresh result".into()); }
                    let mut changed = false;
                    if let Some(html) = c.html {
                        if html != panel.html { panel.html = html; changed = true; }
                    }
                    if let Some(patch) = patch {
                        for (key,value) in patch {
                            if key.trim().is_empty() { return Err("Shared-data keys must be nonempty".into()); }
                            if s.data.get(key) != Some(value) {
                                s.data.insert(key.clone(),value.clone());
                                s.data_revision = s.revision + 1;
                                changed = true;
                            }
                        }
                    }
                    if !changed { return Ok(json!({"changed":false})); }
                    panel.updated_revision = s.revision + 1;
                }
                Action::SetData => {
                    s.data_revision = s.revision + 1;
                    s.data
                        .insert(required(&c.key, "key")?, c.value.unwrap_or(Value::Null));
                }
                Action::DeleteData => {
                    s.data_revision = s.revision + 1;
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
