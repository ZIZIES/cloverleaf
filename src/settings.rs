// the settings window: tabs for appearance, provider keys/logins, mcp servers, plugins,
// skills and saved permissions. each tab loads its own data from the server when opened.

use crate::api::{Api, Msg, location};
use crate::app::{Action, App, PaletteMode};
use crate::model::{Field, Form};
use crate::theme::{self, Pal, medium, mono, sans, semibold};
use crate::ui::{chip, note, section, setting, solid_button, toggle};
use egui::{Align2, Color32, FontId, Frame, Id, Label, Margin, RichText, ScrollArea, Sense, Sides, Stroke, TextEdit, Ui, pos2, vec2};
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    General,
    Providers,
    Ollama,
    Mcp,
    Plugins,
    Skills,
    Permissions,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::General, "General"),
    (Tab::Providers, "Providers & keys"),
    (Tab::Ollama, "Local models"),
    (Tab::Mcp, "MCP servers"),
    (Tab::Plugins, "Plugins"),
    (Tab::Skills, "Skills"),
    (Tab::Permissions, "Permissions"),
];

// a provider you can connect: openrouter, anthropic, ollama cloud, ...
pub struct Integration {
    pub id: String,
    pub name: String,
    pub key: bool,                    // accepts a pasted api key
    pub key_fields: Vec<Field>,       // extra questions that go with the key (azure resource name etc)
    pub logins: Vec<(String, String, Vec<Field>)>, // oauth methods: (method id, button label, extra questions)
    pub externals: Vec<(String, String)>, // logins owned by another program, e.g. claude code: (method id, label)
    pub env: Vec<String>,             // env vars it also reads
    pub conns: Vec<Conn>,
}

// a provider whose server runs on this machine (ollama, lm studio, ...). opencode finds these by itself.
pub struct Local {
    pub id: String,
    pub name: String,
    pub url: String,
}

pub struct Conn {
    pub cred: Option<String>, // credential id; None = it came from an env var
    pub label: String,
    pub method: String,
}

pub struct McpServer {
    pub name: String,
    pub status: String,
    pub error: String,
}

pub struct Plugin {
    pub id: String,
    pub source: String, // "builtin", "package foo@1.2", "local /path"
    pub builtin: bool,
    pub target: Option<String>, // package name, when it can be updated
    pub outdated: bool,
    pub error: String,
}

pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: String,
}

pub struct SavedPerm {
    pub id: String,
    pub action: String,
    pub resource: String,
}

// a browser sign-in that's been started and not finished yet.
pub struct OAuth {
    pub integration: String,
    pub attempt: String,
    pub url: String,
    pub instructions: String,
    pub needs_code: bool,
    pub code: String,
    pub error: String,
    pub polled: f64,
}

pub enum SMsg {
    Integrations(Vec<Integration>, Vec<Local>),
    Mcp(Vec<McpServer>),
    Plugins(Vec<Plugin>),
    Skills(Vec<Skill>),
    Saved(Vec<SavedPerm>),
    Changed, // a write went through: refetch
    OAuth(OAuth),
    OAuthStatus(String, String), // (status, message)
    Ollama(crate::ollama::OMsg),  // the local models tab keeps its own messages, see ollama.rs
    Custom(Result<(), String>),   // the custom providers file was changed (or couldn't be)
}

// things the tabs want done. collected while drawing, executed after.
enum Op {
    SaveKey(String),
    RemoveCred(String),
    Login(String, String),
    External(String, String), // (integration, method): borrow a login another program already has
    CustomAdd,
    CustomRemove(String),
    CustomRefresh(String),
    LoginCode,
    LoginCancel,
    McpToggle(String, bool),
    McpRemove(String),
    McpAdd,
    PluginCheck,
    PluginUpdate(String),
    RemoveSaved(String),
    OpenUrl(String),
    Act(Action),
}

pub struct SettingsState {
    pub tab: Tab,
    loaded: Option<Tab>,
    loaded_at: f64,
    pub dirty: bool, // server says something changed while the window is open
    filter: String,

    integrations: Vec<Integration>,
    locals: Vec<Local>,
    mcp: Vec<McpServer>,
    plugins: Vec<Plugin>,
    skills: Vec<Skill>,
    saved: Vec<SavedPerm>,

    editing: Option<String>,          // integration whose "add key" form is open
    key_input: String,
    answers: HashMap<String, String>, // extra form fields for the open key / login form
    oauth: Option<OAuth>,
    confirm: Option<String>, // destructive button that's been clicked once ("sure?")
    mcp_name: String,
    mcp_target: String,
    show_builtin: bool,
    pub ol: crate::ollama::OllamaState,

    // custom openai-compatible providers (custom.rs) and the form that adds one
    customs: Vec<crate::custom::Custom>,
    c_open: bool,
    c_name: String,
    c_url: String,
    c_key: String,
    c_models: String,
    c_busy: bool,
    c_error: String,
}

impl Default for SettingsState {
    fn default() -> Self {
        SettingsState {
            tab: Tab::General,
            loaded: None,
            loaded_at: 0.0,
            dirty: false,
            filter: String::new(),
            integrations: Vec::new(),
            locals: Vec::new(),
            mcp: Vec::new(),
            plugins: Vec::new(),
            skills: Vec::new(),
            saved: Vec::new(),
            editing: None,
            key_input: String::new(),
            answers: HashMap::new(),
            oauth: None,
            confirm: None,
            mcp_name: String::new(),
            mcp_target: String::new(),
            show_builtin: false,
            ol: Default::default(),
            customs: Vec::new(),
            c_open: false,
            c_name: String::new(),
            c_url: String::new(),
            c_key: String::new(),
            c_models: String::new(),
            c_busy: false,
            c_error: String::new(),
        }
    }
}

impl SettingsState {
    // for the CLOVERLEAF_SETTINGS=custom debug hook.
    pub fn open_custom_form(&mut self) {
        self.c_open = true;
    }
}

// ---------------------------------------------------------------- fetching

