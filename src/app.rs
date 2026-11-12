// app state + everything that mutates it: worker results, server events, user actions.
// drawing lives in ui.rs.

use crate::api::{Api, Msg, Outgoing};
use crate::model::*;
use crate::server::Server;
use crate::theme::{self, Pal};
use egui_commonmark::CommonMarkCache;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Clone, Copy, PartialEq)]
pub enum PaletteMode {
    All,
    Models,
    Variants,
    Agents,
    Sessions,
    Dirs,
}

pub struct Palette {
    pub mode: PaletteMode,
    pub query: String,
    pub sel: usize,
}

#[derive(Clone)]
pub enum Action {
    NewSession,
    Open(String),
    Delete(String),
    SetModel(ModelRef),
    SetAgent(String),
    SetDir(String),
    Accent(usize),
    ToggleSidebar,
    Settings,
    Compact,
    Interrupt,
    Palette(PaletteMode),
    FillPrompt(String),
}

// what you've typed/ticked into one field of a pending form.
#[derive(Default)]
pub struct FieldState {
    pub text: String,
    pub picked: Vec<String>,
    pub flag: bool,
}

pub struct App {
    pub api: Api,
    _server: Option<Server>, // dropped (= opencode killed) when the app exits
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub ctx: egui::Context,

    pub pal: Pal,
    pub accent: usize,
    pub opacity: f32, // 1.0 = solid window, lower lets the wallpaper/blur through
    pub zoom: f32,
    pub show_thinking: bool,
    pub open_diffs: bool,
    pub settings_open: bool,
    pub st: crate::settings::SettingsState,
    // mcp servers added from the settings window: name -> config. the server only keeps api-added
    // servers until it exits, so we remember them here and re-add them on every launch.
    pub own_mcp: serde_json::Map<String, Value>,
    mcp_applied: HashSet<String>, // project dirs we've already re-added them for

    pub ready: bool,
    pub fatal: Option<String>,
    pub sse: bool,
    pub toast: Option<(String, f64)>, // (text, time shown)

    pub sessions: Vec<Session>,
    pub active: Option<String>,
    pub items: Vec<Item>,
    pub loading: bool,
    pub busy: HashSet<String>,

    pub models: Vec<ModelInfo>,
    pub default_model: Option<ModelRef>,
    pub agents: Vec<AgentInfo>,
    pub commands: Vec<CommandInfo>,
    pub perms: Vec<Permission>,
    pub forms: Vec<Form>,
    pub form_state: HashMap<String, FieldState>,
    pub meta_dirty: bool,
    meta_at: f64,
    meta_dir: String, // directory the models/agents/commands lists were loaded for

    // defaults for the next new session
    pub new_dir: String,
    pub new_model: Option<ModelRef>,
    pub new_agent: Option<String>,

    pub prompt: String,
    pub focus_prompt: bool,
    pub stick: bool, // jump the transcript to the bottom on the next frame
    pending_n: u32,

    pub sidebar: bool,
    pub search: String,
    pub palette: Option<Palette>,

    pub toggled: HashSet<String>,        // tool rows flipped from their default open/closed state
    pub md: CommonMarkCache,
    pub heights: HashMap<String, (f32, f32)>, // item id -> (width it was laid out at, height)
    pub rail: Option<egui::Pos2>,             // last timeline dot drawn this frame (see ui.rs)

    pub shot: Option<(String, f64)>, // debug: screenshot path + when to take it
    pub shot_sent: bool,
    sse_stop: Arc<AtomicBool>, // tells the event-stream thread to quit. set when the server is replaced
    hooks_done: bool,          // debug hooks run once, not again after a server restart
    quiet_until: f64,          // no error toasts before this: requests still in the air when the server was restarted all fail

    // tok/s. the clock for a step starts at its first streamed chunk and stops at step.ended.
    stream: Option<Stream>,
    tps: HashMap<String, f32>, // session id -> speed of its last measured step
}

// the step that's streaming right now.
struct Stream {
    mid: String, // assistant message it belongs to
    first: f64,  // server time of the first chunk, ms
    last: f64,   // ... and of the latest one
    chars: usize, // text received so far, for the live estimate
}

