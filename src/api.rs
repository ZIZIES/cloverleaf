// blocking http client for the opencode server. every call here runs on a worker thread
// (see App::spawn) and the result goes back to the ui thread over an mpsc channel.

use crate::model::*;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::time::Duration;

#[derive(Clone)]
pub struct Api {
    agent: ureq::Agent,
    base: String,
    auth: String,
}

// everything a worker thread can tell the ui thread.
pub enum Msg {
    Ready,
    Fatal(String),
    Sessions(Vec<Session>),
    Messages(String, Vec<Item>),
    Models(Vec<ModelInfo>, Option<ModelRef>),
    Agents(Vec<AgentInfo>),
    Commands(Vec<CommandInfo>),
    Perms(String, Vec<Permission>),
    Forms(String, Vec<Form>),
    Created(Session, Option<Outgoing>),
    Event(Value),
    Sse(bool),
    Settings(crate::settings::SMsg),
    Err(String),
    Nop,
}

// something the user typed that has to go to a session.
pub enum Outgoing {
    Prompt(String),
    Command { name: String, text: String },
    Shell(String),
}

impl Api {
    pub fn new(base: &str, auth: &str) -> Api {
        let cfg = ureq::Agent::config_builder()
            .http_status_as_error(false) // we read the error body ourselves
            .timeout_global(None)
            .timeout_connect(Some(Duration::from_secs(5)))
            .build();
        Api { agent: ureq::Agent::new_with_config(cfg), base: base.to_owned(), auth: auth.to_owned() }
    }

    pub fn req(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        let a = &self.agent;
        let h = ("Authorization", self.auth.as_str());
        let res = match (method, body) {
            ("GET", _) => a.get(&url).header(h.0, h.1).call(),
            ("DELETE", _) => a.delete(&url).header(h.0, h.1).call(),
            ("POST", Some(b)) => a.post(&url).header(h.0, h.1).send_json(b),
            ("POST", None) => a.post(&url).header(h.0, h.1).send_empty(),
            ("PATCH", Some(b)) => a.patch(&url).header(h.0, h.1).send_json(b),
            ("PUT", Some(b)) => a.put(&url).header(h.0, h.1).send_json(b),
            _ => return Err(format!("bad request shape: {method} {path}")),
        };
        let mut res = res.map_err(|e| format!("{method} {path}: {e}"))?;
        let status = res.status().as_u16();
        let text = res.body_mut().with_config().limit(512 * 1024 * 1024).read_to_string().map_err(|e| format!("{method} {path}: {e}"))?;
        let v: Value = if text.trim().is_empty() { Value::Null } else { serde_json::from_str(&text).unwrap_or(Value::String(text)) };
        if status >= 400 {
            let why = v["message"].as_str().or(v.as_str()).unwrap_or("request failed");
            return Err(format!("{method} {path}: {status} {why}"));
        }
        Ok(v)
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.req("GET", path, None)
    }
    pub fn post(&self, path: &str, body: Value) -> Result<Value, String> {
        self.req("POST", path, Some(&body))
    }