fn st(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_owned()
}
fn arr(v: &Value) -> &[Value] {
    v.as_array().map(|a| a.as_slice()).unwrap_or(&[])
}
fn fields(v: &Value) -> Vec<Field> {
    if !v.is_array() {
        return Vec::new();
    }
    Form::parse(&json!({ "id": "", "fields": v })).map(|f| f.fields).unwrap_or_default()
}

fn fetch_integrations(api: &Api, dir: &str) -> Result<Vec<Integration>, String> {
    let v = api.get(&format!("/api/integration?{}", location(dir)))?;
    let mut out: Vec<Integration> = arr(&v["data"])
        .iter()
        .map(|i| {
            let mut it = Integration { id: st(i, "id"), name: st(i, "name"), key: false, key_fields: Vec::new(), logins: Vec::new(), externals: Vec::new(), env: Vec::new(), conns: Vec::new() };
            for m in arr(&i["methods"]) {
                match m["type"].as_str() {
                    Some("key") => {
                        it.key = true;
                        it.key_fields = fields(&m["form"]);
                    }
                    Some("oauth") => it.logins.push((st(m, "id"), st(m, "label"), fields(&m["form"]))),
                    Some("external") => it.externals.push((st(m, "id"), st(m, "label"))),
                    Some("env") => it.env = arr(&m["names"]).iter().filter_map(|n| n.as_str().map(str::to_owned)).collect(),
                    _ => {}
                }
            }
            for c in arr(&i["connections"]) {
                // note: we never ask for /api/credential, so the secret itself never enters this process.
                it.conns.push(match c["type"].as_str() {
                    Some("env") => Conn { cred: None, label: format!("${}", st(c, "name")), method: "env".to_owned() },
                    _ => Conn { cred: c["id"].as_str().map(str::to_owned), label: st(c, "label"), method: st(c, "method") },
                });
            }
            it
        })
        .collect();
    out.sort_by_key(|i| i.name.to_lowercase());
    Ok(out)
}

// active providers that point at this machine. not worth failing the whole tab over, so errors = none.
fn fetch_locals(api: &Api, dir: &str) -> Vec<Local> {
    let Ok(v) = api.get(&format!("/api/provider?{}", location(dir))) else { return Vec::new() };
    arr(&v["data"])
        .iter()
        .filter_map(|p| {
            let url = p["settings"]["baseURL"].as_str()?;
            let host = url.split("://").nth(1).unwrap_or(url);
            let local = ["127.0.0.1", "localhost", "[::1]", "0.0.0.0"].iter().any(|h| host.starts_with(h));
            local.then(|| Local { id: st(p, "id"), name: st(p, "name"), url: url.to_owned() })
        })
        .collect()
}

fn fetch_mcp(api: &Api, dir: &str) -> Result<Vec<McpServer>, String> {
    let v = api.get(&format!("/api/mcp?{}", location(dir)))?;
    Ok(arr(&v["data"]).iter().map(|m| McpServer { name: st(m, "name"), status: st(&m["status"], "status"), error: st(&m["status"], "error") }).collect())
}

fn fetch_plugins(api: &Api, dir: &str) -> Result<Vec<Plugin>, String> {
    let v = api.get(&format!("/api/plugin?{}", location(dir)))?;
    let mut out: Vec<Plugin> = arr(&v["data"])
        .iter()
        .map(|p| {
            let s = &p["source"];
            let kind = st(s, "type");
            let (source, target) = match kind.as_str() {
                "package" => {
                    let t = st(s, "target");
                    let ver = s["version"].as_str().map(|v| format!("@{v}")).unwrap_or_default();
                    (format!("package {t}{ver}"), Some(t))
                }
                "local" => (format!("local {}", st(s, "path")), None),
                other => (other.to_owned(), None),
            };
            let id = p["id"].as_str().map(str::to_owned).or(target.clone()).unwrap_or_else(|| source.clone());
            Plugin { id, source, builtin: kind == "builtin", target, outdated: s["outdated"].as_bool() == Some(true), error: st(&p["state"], "error") }
        })
        .collect();
    out.sort_by(|a, b| a.builtin.cmp(&b.builtin).then(a.id.cmp(&b.id)));
    Ok(out)
}

fn fetch_skills(api: &Api, dir: &str) -> Result<Vec<Skill>, String> {
    let v = api.get(&format!("/api/skill?{}", location(dir)))?;
    Ok(arr(&v["data"]).iter().map(|s| Skill { name: st(s, "name"), description: st(s, "description"), path: st(s, "path") }).collect())
}

fn fetch_saved(api: &Api) -> Result<Vec<SavedPerm>, String> {
    let v = api.get("/api/permission/saved")?;
    Ok(arr(&v["data"]).iter().map(|s| SavedPerm { id: st(s, "id"), action: st(s, "action"), resource: st(s, "resource") }).collect())
}

// "npx -y foo --flag 'two words'" -> argv. just enough quoting to be useful.
fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

// ---------------------------------------------------------------- state changes

impl App {
    fn load_tab(&mut self) {
        let tab = self.st.tab;
        self.st.loaded = Some(tab);
        self.st.loaded_at = self.now();
        self.st.dirty = false;
        let dir = self.dir();
        let wrap = |r: Result<SMsg, String>| r.map(Msg::Settings).unwrap_or_else(Msg::Err);
        match tab {
            Tab::General => {}
            Tab::Providers => {
                self.st.customs = crate::custom::list();
                self.spawn(move |api| wrap(fetch_integrations(api, &dir).map(|i| SMsg::Integrations(i, fetch_locals(api, &dir)))))
            }
            Tab::Ollama => self.ollama_load(),
            Tab::Mcp => self.spawn(move |api| wrap(fetch_mcp(api, &dir).map(SMsg::Mcp))),
            Tab::Plugins => self.spawn(move |api| wrap(fetch_plugins(api, &dir).map(SMsg::Plugins))),
            Tab::Skills => self.spawn(move |api| wrap(fetch_skills(api, &dir).map(SMsg::Skills))),
            Tab::Permissions => self.spawn(move |api| wrap(fetch_saved(api).map(SMsg::Saved))),
        }
    }

