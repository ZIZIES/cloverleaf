// plain structs for the bits of the opencode api we care about, parsed by hand out of
// serde_json::Value. the api is marked experimental upstream, so missing fields fall back
// to defaults instead of failing the whole parse.

use serde_json::{Value, json};

fn st(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_owned()
}
fn opt(v: &Value, k: &str) -> Option<String> {
    v[k].as_str().map(str::to_owned)
}
fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

#[derive(Clone, PartialEq, Debug)]
pub struct ModelRef {
    pub id: String,
    pub provider: String,
    pub variant: Option<String>,
}

impl ModelRef {
    pub fn parse(v: &Value) -> Option<ModelRef> {
        Some(ModelRef { id: v["id"].as_str()?.to_owned(), provider: v["providerID"].as_str()?.to_owned(), variant: opt(v, "variant") })
    }
    pub fn to_json(&self) -> Value {
        let mut j = json!({ "id": self.id, "providerID": self.provider });
        if let Some(var) = &self.variant {
            j["variant"] = json!(var);
        }
        j
    }
}

#[derive(Clone)]
pub struct Session {
    pub id: String,
    pub title: String,
    pub dir: String,
    pub updated: f64, // unix ms
    pub cost: f64,
    pub agent: Option<String>,
    pub model: Option<ModelRef>,
    pub running: bool,
}

impl Session {
    pub fn parse(v: &Value) -> Option<Session> {
        let t = &v["time"];
        Some(Session {
            id: v["id"].as_str()?.to_owned(),
            title: st(v, "title"),
            dir: st(&v["location"], "directory"),
            updated: num(&t["updated"]),
            cost: num(&v["cost"]),
            agent: opt(v, "agent"),
            model: ModelRef::parse(&v["model"]),
            running: num(&t["busy"]) > num(&t["idle"]),
        })
    }
    pub fn label(&self) -> &str {
        if self.title.is_empty() { "New session" } else { &self.title }
    }
}

#[derive(Clone)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub name: String,
    pub variants: Vec<String>,
    pub context: u64,
}

impl ModelInfo {
    pub fn parse(v: &Value) -> Option<ModelInfo> {
        if v["enabled"].as_bool() == Some(false) {
            return None;
        }
        if let Some(t) = v["type"].as_str()
            && t != "chat"
        {
            return None;
        }
        let id = v["id"].as_str()?.to_owned();
        let name = v["name"].as_str().unwrap_or(&id).to_owned();
        let variants = v["variants"].as_array().map(|a| a.iter().filter_map(|x| x["id"].as_str().map(str::to_owned)).collect()).unwrap_or_default();
        Some(ModelInfo { id, provider: st(v, "providerID"), name, variants, context: v["limit"]["context"].as_u64().unwrap_or(0) })
    }
}

#[derive(Clone)]
pub struct AgentInfo {
    pub id: String,
    pub description: String,
}

impl AgentInfo {
    // only agents you can actually pick: primary (or "all") and not hidden.
    pub fn parse(v: &Value) -> Option<AgentInfo> {
        if v["hidden"].as_bool() == Some(true) || v["mode"].as_str() == Some("subagent") {
            return None;
        }
        Some(AgentInfo { id: v["id"].as_str()?.to_owned(), description: st(v, "description") })
    }
}

#[derive(Clone)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
}

impl CommandInfo {
    pub fn parse(v: &Value) -> Option<CommandInfo> {
        Some(CommandInfo { name: v["name"].as_str()?.to_owned(), description: st(v, "description") })
    }
}

#[derive(Clone)]
pub struct Permission {
    pub id: String,
    pub session: String,
    pub action: String,
    pub resources: Vec<String>,
    pub message: String,
}

impl Permission {
    pub fn parse(v: &Value) -> Option<Permission> {
        Some(Permission {
            id: v["id"].as_str()?.to_owned(),
            session: st(v, "sessionID"),
            action: st(v, "action"),
            resources: v["resources"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect()).unwrap_or_default(),
            message: st(v, "message"),
        })
    }
}

// a form is how the agent asks you a multiple-choice question mid-run (the `question` tool).
#[derive(Clone)]
pub struct Form {
    pub id: String,
    pub session: String,
    pub title: String,
    pub fields: Vec<Field>,
}

#[derive(Clone)]
pub struct Field {
    pub key: String,
    pub title: String,
    pub description: String,
    pub kind: String, // string | number | integer | boolean | multiselect | external
    pub options: Vec<FieldOption>,
    pub custom: bool,
    pub url: String,
}

#[derive(Clone)]
pub struct FieldOption {
    pub value: String,
    pub label: String,
    pub description: String,
}