    pub fn wait_ready(&self) -> Result<(), String> {
        let mut last = String::new();
        for _ in 0..150 {
            match self.get("/api/info") {
                Ok(_) => return Ok(()),
                Err(e) => last = e,
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Err(format!("opencode server never came up: {last}"))
    }

    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        let v = self.get("/api/session?limit=300&parentID=null")?;
        Ok(arr(&v["data"]).iter().filter_map(Session::parse).collect())
    }

    // whole transcript, oldest first. the cursor already encodes the order, so only page 1 passes it.
    pub fn messages(&self, sid: &str) -> Result<Vec<Item>, String> {
        const PAGE: usize = 200;
        let mut out = Vec::new();
        let mut path = format!("/api/session/{sid}/message?order=asc&limit={PAGE}");
        for _ in 0..100 {
            let v = self.get(&path)?;
            let data = arr(&v["data"]);
            out.extend(data.iter().filter_map(Item::parse));
            match v["cursor"]["next"].as_str() {
                Some(c) if data.len() >= PAGE => path = format!("/api/session/{sid}/message?limit={PAGE}&cursor={}", enc(c)),
                _ => break,
            }
        }
        Ok(out)
    }

    // models, agents and commands depend on the project's config, so they're asked per directory.
    pub fn models(&self, dir: &str) -> Result<(Vec<ModelInfo>, Option<ModelRef>), String> {
        let loc = location(dir);
        let v = self.get(&format!("/api/model?{loc}"))?;
        let mut list: Vec<ModelInfo> = arr(&v["data"]).iter().filter_map(ModelInfo::parse).collect();
        list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let def = self.get(&format!("/api/model/default?{loc}")).ok().and_then(|d| ModelRef::parse(&d["data"]));
        Ok((list, def))
    }

    pub fn agents(&self, dir: &str) -> Result<Vec<AgentInfo>, String> {
        let v = self.get(&format!("/api/agent?{}", location(dir)))?;
        Ok(arr(&v["data"]).iter().filter_map(AgentInfo::parse).collect())
    }

    pub fn commands(&self, dir: &str) -> Result<Vec<CommandInfo>, String> {
        let v = self.get(&format!("/api/command?{}", location(dir)))?;
        Ok(arr(&v["data"]).iter().filter_map(CommandInfo::parse).collect())
    }

    // things a session is blocked on: permission prompts and question forms.
    pub fn perms(&self, sid: &str) -> Result<Vec<Permission>, String> {
        let v = self.get(&format!("/api/session/{sid}/permission"))?;
        Ok(list_of(&v).iter().filter_map(Permission::parse).collect())
    }

    pub fn forms(&self, sid: &str) -> Result<Vec<Form>, String> {
        let v = self.get(&format!("/api/session/{sid}/form"))?;
        Ok(list_of(&v).iter().filter_map(Form::parse).collect())
    }

    pub fn create_session(&self, dir: &str, agent: Option<&str>, model: Option<&ModelRef>) -> Result<Session, String> {
        let mut body = json!({ "location": { "directory": dir } });
        if let Some(a) = agent {
            body["agent"] = json!(a);
        }
        if let Some(m) = model {
            body["model"] = m.to_json();
        }
        let v = self.post("/api/session", body)?;
        Session::parse(&v["data"]).ok_or_else(|| "server returned a session i couldn't parse".to_owned())
    }

    // server-sent events: one long GET, one json object per `data:` line. reconnects forever,
    // until `stop` is set (the app swapped this server for a new one and started a new loop).
    pub fn event_loop(&self, tx: Sender<Msg>, ctx: egui::Context, stop: Arc<AtomicBool>) {
        loop {
            if stop.load(Ordering::Relaxed) {
                return;
            }
            let res = self.agent.get(format!("{}/api/event", self.base)).header("Authorization", &self.auth).call();
            if let Ok(res) = res
                && res.status().as_u16() < 400
            {
                let _ = tx.send(Msg::Sse(true));
                ctx.request_repaint();
                let reader = BufReader::new(res.into_body().into_reader());
                for line in reader.lines() {
                    let Ok(line) = line else { break };
                    if stop.load(Ordering::Relaxed) {
                        return;
                    }
                    let Some(data) = line.strip_prefix("data:") else { continue };
                    if let Ok(v) = serde_json::from_str::<Value>(data.trim()) {
                        if tx.send(Msg::Event(v)).is_err() {
                            return; // ui is gone
                        }
                        ctx.request_repaint();
                    }
                }
            }
            // a stopped loop must not report "disconnected" over the new one's "connected".
            if stop.load(Ordering::Relaxed) || tx.send(Msg::Sse(false)).is_err() {
                return;
            }
            ctx.request_repaint();
            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

fn arr(v: &Value) -> &[Value] {
    v.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}

// some list endpoints wrap in {data: [...]}, some return the bare array.
fn list_of(v: &Value) -> &[Value] {
    if v.is_array() { arr(v) } else { arr(&v["data"]) }
}

// the `location` query param is an object, spelled location[directory]=... on the wire.
pub fn location(dir: &str) -> String {
    format!("location%5Bdirectory%5D={}", enc(dir))
}

// percent-encode a query value.
pub fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}