pub(crate) fn state_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{}/.config", std::env::var("HOME").unwrap_or_default()));
    std::path::PathBuf::from(base).join("cloverleaf/state.json")
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        let ctx = cc.egui_ctx.clone();
        let (tx, rx) = channel();

        let saved: Value = std::fs::read_to_string(state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
        let accent = saved["accent"].as_u64().unwrap_or(0) as usize % theme::ACCENTS.len();
        let opacity = (saved["opacity"].as_f64().unwrap_or(0.78) as f32).clamp(0.3, 1.0);
        let zoom = (saved["zoom"].as_f64().unwrap_or(1.0) as f32).clamp(0.7, 1.6);
        let pal = theme::pal(accent, opacity);
        theme::install_fonts(&ctx);
        theme::apply(&ctx, &pal);
        ctx.set_zoom_factor(zoom);

        // first cli arg = project dir, else wherever we were launched from.
        let cwd = std::env::args()
            .nth(1)
            .and_then(|a| std::fs::canonicalize(a).ok())
            .or_else(|| std::env::current_dir().ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|| "/".to_owned());

        let (server, api, fatal) = match Server::start() {
            Ok(s) => {
                let api = Api::new(&s.base, &s.auth);
                (Some(s), api, None)
            }
            Err(e) => (None, Api::new("http://127.0.0.1:0", ""), Some(e)),
        };

        let app = App {
            api,
            _server: server,
            tx,
            rx,
            ctx,
            pal,
            accent,
            opacity,
            zoom,
            show_thinking: saved["show_thinking"].as_bool().unwrap_or(true),
            open_diffs: saved["open_diffs"].as_bool().unwrap_or(true),
            settings_open: false,
            st: Default::default(),
            own_mcp: saved["mcp"].as_object().cloned().unwrap_or_default(),
            mcp_applied: HashSet::new(),
            ready: false,
            fatal,
            sse: false,
            toast: None,
            sessions: Vec::new(),
            active: None,
            items: Vec::new(),
            loading: false,
            busy: HashSet::new(),
            models: Vec::new(),
            default_model: None,
            agents: Vec::new(),
            commands: Vec::new(),
            perms: Vec::new(),
            forms: Vec::new(),
            form_state: HashMap::new(),
            meta_dirty: false,
            meta_at: 0.0,
            meta_dir: String::new(),
            new_dir: cwd,
            new_model: ModelRef::parse(&saved["model"]),
            new_agent: saved["agent"].as_str().map(str::to_owned),
            prompt: String::new(),
            focus_prompt: true,
            stick: false,
            pending_n: 0,
            sidebar: saved["sidebar"].as_bool().unwrap_or(true),
            search: String::new(),
            palette: None,
            toggled: HashSet::new(),
            md: {
                let mut md = CommonMarkCache::default();
                let _ = md.add_syntax_theme_from_bytes("cloverleaf", include_bytes!("../assets/cloverleaf.tmTheme"));
                md
            },
            heights: HashMap::new(),
            rail: None,
            shot: std::env::var("CLOVERLEAF_SHOT").ok().map(|p| (p, std::env::var("CLOVERLEAF_SHOT_AFTER").ok().and_then(|s| s.parse().ok()).unwrap_or(4.0))),
            shot_sent: false,
            sse_stop: Arc::new(AtomicBool::new(false)),
            hooks_done: false,
            quiet_until: 0.0,
            stream: None,
            tps: HashMap::new(),
        };

        if app.fatal.is_none() {
            app.spawn(|api| match api.wait_ready() {
                Ok(()) => Msg::Ready,
                Err(e) => Msg::Fatal(e),
            });
        }
        app
    }

    // run `f` on a worker thread, deliver its result to pump() on the ui thread.
    pub fn spawn<F: FnOnce(&Api) -> Msg + Send + 'static>(&self, f: F) {
        let (api, tx, ctx) = (self.api.clone(), self.tx.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(f(&api));
            ctx.request_repaint();
        });
    }

    // for threads that report more than once (a download's progress): the raw way into pump().
    pub fn channel(&self) -> (Sender<Msg>, egui::Context) {
        (self.tx.clone(), self.ctx.clone())
    }

    // kill the opencode server and start a fresh one. the only way to make it reread its config,
    // which is how custom providers get in (custom.rs). anything that was running is stopped.
    pub fn restart_server(&mut self) {
        self.sse_stop.store(true, Ordering::Relaxed);
        self._server = None; // drop = SIGTERM and wait for it to go (up to a second)
        self.ready = false;
        self.sse = false;
        self.busy.clear();
        self.perms.clear();
        self.forms.clear();
        self.mcp_applied.clear(); // the new server hasn't been given our mcp servers yet
        self.stream = None;
        self.quiet_until = self.now() + 4.0;
        match Server::start() {
            Ok(s) => {
                self.api = Api::new(&s.base, &s.auth);
                self._server = Some(s);
                self.spawn(|api| match api.wait_ready() {
                    Ok(()) => Msg::Ready,
                    Err(e) => Msg::Fatal(e),
                });
            }
            Err(e) => self.fatal = Some(e),
        }
    }

    // fire a request where the only interesting outcome is failure.
    pub fn fire(&self, method: &'static str, path: String, body: Option<Value>) {
        self.spawn(move |api| match api.req(method, &path, body.as_ref()) {
            Ok(_) => Msg::Nop,
            Err(e) => Msg::Err(e),
        });
    }

    // re-derive colors after the accent or opacity changed.
    pub fn restyle(&mut self) {
        self.pal = theme::pal(self.accent, self.opacity);
        theme::apply(&self.ctx, &self.pal);
    }

    pub fn save_state(&self) {
        let v = json!({
            "accent": self.accent,
            "opacity": self.opacity,
            "zoom": self.zoom,
            "show_thinking": self.show_thinking,
            "open_diffs": self.open_diffs,
            "mcp": self.own_mcp,
            "sidebar": self.sidebar,
            "model": self.new_model.as_ref().map(|m| m.to_json()),
            "agent": self.new_agent,
        });
        let p = state_path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(p, v.to_string());
    }

    pub fn now(&self) -> f64 {
        self.ctx.input(|i| i.time)
    }

    pub fn error(&mut self, e: String) {
        eprintln!("cloverleaf: {e}");
        self.toast = Some((e, self.now()));
    }

    // ---------- loading ----------

    pub fn reload_sessions(&self) {
        self.spawn(|api| api.sessions().map(Msg::Sessions).unwrap_or_else(Msg::Err));
    }
    pub fn reload_messages(&self) {
        if let Some(sid) = self.active.clone() {
            self.spawn(move |api| match api.messages(&sid) {
                Ok(items) => Msg::Messages(sid, items),
                Err(e) => Msg::Err(e),
            });
        }
    }
    fn reload_meta(&mut self) {
        let dir = self.dir();
        self.meta_dir = dir.clone();
        if self.mcp_applied.insert(dir.clone()) {
            for (name, config) in &self.own_mcp {
                let path = format!("/api/experimental/mcp/{}?{}", crate::api::enc(name), crate::api::location(&dir));
                self.fire("PUT", path, Some(json!({ "config": config })));
            }
        }
        let (d1, d2, d3) = (dir.clone(), dir.clone(), dir);
        self.spawn(move |api| api.models(&d1).map(|(m, d)| Msg::Models(m, d)).unwrap_or_else(Msg::Err));
        self.spawn(move |api| api.agents(&d2).map(Msg::Agents).unwrap_or_else(Msg::Err));
        self.spawn(move |api| api.commands(&d3).map(Msg::Commands).unwrap_or_else(Msg::Err));
    }
    // permission prompts + forms the active session is waiting on.
    fn reload_requests(&self) {
        let Some(sid) = self.active.clone() else { return };
        let s2 = sid.clone();
        self.spawn(move |api| match api.perms(&sid) {
            Ok(p) => Msg::Perms(sid, p),
            Err(e) => Msg::Err(e),
        });
        self.spawn(move |api| match api.forms(&s2) {
            Ok(f) => Msg::Forms(s2, f),
            Err(e) => Msg::Err(e),
        });
    }

    // ---------- worker results ----------

    pub fn pump(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Ready => {
                    self.ready = true;
                    self.reload_sessions();
                    self.reload_meta();
                    let (api, tx, ctx) = (self.api.clone(), self.tx.clone(), self.ctx.clone());
                    let stop = Arc::new(AtomicBool::new(false));
                    self.sse_stop = stop.clone();
                    std::thread::spawn(move || api.event_loop(tx, ctx, stop));
                    self.st.dirty = true; // an open settings tab should refetch from the (maybe new) server
                    if !std::mem::replace(&mut self.hooks_done, true) {
                        self.debug_hooks();
                    }
                }
                Msg::Fatal(e) => self.fatal = Some(e),
                Msg::Sessions(mut list) => {
                    list.sort_by(|a, b| b.updated.total_cmp(&a.updated));
                    for s in &list {
                        if s.running {
                            self.busy.insert(s.id.clone());
                        }
                    }
                    self.sessions = list;
                }
                Msg::Messages(sid, items) => {
                    if self.active.as_deref() == Some(sid.as_str()) {
                        self.items = items;
                        self.loading = false;
                        self.heights.clear();
                    }
                }
                Msg::Models(list, def) => {
                    self.models = list;
                    self.default_model = def;
                }
                Msg::Agents(a) => self.agents = a,
                Msg::Commands(c) => self.commands = c,
                Msg::Perms(sid, list) => {
                    self.perms.retain(|p| p.session != sid);
                    self.perms.extend(list);
                }
                Msg::Forms(sid, list) => {
                    self.forms.retain(|f| f.session != sid);
                    self.forms.extend(list);
                }
                Msg::Created(s, out) => {
                    let sid = s.id.clone();
                    self.sessions.insert(0, s);
                    self.active = Some(sid.clone());
                    self.items.clear();
                    self.heights.clear();
                    if let Some(out) = out {
                        self.dispatch(sid, out);
                    }
                }
                Msg::Event(e) => self.on_event(&e),
                Msg::Sse(up) => {
                    // after a reconnect we may have missed events, so resync.
                    if up && !self.sse && self.ready {
                        self.reload_sessions();
                        self.reload_messages();
                        self.reload_requests();
                    }
                    self.sse = up;
                }
                Msg::Settings(m) => self.on_settings(m),
                Msg::Err(e) if self.now() < self.quiet_until => eprintln!("cloverleaf: (during restart) {e}"),
                Msg::Err(e) => self.error(e),
                Msg::Nop => {}
            }
        }

        // model/agent/command "updated" events come in bursts at startup; coalesce them.
        if self.ready && self.meta_dir != self.dir() {
            self.meta_dirty = true; // switched project: its config may define other models/agents/commands
            self.meta_dir = self.dir();
        }
        if self.meta_dirty && self.now() - self.meta_at > 1.0 {
            self.meta_dirty = false;
            self.meta_at = self.now();
            self.reload_meta();
        }
    }

    // env knobs for driving the app from a script (used to test it headlessly-ish).
    fn debug_hooks(&mut self) {
        if let Ok(sid) = std::env::var("CLOVERLEAF_OPEN") {
            self.open(sid);
        }
        if let Ok(text) = std::env::var("CLOVERLEAF_PROMPT") {
            self.prompt = text;
            self.send();
        }
        if let Ok(tab) = std::env::var("CLOVERLEAF_SETTINGS") {
            use crate::settings::Tab;
            self.settings_open = true;
            self.st.tab = match tab.as_str() {
                "providers" => Tab::Providers,
                "ollama" => Tab::Ollama,
                "custom" => {
                    self.st.open_custom_form();
                    Tab::Providers
                }
                "mcp" => Tab::Mcp,
                "plugins" => Tab::Plugins,
                "skills" => Tab::Skills,
                "permissions" => Tab::Permissions,
                _ => Tab::General,
            };
        }
        if std::env::var("CLOVERLEAF_RESTART").is_ok() {
            self.restart_server(); // exercise the restart path: the app should come back up by itself
        }
        match std::env::var("CLOVERLEAF_PALETTE").as_deref() {
            Ok("models") => self.run(Action::Palette(PaletteMode::Models)),
            Ok("agents") => self.run(Action::Palette(PaletteMode::Agents)),
            Ok(_) => self.run(Action::Palette(PaletteMode::All)),
            Err(_) => {}
        }
    }

    // ---------- server events ----------

    fn on_event(&mut self, e: &Value) {
        let t = e["type"].as_str().unwrap_or("");
        let d = &e["data"];
        let sid = d["sessionID"].as_str().unwrap_or("");

        match t {
            "session.created" | "session.deleted" | "session.renamed" | "session.archived" | "session.unarchived" | "session.forked" | "session.moved"
            | "session.model.selected" | "session.agent.selected" => self.reload_sessions(),
            "session.usage.updated" => {
                if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                    s.cost = d["cost"].as_f64().unwrap_or(s.cost);
                }
            }
            "session.execution.started" => {
                self.busy.insert(sid.to_owned());
            }
            "session.execution.succeeded" | "session.execution.failed" | "session.execution.interrupted" => {
                self.busy.remove(sid);
                self.reload_sessions();
                if self.active.as_deref() == Some(sid) {
                    self.reload_messages();
                }
            }
            "permission.asked" => {
                if let Some(p) = Permission::parse(d) {
                    self.perms.retain(|x| x.id != p.id);
                    self.perms.push(p);
                }
            }
            "form.created" => {
                let f = if d["form"].is_object() { &d["form"] } else { d };
                if let Some(f) = Form::parse(f) {
                    self.forms.retain(|x| x.id != f.id);
                    self.forms.push(f);
                }
            }
            // answered (maybe from the tui or another window): drop whatever ids the event names, then resync.
            "permission.replied" | "form.replied" | "form.cancelled" => {
                let ids: Vec<&str> = [&d["id"], &d["requestID"], &d["permissionID"], &d["formID"], &d["form"]["id"]].iter().filter_map(|v| v.as_str()).collect();
                self.perms.retain(|x| !ids.contains(&x.id.as_str()));
                self.forms.retain(|x| !ids.contains(&x.id.as_str()));
                self.reload_requests();
            }
            "model.updated" | "agent.updated" | "command.updated" => self.meta_dirty = true,
            "integration.updated" | "credential.updated" | "mcp.status.changed" | "plugin.updated" | "skill.updated" => self.st.dirty = true,
            _ => {}
        }

        if sid.is_empty() || self.active.as_deref() != Some(sid) {
            return;
        }
        let mid = d["assistantMessageID"].as_str().unwrap_or("");
        let ord = d["ordinal"].as_u64().unwrap_or(0) as usize;
        let tid = d["id"].as_str().unwrap_or("");

        // tok/s bookkeeping: every streamed chunk of text, thinking or tool arguments counts.
        if t.ends_with(".delta") {
            let at = e["created"].as_f64().unwrap_or(0.0);
            let n = d["delta"].as_str().map(str::len).unwrap_or(0);
            match &mut self.stream {
                Some(s) if s.mid == mid => {
                    s.chars += n;
                    s.last = at;
                }
                _ => self.stream = Some(Stream { mid: mid.to_owned(), first: at, last: at, chars: n }),
            }
        }

        match t {
            // things that change the transcript in ways we don't model incrementally: just refetch.
            "session.inbox.delivered" | "session.step.failed" | "session.compaction.started" | "session.compaction.ended" | "session.compaction.failed"
            | "session.revert.staged" | "session.revert.cleared" | "session.revert.committed" | "session.shell.started" | "session.shell.ended"
            | "session.synthetic" | "session.skill.activated" => self.reload_messages(),

            "session.step.started" => {
                let model = d["model"]["id"].as_str().unwrap_or("").to_owned();
                if let Item::Assistant { model: m, .. } = self.assistant(mid) {
                    *m = model;
                }
            }
            "session.step.ended" => {
                // real numbers now: the tokens the provider counted, over the time the step spent streaming.
                // some providers count thinking inside `output`, some next to it, so add both.
                if let Some(s) = self.stream.take()
                    && s.mid == mid
                {
                    let out = d["tokens"]["output"].as_f64().unwrap_or(0.0) + d["tokens"]["reasoning"].as_f64().unwrap_or(0.0);
                    let secs = (e["created"].as_f64().unwrap_or(0.0) - s.first) / 1000.0;
                    if out > 0.0 && secs > 0.2 {
                        self.tps.insert(sid.to_owned(), (out / secs) as f32);
                    }
                }
                if let Item::Assistant { context, done, .. } = self.assistant(mid) {
                    *context = context_tokens(&d["tokens"]);
                    *done = true;
                }
            }

            "session.text.started" => {
                self.text_part(mid, true, ord);
            }
            "session.text.delta" => self.text_part(mid, true, ord).push_str(d["delta"].as_str().unwrap_or("")),
            "session.text.ended" => *self.text_part(mid, true, ord) = d["text"].as_str().unwrap_or("").to_owned(),
            "session.reasoning.started" => {
                self.text_part(mid, false, ord);
            }
            "session.reasoning.delta" => self.text_part(mid, false, ord).push_str(d["delta"].as_str().unwrap_or("")),
            "session.reasoning.ended" => *self.text_part(mid, false, ord) = d["text"].as_str().unwrap_or("").to_owned(),

            "session.tool.input.started" => {
                let name = d["name"].as_str().unwrap_or("tool").to_owned();
                let created = e["created"].as_f64().unwrap_or(0.0);
                let tool = self.tool(mid, tid);
                tool.name = name;
                tool.created = created;
            }
            "session.tool.input.delta" => self.tool(mid, tid).raw_input.push_str(d["delta"].as_str().unwrap_or("")),
            "session.tool.input.ended" => self.tool(mid, tid).raw_input = d["text"].as_str().unwrap_or("").to_owned(),
            "session.tool.called" => {
                let tool = self.tool(mid, tid);
                tool.input = d["input"].clone();
                tool.status = ToolStatus::Running;
            }
            "session.tool.progress" => self.tool(mid, tid).meta = d["metadata"].clone(),
            "session.tool.success" => {
                let done = e["created"].as_f64().unwrap_or(0.0);
                let tool = self.tool(mid, tid);
                tool.output = content_text(&d["content"]);
                tool.meta = d["metadata"].clone();
                tool.status = ToolStatus::Done;
                tool.completed = done;
            }
            "session.tool.failed" => {
                let tool = self.tool(mid, tid);
                tool.error = d["error"]["message"].as_str().unwrap_or("tool failed").to_owned();
                tool.output = content_text(&d["content"]);
                tool.status = ToolStatus::Error;
            }
            "session.retry.scheduled" => {
                let why = d["error"]["message"].as_str().unwrap_or("provider error");
                self.toast = Some((format!("retrying: {why}"), self.now()));
            }
            _ => {}
        }
    }

    // find (or append) the assistant message a streaming event belongs to.
    fn assistant(&mut self, mid: &str) -> &mut Item {
        let found = self.items.iter().rposition(|i| matches!(i, Item::Assistant { id, .. } if id == mid));
        let idx = found.unwrap_or_else(|| {
            self.items.push(Item::Assistant { id: mid.to_owned(), model: String::new(), parts: Vec::new(), error: String::new(), context: 0, done: false });
            self.items.len() - 1
        });
        &mut self.items[idx]
    }

    fn parts(&mut self, mid: &str) -> &mut Vec<Part> {
        match self.assistant(mid) {
            Item::Assistant { parts, .. } => parts,
            _ => unreachable!(),
        }
    }

    // `ordinal` counts per kind: the nth text block, or the nth reasoning block, of the message.
    fn text_part(&mut self, mid: &str, is_text: bool, ordinal: usize) -> &mut String {
        let parts = self.parts(mid);
        let same_kind = |p: &Part| matches!((p, is_text), (Part::Text(_), true) | (Part::Reasoning(_), false));
        let found = parts.iter().enumerate().filter(|(_, p)| same_kind(p)).nth(ordinal).map(|(i, _)| i);
        let idx = found.unwrap_or_else(|| {
            parts.push(if is_text { Part::Text(String::new()) } else { Part::Reasoning(String::new()) });
            parts.len() - 1
        });
        match &mut parts[idx] {
            Part::Text(s) | Part::Reasoning(s) => s,
            Part::Tool(_) => unreachable!(),
        }
    }

    fn tool(&mut self, mid: &str, tid: &str) -> &mut Tool {
        let parts = self.parts(mid);
        let found = parts.iter().position(|p| matches!(p, Part::Tool(t) if t.id == tid));
        let idx = found.unwrap_or_else(|| {
            parts.push(Part::Tool(Tool::new(tid, "tool")));
            parts.len() - 1
        });
        match &mut parts[idx] {
            Part::Tool(t) => t,
            _ => unreachable!(),
        }
    }

    // ---------- lookups ----------

    pub fn session(&self) -> Option<&Session> {
        let id = self.active.as_deref()?;
        self.sessions.iter().find(|s| s.id == id)
    }

    pub fn is_busy(&self) -> bool {
        self.active.as_ref().is_some_and(|id| self.busy.contains(id))
    }

    // the directory the active session (or the next new one) works in.
    pub fn dir(&self) -> String {
        self.session().map(|s| s.dir.clone()).unwrap_or_else(|| self.new_dir.clone())
    }

    pub fn current_model(&self) -> Option<ModelRef> {
        if let Some(s) = self.session() {
            if let Some(m) = &s.model {
                return Some(m.clone());
            }
            // session never had a model picked explicitly: show whatever answered last.
            let last = self.items.iter().rev().find_map(|i| match i {
                Item::Assistant { model, .. } if !model.is_empty() => Some(model.as_str()),
                _ => None,
            });
            if let Some(m) = last.and_then(|id| self.models.iter().find(|m| m.id == id)) {
                return Some(ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant: None });
            }
            return self.default_model.clone();
        }
        self.new_model.clone().or_else(|| self.default_model.clone())
    }

    pub fn model_info(&self, r: &ModelRef) -> Option<&ModelInfo> {
        self.models.iter().find(|m| m.id == r.id && m.provider == r.provider)
    }

    pub fn current_agent(&self) -> String {
        let picked = match self.session() {
            Some(s) => s.agent.clone(),
            None => self.new_agent.clone(),
        };
        picked.or_else(|| self.agents.first().map(|a| a.id.clone())).unwrap_or_else(|| "build".to_owned())
    }

    // generation speed to show for the active session: (tokens per second, is it a live estimate).
    // live = while a step streams, guessed from the text so far at ~4 characters per token.
    // otherwise the last finished step's real figure. None if nothing was measured this run:
    // old messages only record when they started and ended, which includes waiting for the first token.
    pub fn tok_rate(&self) -> Option<(f32, bool)> {
        let sid = self.active.as_ref()?;
        if let Some(s) = &self.stream
            && self.busy.contains(sid)
            && s.last - s.first > 1000.0
        {
            return Some(((s.chars as f64 / 4.0 / ((s.last - s.first) / 1000.0)) as f32, true));
        }
        self.tps.get(sid).map(|r| (*r, false))
    }

    // tokens in the context window after the latest step.
    pub fn context_used(&self) -> u64 {
        self.items
            .iter()
            .rev()
            .find_map(|i| match i {
                Item::Assistant { context, .. } if *context > 0 => Some(*context),
                _ => None,
            })
            .unwrap_or(0)
    }

    // ---------- actions ----------

    pub fn open(&mut self, sid: String) {
        if self.active.as_deref() == Some(sid.as_str()) {
            return;
        }
        self.active = Some(sid);
        self.items.clear();
        self.heights.clear();
        self.loading = true;
        self.stick = true;
        self.focus_prompt = true;
        self.reload_messages();
        self.reload_requests();
    }

    pub fn send(&mut self) {
        let text = self.prompt.trim().to_owned();
        if text.is_empty() || !self.ready {
            return;
        }
        self.prompt.clear();

        let out = if let Some(rest) = text.strip_prefix('/') {
            let (name, arg) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
            match name {
                "new" | "clear" => return self.run(Action::NewSession),
                "models" | "model" => return self.run(Action::Palette(PaletteMode::Models)),
                "agents" | "agent" => return self.run(Action::Palette(PaletteMode::Agents)),
                "sessions" | "resume" => return self.run(Action::Palette(PaletteMode::Sessions)),
                "compact" => return self.run(Action::Compact),
                _ if self.commands.iter().any(|c| c.name == name) => Outgoing::Command { name: name.to_owned(), text: arg.trim().to_owned() },
                _ => Outgoing::Prompt(text), // not a command we know, let the model have it
            }
        } else if let Some(cmd) = text.strip_prefix('!') {
            Outgoing::Shell(cmd.trim().to_owned())
        } else {
            Outgoing::Prompt(text)
        };

        match self.active.clone() {
            Some(sid) => self.dispatch(sid, out),
            None => {
                let (dir, agent, model) = (self.new_dir.clone(), self.new_agent.clone(), self.new_model.clone());
                self.spawn(move |api| match api.create_session(&dir, agent.as_deref(), model.as_ref()) {
                    Ok(s) => Msg::Created(s, Some(out)),
                    Err(e) => Msg::Err(e),
                });
            }
        }
    }

    fn dispatch(&mut self, sid: String, out: Outgoing) {
        self.stick = true;
        let (path, body) = match out {
            Outgoing::Prompt(text) => {
                // show it right away; the next transcript refetch swaps in the real message.
                self.pending_n += 1;
                self.items.push(Item::User { id: format!("pending-{}", self.pending_n), text: text.clone() });
                self.busy.insert(sid.clone());
                (format!("/api/session/{sid}/prompt"), json!({ "text": text }))
            }
            Outgoing::Command { name, text } => {
                self.busy.insert(sid.clone());
                (format!("/api/session/{sid}/command"), json!({ "name": name, "text": text }))
            }
            Outgoing::Shell(command) => (format!("/api/session/{sid}/shell"), json!({ "command": command })),
        };
        self.fire("POST", path, Some(body));
    }

    pub fn run(&mut self, a: Action) {
        match a {
            Action::NewSession => {
                self.active = None;
                self.items.clear();
                self.heights.clear();
                self.focus_prompt = true;
            }
            Action::Open(sid) => self.open(sid),
            Action::Delete(sid) => {
                if self.active.as_deref() == Some(sid.as_str()) {
                    self.run(Action::NewSession);
                }
                self.sessions.retain(|s| s.id != sid);
                self.fire("DELETE", format!("/api/session/{sid}"), None);
            }
            Action::SetModel(m) => {
                if let Some(sid) = self.active.clone() {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                        s.model = Some(m.clone());
                    }
                    self.fire("POST", format!("/api/session/{sid}/model"), Some(json!({ "model": m.to_json() })));
                }
                self.new_model = Some(m);
                self.save_state();
                self.focus_prompt = true;
            }
            Action::SetAgent(agent) => {
                if let Some(sid) = self.active.clone() {
                    if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                        s.agent = Some(agent.clone());
                    }
                    self.fire("POST", format!("/api/session/{sid}/agent"), Some(json!({ "agent": agent })));
                }
                self.new_agent = Some(agent);
                self.save_state();
                self.focus_prompt = true;
            }
            Action::SetDir(dir) => {
                self.new_dir = dir;
                self.run(Action::NewSession);
            }
            Action::Accent(i) => {
                self.accent = i % theme::ACCENTS.len();
                self.restyle();
                self.save_state();
            }
            Action::ToggleSidebar => {
                self.sidebar = !self.sidebar;
                self.save_state();
            }
            Action::Settings => self.settings_open = true,
            Action::Compact => {
                if let Some(sid) = self.active.clone() {
                    self.fire("POST", format!("/api/session/{sid}/compact"), Some(json!({})));
                }
            }
            Action::Interrupt => {
                if let Some(sid) = self.active.clone() {
                    self.fire("POST", format!("/api/session/{sid}/interrupt"), None);
                }
            }
            Action::Palette(mode) => self.palette = Some(Palette { mode, query: String::new(), sel: 0 }),
            Action::FillPrompt(text) => {
                self.prompt = text;
                self.focus_prompt = true;
            }
        }
    }

    pub fn reply_permission(&mut self, p: &Permission, decision: &str) {
        self.fire("POST", format!("/api/session/{}/permission/{}/reply", p.session, p.id), Some(json!({ "decision": decision })));
        self.perms.retain(|x| x.id != p.id);
    }

    pub fn reply_form(&mut self, f: &Form) {
        let mut answer = serde_json::Map::new();
        for field in &f.fields {
            let Some(st) = self.form_state.get(&format!("{}/{}", f.id, field.key)) else { continue };
            let text = st.text.trim();
            let v = match field.kind.as_str() {
                "boolean" => json!(st.flag),
                "multiselect" => {
                    let mut all = st.picked.clone();
                    if !text.is_empty() {
                        all.push(text.to_owned());
                    }
                    json!(all)
                }
                "number" | "integer" => match text.parse::<f64>() {
                    Ok(n) => json!(n),
                    Err(_) => continue,
                },
                "external" => continue,
                _ => {
                    if !text.is_empty() {
                        json!(text)
                    } else if let Some(p) = st.picked.first() {
                        json!(p)
                    } else {
                        continue;
                    }
                }
            };
            answer.insert(field.key.clone(), v);
        }
        self.fire("POST", format!("/api/session/{}/form/{}/reply", f.session, f.id), Some(json!({ "answer": answer })));
        self.forms.retain(|x| x.id != f.id);
    }

    pub fn cancel_form(&mut self, f: &Form) {
        self.fire("DELETE", format!("/api/session/{}/form/{}", f.session, f.id), None);
        self.forms.retain(|x| x.id != f.id);
    }
}