    pub fn on_settings(&mut self, m: SMsg) {
        match m {
            SMsg::Integrations(v, l) => {
                self.st.integrations = v;
                self.st.locals = l;
            }
            SMsg::Mcp(v) => self.st.mcp = v,
            SMsg::Plugins(v) => self.st.plugins = v,
            SMsg::Skills(v) => self.st.skills = v,
            SMsg::Saved(v) => self.st.saved = v,
            SMsg::Changed => {
                self.st.loaded = None; // refetch the open tab right away
                self.meta_dirty = true; // a new key means new models
            }
            SMsg::Ollama(m) => self.on_ollama(m),
            SMsg::Custom(Ok(())) => {
                let s = &mut self.st;
                s.c_busy = false;
                s.c_open = false;
                for f in [&mut s.c_name, &mut s.c_url, &mut s.c_key, &mut s.c_models, &mut s.c_error] {
                    f.clear();
                }
                s.customs = crate::custom::list();
                self.restart_server(); // opencode only reads that file when it starts
            }
            SMsg::Custom(Err(e)) => {
                self.st.c_busy = false;
                if self.st.c_open { self.st.c_error = e } else { self.error(e) }
            }
            SMsg::OAuth(o) => self.st.oauth = Some(o),
            SMsg::OAuthStatus(status, message) => match status.as_str() {
                "pending" => {}
                "complete" => {
                    self.st.oauth = None;
                    self.st.loaded = None;
                    self.meta_dirty = true;
                }
                _ => {
                    if let Some(o) = &mut self.st.oauth {
                        o.error = if message.is_empty() { format!("sign-in {status}") } else { message };
                    }
                }
            },
        }
    }

    // a write where success just means "refetch".
    fn mutate(&self, method: &'static str, path: String, body: Option<Value>) {
        self.spawn(move |api| match api.req(method, &path, body.as_ref()) {
            Ok(_) => Msg::Settings(SMsg::Changed),
            Err(e) => Msg::Err(e),
        });
    }