impl Form {
    pub fn parse(v: &Value) -> Option<Form> {
        let fields = v["fields"]
            .as_array()?
            .iter()
            .filter(|f| f["hidden"].as_bool() != Some(true))
            .filter_map(|f| {
                Some(Field {
                    key: f["key"].as_str()?.to_owned(),
                    title: st(f, "title"),
                    description: st(f, "description"),
                    kind: st(f, "type"),
                    options: f["options"]
                        .as_array()
                        .map(|a| a.iter().map(|o| FieldOption { value: st(o, "value"), label: st(o, "label"), description: st(o, "description") }).collect())
                        .unwrap_or_default(),
                    custom: f["custom"].as_bool().unwrap_or(false),
                    url: st(f, "url"),
                })
            })
            .collect();
        Some(Form { id: v["id"].as_str()?.to_owned(), session: st(v, "sessionID"), title: st(v, "title"), fields })
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum ToolStatus {
    Streaming, // model is still writing the arguments
    Running,
    Done,
    Error,
}

#[derive(Clone)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub status: ToolStatus,
    pub input: Value,
    pub raw_input: String, // partial argument json while Streaming
    pub output: String,
    pub error: String,
    pub meta: Value,
    pub created: f64,
    pub completed: f64,
}

impl Tool {
    pub fn new(id: &str, name: &str) -> Tool {
        Tool {
            id: id.to_owned(),
            name: name.to_owned(),
            status: ToolStatus::Streaming,
            input: Value::Null,
            raw_input: String::new(),
            output: String::new(),
            error: String::new(),
            meta: Value::Null,
            created: 0.0,
            completed: 0.0,
        }
    }

    fn parse(v: &Value) -> Tool {
        let s = &v["state"];
        let mut t = Tool::new(v["id"].as_str().unwrap_or(""), v["name"].as_str().unwrap_or("tool"));
        t.status = match s["status"].as_str() {
            Some("streaming") => ToolStatus::Streaming,
            Some("running") => ToolStatus::Running,
            Some("error") => ToolStatus::Error,
            _ => ToolStatus::Done,
        };
        match &s["input"] {
            Value::String(raw) => t.raw_input = raw.clone(),
            other => t.input = other.clone(),
        }
        t.output = content_text(&s["content"]);
        t.error = st(&s["error"], "message");
        t.meta = s["metadata"].clone();
        t.created = num(&v["time"]["created"]);
        t.completed = num(&v["time"]["completed"]);
        t
    }
}

// tool results are a list of text/file chunks. files (images mostly) get a placeholder for now.
pub fn content_text(v: &Value) -> String {
    let mut out = String::new();
    for c in v.as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        match c["type"].as_str() {
            Some("text") => out.push_str(c["text"].as_str().unwrap_or("")),
            Some("file") => out.push_str("[file attachment]\n"),
            _ => {}
        }
    }
    out
}

#[derive(Clone)]
pub enum Part {
    Text(String),
    Reasoning(String),
    Tool(Tool),
}

// one row of the transcript.
#[derive(Clone)]
pub enum Item {
    User { id: String, text: String },
    Assistant { id: String, model: String, parts: Vec<Part>, error: String, context: u64, done: bool },
    Shell { id: String, command: String, output: String, running: bool, exit: Option<i64> },
    Notice { id: String, text: String },
    Failure { id: String, text: String },
}

impl Item {
    pub fn id(&self) -> &str {
        match self {
            Item::User { id, .. } | Item::Assistant { id, .. } | Item::Shell { id, .. } | Item::Notice { id, .. } | Item::Failure { id, .. } => id,
        }
    }

    pub fn parse(v: &Value) -> Option<Item> {
        let id = v["id"].as_str()?.to_owned();
        Some(match v["type"].as_str()? {
            "user" => Item::User { id, text: st(v, "text") },
            "assistant" => {
                let parts = v["content"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|c| match c["type"].as_str()? {
                                "text" => Some(Part::Text(st(c, "text"))),
                                "reasoning" => Some(Part::Reasoning(st(c, "text"))),
                                "tool" => Some(Part::Tool(Tool::parse(c))),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Item::Assistant {
                    id,
                    model: st(&v["model"], "id"),
                    parts,
                    error: st(&v["error"], "message"),
                    context: context_tokens(&v["tokens"]),
                    done: v["time"]["completed"].is_number(),
                }
            }
            "shell" => Item::Shell {
                id,
                command: st(v, "command"),
                output: st(&v["output"], "output"),
                running: v["status"].as_str() == Some("running"),
                exit: v["exit"].as_i64(),
            },
            "agent-switched" => Item::Notice { id, text: format!("agent → {}", st(v, "agent")) },
            "model-switched" => {
                let m = &v["model"];
                let var = m["variant"].as_str().map(|x| format!(" ({x})")).unwrap_or_default();
                Item::Notice { id, text: format!("model → {}{}", st(m, "id"), var) }
            }
            "location-switched" => Item::Notice { id, text: format!("moved to {}", st(&v["location"], "directory")) },
            "skill" => Item::Notice { id, text: format!("skill: {}", st(v, "name")) },
            "compaction" => match v["status"].as_str() {
                Some("failed") => Item::Failure { id, text: "compaction failed".to_owned() },
                Some("running") => Item::Notice { id, text: "compacting context…".to_owned() },
                _ => Item::Notice { id, text: "context compacted".to_owned() },
            },
            "synthetic" => {
                let d = st(v, "description");
                if d.is_empty() {
                    return None;
                }
                Item::Notice { id, text: d }
            }
            "idle" => match v["outcome"].as_str() {
                Some("failed") => {
                    let m = st(&v["error"], "message");
                    Item::Failure { id, text: if m.is_empty() { "run failed".to_owned() } else { m } }
                }
                Some("interrupted") => Item::Notice { id, text: "interrupted".to_owned() },
                _ => return None,
            },
            _ => return None, // system messages etc: not shown
        })
    }
}

// how full the context window was for a step: everything the model read plus what it wrote.
pub fn context_tokens(t: &Value) -> u64 {
    (num(&t["input"]) + num(&t["output"]) + num(&t["cache"]["read"]) + num(&t["cache"]["write"])) as u64
}