    fn exec(&mut self, op: Op) {
        let loc = location(&self.dir());
        match op {
            Op::SaveKey(id) => {
                let key = std::mem::take(&mut self.st.key_input); // don't leave the secret sitting in ui state
                let answer: serde_json::Map<String, Value> = self.st.answers.drain().filter(|(_, v)| !v.trim().is_empty()).map(|(k, v)| (k, json!(v.trim()))).collect();
                self.st.editing = None;
                let mut body = json!({ "key": key.trim() });
                if !answer.is_empty() {
                    body["answer"] = Value::Object(answer);
                }
                self.mutate("POST", format!("/api/integration/{id}/connect/key?{loc}"), Some(body));
            }
            Op::RemoveCred(cred) => self.mutate("DELETE", format!("/api/credential/{cred}"), None),
            Op::Login(id, method) => {
                let answer: serde_json::Map<String, Value> = self.st.answers.drain().filter(|(_, v)| !v.trim().is_empty()).map(|(k, v)| (k, json!(v.trim()))).collect();
                self.st.editing = None;
                let mut body = json!({ "methodID": method });
                if !answer.is_empty() {
                    body["answer"] = Value::Object(answer);
                }
                let ctx = self.ctx.clone();
                self.spawn(move |api| match api.post(&format!("/api/integration/{id}/connect/oauth?{loc}"), body) {
                    Ok(v) => {
                        let d = &v["data"];
                        let url = st(d, "url");
                        ctx.open_url(egui::OpenUrl::new_tab(&url));
                        Msg::Settings(SMsg::OAuth(OAuth {
                            integration: id,
                            attempt: st(d, "attemptID"),
                            url,
                            instructions: st(d, "instructions"),
                            needs_code: d["mode"].as_str() == Some("code"),
                            code: String::new(),
                            error: String::new(),
                            polled: 0.0,
                        }))
                    }
                    Err(e) => Msg::Err(e),
                });
            }
            // nothing to type or wait for: the server just records "use that program's login".
            Op::External(id, method) => self.mutate("POST", format!("/api/integration/{id}/connect/external?{loc}"), Some(json!({ "methodID": method }))),
            Op::CustomAdd => {
                let s = &mut self.st;
                if s.c_busy {
                    return;
                }
                s.c_busy = true;
                s.c_error.clear();
                // the key stays in the form until this works, so a typo in the url doesn't cost you a re-paste.
                let (name, url, key, models) = (s.c_name.clone(), s.c_url.clone(), s.c_key.clone(), s.c_models.clone());
                let taken: Vec<String> = s.integrations.iter().map(|i| i.id.clone()).collect();
                self.spawn(move |_| Msg::Settings(SMsg::Custom(crate::custom::add(&name, &url, &key, &models, &taken))));
            }
            Op::CustomRemove(id) => self.spawn(move |_| Msg::Settings(SMsg::Custom(crate::custom::remove(&id)))),
            Op::CustomRefresh(id) => self.spawn(move |_| Msg::Settings(SMsg::Custom(crate::custom::refresh(&id)))),
            Op::LoginCode => {
                if let Some(o) = &mut self.st.oauth {
                    let path = format!("/api/integration/{}/connect/oauth/{}/complete?{loc}", o.integration, o.attempt);
                    let body = json!({ "code": std::mem::take(&mut o.code).trim() });
                    self.spawn(move |api| match api.post(&path, body) {
                        Ok(_) => Msg::Nop, // the status poll picks up the result
                        Err(e) => Msg::Err(e),
                    });
                }
            }
            Op::LoginCancel => {
                if let Some(o) = self.st.oauth.take() {
                    self.spawn(move |api| {
                        let _ = api.req("DELETE", &format!("/api/integration/{}/connect/oauth/{}?{loc}", o.integration, o.attempt), None);
                        Msg::Nop
                    });
                }
            }
            Op::McpToggle(name, connect) => {
                let verb = if connect { "connect" } else { "disconnect" };
                self.mutate("POST", format!("/api/experimental/mcp/{}/{verb}?{loc}", crate::api::enc(&name)), None);
            }
            Op::McpRemove(name) => {
                self.own_mcp.remove(&name);
                self.save_state();
                self.mutate("DELETE", format!("/api/experimental/mcp/{}?{loc}", crate::api::enc(&name)), None);
            }
            Op::McpAdd => {
                let name = self.st.mcp_name.trim().to_owned();
                let target = self.st.mcp_target.trim().to_owned();
                if name.is_empty() || target.is_empty() {
                    return;
                }
                let config = if target.starts_with("http://") || target.starts_with("https://") {
                    json!({ "type": "remote", "url": target })
                } else {
                    json!({ "type": "local", "command": split_command(&target) })
                };
                self.st.mcp_name.clear();
                self.st.mcp_target.clear();
                self.own_mcp.insert(name.clone(), config.clone());
                self.save_state();
                self.mutate("PUT", format!("/api/experimental/mcp/{}?{loc}", crate::api::enc(&name)), Some(json!({ "config": config })));
            }
            Op::PluginCheck => self.mutate("POST", format!("/api/plugin/check?{loc}"), Some(json!({}))),
            Op::PluginUpdate(target) => self.mutate("POST", format!("/api/plugin/update?{loc}"), Some(json!({ "targets": [target] }))),
            Op::RemoveSaved(id) => self.mutate("DELETE", format!("/api/permission/saved/{id}"), None),
            Op::OpenUrl(url) => self.ctx.open_url(egui::OpenUrl::new_tab(url)),
            Op::Act(a) => {
                if matches!(a, Action::Palette(_)) {
                    self.settings_open = false;
                }
                self.run(a);
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    pub fn settings_ui(&mut self, ctx: &egui::Context) {
        if !self.settings_open {
            return;
        }
        let p = self.pal;
        let now = self.now();

        if self.st.loaded != Some(self.st.tab) || (self.st.dirty && now - self.st.loaded_at > 0.7) {
            self.load_tab();
        }
        // poll a sign-in that's waiting on the browser.
        let loc = location(&self.dir());
        if let Some(o) = &mut self.st.oauth
            && o.error.is_empty()
            && now - o.polled > 2.0
        {
            o.polled = now;
            let path = format!("/api/integration/{}/connect/oauth/{}?{loc}", o.integration, o.attempt);
            self.spawn(move |api| match api.get(&path) {
                Ok(v) => Msg::Settings(SMsg::OAuthStatus(st(&v["data"], "status"), st(&v["data"], "message"))),
                Err(e) => Msg::Settings(SMsg::OAuthStatus("failed".to_owned(), e)),
            });
        }
        if self.st.oauth.is_some() {
            ctx.request_repaint_after_secs(1.0);
        }

        let mut ops: Vec<Op> = Vec::new();
        let screen = ctx.content_rect();
        let w = 780.0_f32.min(screen.width() - 40.0);
        let h = 520.0_f32.min(screen.height() - 90.0);
        let nav = 168.0_f32.min(w * 0.3);

        let frame = Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.line)).corner_radius(16).inner_margin(Margin::symmetric(14, 14));
        let res = egui::Modal::new(Id::unique("settings")).frame(frame).backdrop_color(Color32::from_black_alpha(110)).show(ctx, |ui| {
            ui.set_width(w);
            ui.set_max_width(w);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                ui.vertical(|ui| {
                    ui.set_width(nav);
                    ui.set_min_height(h);
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.add_space(4.0);
                    ui.label(RichText::new("  Settings").font(FontId::new(17.0, semibold())).color(p.text));
                    ui.add_space(8.0);
                    for (tab, label) in TABS {
                        let (r, resp) = ui.allocate_exact_size(vec2(nav - 8.0, 30.0), Sense::click());
                        let on = self.st.tab == tab;
                        if on || resp.hovered() {
                            ui.painter().rect_filled(r, 8, if on { p.raised } else { p.line_soft });
                        }
                        ui.painter().text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, label, sans(13.5), if on { p.text } else { p.dim });
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.st.tab = tab;
                            self.st.filter.clear();
                            self.st.confirm = None;
                        }
                    }
                });

                let (line, _) = ui.allocate_exact_size(vec2(1.0, h), Sense::hover());
                ui.painter().rect_filled(line, 0, p.line_soft);

                let cw = w - nav - 1.0;
                ui.vertical(|ui| {
                    ui.set_width(cw);
                    ui.set_max_width(cw);
                    ScrollArea::vertical().id_salt(("settings_tab", self.st.tab as u8)).max_height(h).auto_shrink(false).show(ui, |ui| {
                        Frame::new().inner_margin(Margin { left: 18, right: 16, top: 2, bottom: 8 }).show(ui, |ui| {
                            ui.set_width(cw - 34.0);
                            ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                            ui.spacing_mut().slider_width = 180.0;
                            match self.st.tab {
                                Tab::General => self.tab_general(ui, &mut ops),
                                Tab::Providers => self.tab_providers(ui, &mut ops),
                                Tab::Ollama => self.tab_ollama(ui),
                                Tab::Mcp => self.tab_mcp(ui, &mut ops),
                                Tab::Plugins => self.tab_plugins(ui, &mut ops),
                                Tab::Skills => self.tab_skills(ui),
                                Tab::Permissions => self.tab_permissions(ui, &mut ops),
                            }
                        });
                    });
                });
            });
        });

        if res.should_close() {
            self.settings_open = false;
            self.st.key_input.clear();
            self.st.editing = None;
        }
        for op in ops {
            self.exec(op);
        }
    }

    fn tab_general(&mut self, ui: &mut Ui, ops: &mut Vec<Op>) {
        let p = self.pal;
        let ctx = ui.ctx().clone();
        let mut changed = false;
        let mut restyle = false;

        section(ui, &p, "Appearance");
        setting(ui, &p, "Accent", |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, a) in theme::ACCENTS.iter().enumerate().rev() {
                let (r, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                let col = theme::pal(i, 1.0).accent;
                ui.painter().circle_filled(r.center(), 8.0, col);
                if i == self.accent {
                    ui.painter().circle_stroke(r.center(), 11.0, Stroke::new(1.5, col));
                }
                if resp.on_hover_text(a.0).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    ops.push(Op::Act(Action::Accent(i)));
                }
            }
        });
        setting(ui, &p, "Window opacity", |ui| {
            ui.add_sized([38.0, 18.0], Label::new(RichText::new(format!("{}%", (self.opacity * 100.0).round())).size(12.5).color(p.dim)));
            if ui.add(egui::Slider::new(&mut self.opacity, 0.3..=1.0).show_value(false).trailing_fill(true)).changed() {
                restyle = true;
                changed = true;
            }
        });
        note(ui, &p, "below 100% the background goes see-through, so your compositor's blur shows behind it. text and cards stay solid.");
        setting(ui, &p, "Interface scale", |ui| {
            ui.add_sized([38.0, 18.0], Label::new(RichText::new(format!("{}%", (self.zoom * 100.0).round())).size(12.5).color(p.dim)));
            let r = ui.add(egui::Slider::new(&mut self.zoom, 0.7..=1.6).step_by(0.05).show_value(false).trailing_fill(true));
            // only rescale once the drag ends, otherwise the slider moves out from under the cursor.
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                ctx.set_zoom_factor(self.zoom);
                changed = true;
            }
        });

        section(ui, &p, "Transcript");
        setting(ui, &p, "Show thinking", |ui| changed |= toggle(ui, &p, &mut self.show_thinking));
        setting(ui, &p, "Open diffs by default", |ui| changed |= toggle(ui, &p, &mut self.open_diffs));

        section(ui, &p, "New sessions");
        setting(ui, &p, "Model", |ui| {
            let name = self.new_model.as_ref().or(self.default_model.as_ref()).map(|m| self.model_info(m).map(|i| i.name.clone()).unwrap_or_else(|| m.id.clone()));
            if chip(ui, &p, name.as_deref().unwrap_or("default"), true).clicked() {
                ops.push(Op::Act(Action::Palette(PaletteMode::Models)));
            }
        });
        setting(ui, &p, "Agent", |ui| {
            if chip(ui, &p, self.new_agent.as_deref().unwrap_or("default"), true).clicked() {
                ops.push(Op::Act(Action::Palette(PaletteMode::Agents)));
            }
        });

        section(ui, &p, "Server");
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 3.5, if self.sse { p.good } else { p.err });
            let state = if self.sse { "connected" } else { "reconnecting" };
            ui.label(RichText::new(format!("{state} · {}", self.api.base())).font(mono(12.0)).color(p.dim));
        });

        if restyle {
            self.restyle();
        }
        if changed {
            self.save_state();
            self.heights.clear(); // row heights depend on these
        }
    }

    fn tab_providers(&mut self, ui: &mut Ui, ops: &mut Vec<Op>) {
        let p = self.pal;
        let s = &mut self.st;

        // sign-in in progress
        if let Some(o) = &mut s.oauth {
            let name = s.integrations.iter().find(|i| i.id == o.integration).map(|i| i.name.as_str()).unwrap_or(&o.integration);
            Frame::new().fill(p.inset).stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.6))).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(format!("Signing in to {name}")).font(FontId::new(14.0, medium())).color(p.text));
                if !o.instructions.is_empty() {
                    ui.add(Label::new(RichText::new(&o.instructions).size(13.0).color(p.dim)).wrap());
                }
                ui.add(Label::new(RichText::new(&o.url).font(mono(11.5)).color(p.faint)).truncate());
                if !o.error.is_empty() {
                    ui.add(Label::new(RichText::new(&o.error).size(13.0).color(p.err)).wrap());
                }
                if o.needs_code {
                    field_box(ui, &p, |ui| ui.add(TextEdit::singleline(&mut o.code).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new("paste the code from the browser").color(p.faint))));
                }
                ui.horizontal(|ui| {
                    if o.needs_code && solid_button(ui, &p, "Submit code", true).clicked() && !o.code.trim().is_empty() {
                        ops.push(Op::LoginCode);
                    }
                    if solid_button(ui, &p, "Open browser again", false).clicked() {
                        ops.push(Op::OpenUrl(o.url.clone()));
                    }
                    if solid_button(ui, &p, "Cancel", false).clicked() {
                        ops.push(Op::LoginCancel);
                    }
                    if o.error.is_empty() && !o.needs_code {
                        ui.label(RichText::new("waiting for the browser…").size(12.5).color(p.faint));
                    }
                });
            });
        }

        // pinned slots, so the usual suspects don't have to be dug out of the full provider list.
        section(ui, &p, "Quick connect");
        if s.integrations.is_empty() {
            note(ui, &p, "loading providers…");
        }
        for pin in &PINNED {
            pinned_row(ui, &p, pin, &s.integrations, &mut s.editing, &mut s.key_input, &mut s.answers, ops);
        }
        // custom slots: any other server that speaks the openai api.
        for c in &s.customs {
            Sides::new().height(30.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    dot(ui, p.good);
                    ui.label(RichText::new(&c.name).size(14.0).color(p.text));
                    ui.label(RichText::new(&c.url).font(mono(11.5)).color(p.faint));
                },
                |ui| {
                    if confirm_button(ui, &p, &mut s.confirm, &format!("custom/{}", c.id), "Remove") {
                        ops.push(Op::CustomRemove(c.id.clone()));
                    }
                    if solid_button(ui, &p, "Refresh", false).on_hover_text("ask it for its model list again").clicked() {
                        ops.push(Op::CustomRefresh(c.id.clone()));
                    }
                    ui.label(RichText::new(if c.models == 1 { "1 model".to_owned() } else { format!("{} models", c.models) }).size(12.5).color(p.dim));
                },
            );
        }
        // they get to the server through a config file handed over at launch, so cloverleaf has to be the one launching it.
        let blocked = std::env::var_os("CLOVERLEAF_SERVER").is_some() || std::env::var_os("OPENCODE_CONFIG").is_some();
        if blocked {
            note(ui, &p, "custom providers are off: cloverleaf passes them to the opencode server it launches, and here it's attached to an outside one (CLOVERLEAF_SERVER) or OPENCODE_CONFIG is already set.");
        } else if !s.c_open {
            if solid_button(ui, &p, "Add custom provider", false).on_hover_text("any server that speaks the openai api").clicked() {
                s.c_open = true;
            }
        } else {
            Frame::new().fill(p.inset).corner_radius(10).inner_margin(12).show(ui, |ui| {
                ui.set_width(ui.available_width());
                let line = |ui: &mut Ui, val: &mut String, hint: &str, secret: bool| {
                    field_box(ui, &p, |ui| ui.add(TextEdit::singleline(val).password(secret).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new(hint).color(p.faint))));
                };
                line(ui, &mut s.c_name, "name", false);
                line(ui, &mut s.c_url, "base url, like https://host/v1", false);
                line(ui, &mut s.c_key, "api key (leave empty if it doesn't want one)", true);
                line(ui, &mut s.c_models, "models, comma separated (leave empty to ask the server)", false);
                if !s.c_error.is_empty() {
                    ui.add(Label::new(RichText::new(&s.c_error).size(13.0).color(p.err)).wrap());
                }
                ui.horizontal(|ui| {
                    if solid_button(ui, &p, "Add provider", true).clicked() && !s.c_name.trim().is_empty() && !s.c_url.trim().is_empty() {
                        ops.push(Op::CustomAdd);
                    }
                    if solid_button(ui, &p, "Cancel", false).clicked() {
                        s.c_open = false;
                        s.c_key.clear();
                        s.c_error.clear();
                    }
                    if s.c_busy {
                        ui.label(RichText::new("checking…").size(12.5).color(p.faint));
                    }
                });
                note(ui, &p, "for anything that speaks the openai api. adding or removing one restarts the opencode server, which stops whatever is running. unlike the slots above, this key is kept by cloverleaf itself, in ~/.config/cloverleaf/providers.json (readable only by you): opencode's key store can't hold keys for providers it doesn't know.");
            });
        }

        note(ui, &p, "a subscription login bills your Claude or ChatGPT plan instead of api credits. the Claude one runs Claude Code in the background, so `claude` has to be installed and logged in.");

        section(ui, &p, "Local models");
        for l in &s.locals {
            let n = self.models.iter().filter(|m| m.provider == l.id).count();
            Sides::new().height(30.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    dot(ui, if n > 0 { p.good } else { p.faint });
                    ui.label(RichText::new(&l.name).size(14.0).color(p.text));
                    ui.label(RichText::new(&l.url).font(mono(11.5)).color(p.faint));
                },
                |ui| {
                    ui.label(RichText::new(if n == 1 { "1 model".to_owned() } else { format!("{n} models") }).size(12.5).color(p.dim));
                },
            );
        }
        if s.locals.is_empty() {
            note(ui, &p, "no local server active. start ollama and reopen this tab; if it still isn't listed, add it under `providers` in opencode.json.");
        } else {
            note(ui, &p, "no key needed. these show up in the model picker like any other model.");
        }

        section(ui, &p, "Connected");
        let mut any = false;
        for i in &s.integrations {
            for c in &i.conns {
                any = true;
                let key = format!("cred/{}", c.cred.as_deref().unwrap_or(&c.label));
                Sides::new().height(30.0).shrink_left().truncate().show(
                    ui,
                    |ui| {
                        dot(ui, p.good);
                        ui.label(RichText::new(&i.name).size(14.0).color(p.text));
                        let how = if c.method == "env" { c.label.clone() } else { c.method.clone() };
                        ui.label(RichText::new(how).size(12.5).color(p.faint));
                    },
                    |ui| match &c.cred {
                        Some(cred) => {
                            if confirm_button(ui, &p, &mut s.confirm, &key, "Remove") {
                                ops.push(Op::RemoveCred(cred.clone()));
                            }
                        }
                        None => {
                            ui.label(RichText::new("from environment").size(12.5).color(p.faint));
                        }
                    },
                );
            }
        }
        if !any {
            note(ui, &p, "nothing connected yet. add a key or sign in below.");
        }

        section(ui, &p, "Add a provider");
        field_box(ui, &p, |ui| ui.add(TextEdit::singleline(&mut s.filter).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new(format!("search {} providers", s.integrations.len())).color(p.faint))));
        let q = s.filter.to_lowercase();
        let matches: Vec<&Integration> = s.integrations.iter().filter(|i| q.is_empty() || i.name.to_lowercase().contains(&q) || i.id.contains(&q)).collect();
        const SHOWN: usize = 40;
        for i in matches.iter().take(SHOWN) {
            provider_row(ui, &p, i, &mut s.editing, &mut s.key_input, &mut s.answers, ops);
        }
        if matches.len() > SHOWN {
            note(ui, &p, &format!("…and {} more. type to narrow it down.", matches.len() - SHOWN));
        }
    }

    fn tab_mcp(&mut self, ui: &mut Ui, ops: &mut Vec<Op>) {
        let p = self.pal;
        let s = &mut self.st;

        section(ui, &p, "Servers");
        if s.mcp.is_empty() {
            note(ui, &p, "no mcp servers configured for this project.");
        }
        for m in &s.mcp {
            let up = m.status == "connected";
            let own = self.own_mcp.contains_key(&m.name);
            let col = match m.status.as_str() {
                "connected" => p.good,
                "failed" => p.err,
                "pending" => p.accent,
                _ => p.faint,
            };
            Sides::new().height(30.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    dot(ui, col);
                    ui.label(RichText::new(&m.name).font(mono(13.5)).color(p.text));
                    ui.label(RichText::new(&m.status).size(12.5).color(p.faint));
                    if !own {
                        ui.label(RichText::new("opencode.json").size(12.0).color(p.faint)).on_hover_text("defined in your opencode config; remove it there");
                    }
                },
                |ui| {
                    if own && confirm_button(ui, &p, &mut s.confirm, &format!("mcp/{}", m.name), "Remove") {
                        ops.push(Op::McpRemove(m.name.clone()));
                    }
                    if solid_button(ui, &p, if up { "Disconnect" } else { "Connect" }, false).clicked() {
                        ops.push(Op::McpToggle(m.name.clone(), !up));
                    }
                },
            );
            if !m.error.is_empty() {
                ui.add(Label::new(RichText::new(&m.error).font(mono(12.0)).color(p.err)).wrap());
            }
        }

        section(ui, &p, "Add a server");
        field_box(ui, &p, |ui| ui.add(TextEdit::singleline(&mut s.mcp_name).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new("name").color(p.faint))));
        field_box(ui, &p, |ui| {
            ui.add(TextEdit::singleline(&mut s.mcp_target).frame(Frame::NONE).desired_width(f32::INFINITY).font(mono(13.0)).hint_text(RichText::new("npx -y some-mcp-server   or   https://host/mcp").color(p.faint)))
        });
        if solid_button(ui, &p, "Add server", true).clicked() {
            ops.push(Op::McpAdd);
        }
        note(ui, &p, "a command runs a local server over stdio, a url connects to a remote one. servers added here are remembered by cloverleaf and re-added each launch, so the tui won't see them; put a server in opencode.json (also needed for env vars, headers, oauth) to share it.");
    }

    fn tab_plugins(&mut self, ui: &mut Ui, ops: &mut Vec<Op>) {
        let p = self.pal;
        let s = &mut self.st;
        let builtin = s.plugins.iter().filter(|x| x.builtin).count();

        section(ui, &p, "Installed");
        let mut any = false;
        for pl in s.plugins.iter().filter(|x| !x.builtin || s.show_builtin) {
            any = true;
            Sides::new().height(28.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    dot(ui, if pl.error.is_empty() { p.good } else { p.err });
                    ui.label(RichText::new(&pl.id).font(mono(13.0)).color(p.text));
                    ui.label(RichText::new(&pl.source).size(12.5).color(p.faint));
                },
                |ui| {
                    if pl.outdated
                        && let Some(t) = &pl.target
                        && solid_button(ui, &p, "Update", true).clicked()
                    {
                        ops.push(Op::PluginUpdate(t.clone()));
                    }
                },
            );
            if !pl.error.is_empty() {
                ui.add(Label::new(RichText::new(&pl.error).font(mono(12.0)).color(p.err)).wrap());
            }
        }
        if !any {
            note(ui, &p, "no third-party plugins installed.");
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if solid_button(ui, &p, "Check for updates", false).clicked() {
                ops.push(Op::PluginCheck);
            }
        });
        setting(ui, &p, &format!("Show the {builtin} built-in plugins"), |ui| {
            toggle(ui, &p, &mut s.show_builtin);
        });
        note(ui, &p, "installing a plugin still happens in your opencode config: the server has no install endpoint yet, so there's no button for it here.");
    }

    fn tab_skills(&mut self, ui: &mut Ui) {
        let p = self.pal;
        section(ui, &p, "Available skills");
        if self.st.skills.is_empty() {
            note(ui, &p, "no skills found.");
        }
        for sk in &self.st.skills {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&sk.name).font(FontId::new(14.0, medium())).color(p.text));
                ui.add(Label::new(RichText::new(crate::ui::tilde(&sk.path)).font(mono(11.5)).color(p.faint)).truncate());
            });
            if !sk.description.is_empty() {
                ui.add(Label::new(RichText::new(crate::ui::first_line(&sk.description, 220)).size(12.5).color(p.dim)).wrap());
            }
            ui.add_space(2.0);
        }
    }

    fn tab_permissions(&mut self, ui: &mut Ui, ops: &mut Vec<Op>) {
        let p = self.pal;
        let s = &mut self.st;
        section(ui, &p, "Always allowed");
        if s.saved.is_empty() {
            note(ui, &p, "nothing saved. rules show up here when you pick \"Always allow\" on a permission prompt.");
        }
        for r in &s.saved {
            Sides::new().height(28.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    ui.label(RichText::new(&r.action).font(mono(13.0)).color(p.accent));
                    ui.label(RichText::new(&r.resource).font(mono(13.0)).color(p.dim));
                },
                |ui| {
                    if confirm_button(ui, &p, &mut s.confirm, &format!("perm/{}", r.id), "Remove") {
                        ops.push(Op::RemoveSaved(r.id.clone()));
                    }
                },
            );
        }
    }
}

// ---------------------------------------------------------------- widgets

// a pinned slot at the top of the providers tab. one slot can front two integrations on the
// server: the one that takes an api key, and the one that logs in with a subscription.
struct Pin {
    title: &'static str,
    key: &'static str,                                        // integration that takes the api key
    sub: Option<(&'static str, &'static str, &'static str)>, // (integration, method id, button label)
}

const PINNED: [Pin; 3] = [
    Pin { title: "Claude", key: "anthropic", sub: Some(("claude-agent", "claude-login", "Use Claude Code login")) },
    Pin { title: "OpenAI", key: "openai", sub: Some(("openai", "chatgpt-token-sharing", "Sign in with ChatGPT")) },
    Pin { title: "Ollama", key: "ollama-cloud", sub: None },
];

#[allow(clippy::too_many_arguments)]
fn pinned_row(ui: &mut Ui, p: &Pal, pin: &Pin, all: &[Integration], editing: &mut Option<String>, key_input: &mut String, answers: &mut HashMap<String, String>, ops: &mut Vec<Op>) {
    let key_i = all.iter().find(|i| i.id == pin.key);
    let sub_i = pin.sub.and_then(|(id, _, _)| all.iter().find(|i| i.id == id));
    if key_i.is_none() && sub_i.is_none() {
        return; // this opencode doesn't know the provider
    }

    // how it's connected right now, across both integrations.
    let mut how: Vec<&str> = Vec::new();
    let mut seen: Vec<&str> = Vec::new();
    for i in key_i.iter().chain(sub_i.iter()) {
        if seen.contains(&i.id.as_str()) {
            continue; // key and subscription can live on the same integration (openai)
        }
        seen.push(&i.id);
        for c in &i.conns {
            let h = match c.method.as_str() {
                "key" => "api key",
                "env" => "key from env",
                _ => "subscription",
            };
            if !how.contains(&h) {
                how.push(h);
            }
        }
    }
    let status = how.join(" + "); // empty = not connected: the grey dot says so, and the buttons need the room

    // the subscription button: only if the server really offers that method and it isn't connected yet.
    let sub_op = pin.sub.zip(sub_i).filter(|_| !how.contains(&"subscription")).and_then(|((id, method, label), i)| {
        if i.externals.iter().any(|m| m.0 == method) {
            Some((label, Op::External(id.to_owned(), method.to_owned())))
        } else if i.logins.iter().any(|l| l.0 == method) {
            Some((label, Op::Login(id.to_owned(), method.to_owned())))
        } else {
            None
        }
    });

    let slot = format!("pin/{}", pin.key);
    let open = editing.as_deref() == Some(slot.as_str());
    let mut toggle_open = false;
    Sides::new().height(30.0).shrink_left().truncate().show(
        ui,
        |ui| {
            dot(ui, if how.is_empty() { p.faint } else { p.good });
            ui.label(RichText::new(pin.title).size(14.0).color(p.text));
            ui.label(RichText::new(&status).size(12.5).color(p.faint));
        },
        |ui| {
            // right-to-left: the key button ends up rightmost.
            if key_i.is_some_and(|i| i.key) {
                toggle_open = solid_button(ui, p, if open { "Close" } else { "Add key" }, false).clicked();
            }
            if let Some((label, op)) = sub_op
                && solid_button(ui, p, label, false).clicked()
            {
                ops.push(op);
            }
        },
    );
    if toggle_open {
        *editing = if open { None } else { Some(slot) };
        key_input.clear();
        answers.clear();
    }
    if open && let Some(i) = key_i {
        connect_form(ui, p, i, &[], key_input, answers, ops);
    }
}

// one line of the full provider list, with its "add key / sign in" form underneath while it's open.
fn provider_row(ui: &mut Ui, p: &Pal, i: &Integration, editing: &mut Option<String>, key_input: &mut String, answers: &mut HashMap<String, String>, ops: &mut Vec<Op>) {
    let open = editing.as_deref() == Some(i.id.as_str());
    let mut toggle_open = false;
    let mut external = false;
    Sides::new().height(30.0).shrink_left().truncate().show(
        ui,
        |ui| {
            ui.label(RichText::new(&i.name).size(14.0).color(p.text));
            if let Some(e) = i.env.first() {
                ui.label(RichText::new(format!("${e}")).font(mono(11.5)).color(p.faint)).on_hover_text("also picked up from this environment variable");
            }
        },
        |ui| {
            if i.key || !i.logins.is_empty() {
                let label = if open { "Close" } else if i.key { "Add key" } else { "Sign in" };
                toggle_open = solid_button(ui, p, label, false).clicked();
            } else if let Some((_, label)) = i.externals.first() {
                external = solid_button(ui, p, "Connect", false).on_hover_text(label).clicked();
            }
        },
    );
    if external && let Some((method, _)) = i.externals.first() {
        ops.push(Op::External(i.id.clone(), method.clone()));
    }
    if toggle_open {
        *editing = if open { None } else { Some(i.id.clone()) };
        key_input.clear();
        answers.clear();
    }
    if open {
        connect_form(ui, p, i, &i.logins, key_input, answers, ops);
    }
}

// the box under an open row: extra questions, the key field, and a button per login method in `logins`.
fn connect_form(ui: &mut Ui, p: &Pal, i: &Integration, logins: &[(String, String, Vec<Field>)], key_input: &mut String, answers: &mut HashMap<String, String>, ops: &mut Vec<Op>) {
    Frame::new().fill(p.inset).corner_radius(10).inner_margin(12).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let extra: Vec<&Field> = i.key_fields.iter().chain(logins.iter().flat_map(|l| l.2.iter())).collect();
        let mut seen: Vec<&str> = Vec::new();
        for f in extra {
            if seen.contains(&f.key.as_str()) || f.kind == "external" {
                continue;
            }
            seen.push(&f.key);
            let title = if f.title.is_empty() { f.key.clone() } else { f.title.clone() };
            ui.label(RichText::new(title).size(13.0).color(p.dim));
            let val = answers.entry(f.key.clone()).or_default();
            field_box(ui, p, |ui| ui.add(TextEdit::singleline(val).frame(Frame::NONE).desired_width(f32::INFINITY)));
        }
        if i.key {
            field_box(ui, p, |ui| {
                ui.add(TextEdit::singleline(key_input).password(true).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new("paste api key").color(p.faint)))
            });
        }
        ui.horizontal_wrapped(|ui| {
            if i.key && solid_button(ui, p, "Save key", true).clicked() && !key_input.trim().is_empty() {
                ops.push(Op::SaveKey(i.id.clone()));
            }
            for (method, label, _) in logins {
                if solid_button(ui, p, label, !i.key).clicked() {
                    ops.push(Op::Login(i.id.clone(), method.clone()));
                }
            }
        });
        note(ui, p, "keys are sent straight to your local opencode server, which stores them. cloverleaf doesn't keep a copy.");
    });
}

pub(crate) fn dot(ui: &mut Ui, col: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 3.5, col);
}

pub(crate) fn field_box<R>(ui: &mut Ui, p: &Pal, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new().fill(p.bg.to_opaque()).stroke(Stroke::new(1.0, p.line_soft)).corner_radius(8).inner_margin(Margin::symmetric(8, 5)).show(ui, add).inner
}

// destructive button: first click arms it ("Sure?"), second click returns true.
pub(crate) fn confirm_button(ui: &mut Ui, p: &Pal, armed: &mut Option<String>, key: &str, label: &str) -> bool {
    let is_armed = armed.as_deref() == Some(key);
    let text = RichText::new(if is_armed { "Sure?" } else { label }).font(FontId::new(13.5, medium())).color(if is_armed { p.err } else { p.text });
    if ui.add(egui::Button::new(text).fill(p.raised).corner_radius(8)).clicked() {
        if is_armed {
            *armed = None;
            return true;
        }
        *armed = Some(key.to_owned());
    }
    false
}
