// the "Local models" settings tab: what's installed in ollama, what this machine can run, and
// downloading more. opencode has no api for any of this, so it talks to three other places:
//   the ollama server on this machine   installed + loaded models, pull, delete, unload
//   ollama.com/library                  the list of models you could download (html, scraped)
//   registry.ollama.ai                  the exact download size of one model:tag
// nothing about you is sent to the last two, they're plain anonymous GETs.

use crate::api::{Api, Msg, location};
use crate::app::App;
use crate::settings::{SMsg, confirm_button, dot, field_box};
use crate::theme::{Pal, medium, mono, sans};
use crate::ui::{first_line, note, section, solid_button};
use egui::{Color32, FontId, Frame, Label, RichText, Sense, Sides, Stroke, StrokeKind, TextEdit, Ui, pos2, vec2};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const GIB: u64 = 1 << 30;
const LIBRARY: &str = "https://ollama.com/library?sort=popular";
const REGISTRY: &str = "https://registry.ollama.ai/v2";
const SHOWN: usize = 30; // library rows drawn at once
const LOOKUPS: usize = 4; // size lookups allowed in flight at once

// ---------------------------------------------------------------- what fits

#[derive(Clone, Default)]
pub struct Hw {
    pub gpus: Vec<(String, u64)>, // (name, vram bytes)
    pub ram: u64,
}

impl Hw {
    fn vram(&self) -> u64 {
        self.gpus.iter().map(|g| g.1).sum()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Fit {
    Vram,  // whole thing on the gpu(s)
    Split, // spills into system ram: works, slow
    No,
}

// memory a model wants while running: the weights, ~10% runtime overhead, and 1 GiB for a modest
// context window. a rule of thumb, not a measurement: a big context window costs a lot more.
fn need(size: u64) -> u64 {
    size + size / 10 + GIB
}

fn fit(size: u64, hw: &Hw) -> Fit {
    let n = need(size);
    let vram = hw.vram();
    if vram > 0 && n <= vram {
        Fit::Vram
    } else if n <= vram + hw.ram.saturating_sub(4 * GIB) {
        Fit::Split // 4 GiB of ram stays with the rest of the system
    } else {
        Fit::No
    }
}

fn fit_text(f: Fit, hw: &Hw) -> &'static str {
    match f {
        Fit::Vram => "fits vram",
        Fit::Split if hw.gpus.is_empty() => "cpu + ram",
        Fit::Split => "vram + ram",
        Fit::No => "too big",
    }
}

fn fit_color(f: Fit, p: &Pal) -> Color32 {
    match f {
        Fit::Vram => p.good,
        Fit::Split => Color32::from_rgb(0xd9, 0xb4, 0x5a), // amber. not the accent: coral next to the red reads as the same thing
        Fit::No => p.err,
    }
}

fn detect_hw() -> Hw {
    let mut hw = Hw::default();
    // nvidia: ask the driver's own tool. one "name, MiB" line per card.
    if let Ok(out) = std::process::Command::new("nvidia-smi").args(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"]).output() {
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if let Some((name, mib)) = line.rsplit_once(',')
                && let Ok(mib) = mib.trim().parse::<u64>()
            {
                hw.gpus.push((name.trim().trim_start_matches("NVIDIA ").trim_start_matches("GeForce ").to_owned(), mib << 20));
            }
        }
    }
    // amd (and anything else that fills this in): the kernel exposes vram per card in sysfs.
    if hw.gpus.is_empty()
        && let Ok(dir) = std::fs::read_dir("/sys/class/drm")
    {
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if !name.starts_with("card") || name.contains('-') {
                continue; // card0-DP-1 etc are connectors, not cards
            }
            if let Ok(s) = std::fs::read_to_string(e.path().join("device/mem_info_vram_total"))
                && let Ok(bytes) = s.trim().parse::<u64>()
                && bytes > 0
            {
                hw.gpus.push((format!("gpu {}", name.trim_start_matches("card")), bytes));
            }
        }
    }
    if let Ok(s) = std::fs::read_to_string("/proc/meminfo") {
        let kb = s.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok());
        hw.ram = kb.unwrap_or(0) * 1024;
    }
    hw
}

// ---------------------------------------------------------------- data

pub struct Installed {
    name: String,
    size: u64,
    params: String,             // "27.3B"
    quant: String,              // "Q4_K_M"
    loaded: Option<(u64, u64)>, // in memory right now: (total bytes, bytes of that on the gpu)
}

pub struct LibModel {
    name: String,
    desc: String,
    caps: Vec<String>, // tools, vision, thinking, ...
    tags: Vec<String>, // the size variants: 4b, 12b, 27b
}

#[derive(Clone, Default)]
pub struct Pull {
    status: String,
    done: u64,
    total: u64,
    end: Option<Result<(), String>>, // set on the last message
}

struct PullRow {
    name: String,
    p: Pull,
    cancel: Arc<AtomicBool>,
}

pub enum OMsg {
    Local { base: String, hw: Option<Hw>, models: Result<Vec<Installed>, String> },
    Library(Result<Vec<LibModel>, String>),
    Size(String, Option<u64>), // ("name:tag", download bytes)
    Pull(String, Pull),
    Changed, // a delete/unload went through: refetch
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Filter {
    #[default]
    All,
    Vram,
    Split,
}

#[derive(Clone, Copy, PartialEq, Default)]
enum Lib {
    #[default]
    None,
    Loading,
    Done,
}

#[derive(Default)]
pub struct OllamaState {
    base: String, // where the ollama server is
    hw: Option<Hw>,
    installed: Vec<Installed>,
    error: String,
    loaded_once: bool,
    polled: f64,

    library: Vec<LibModel>,
    lib: Lib,
    lib_error: String,
    sizes: HashMap<String, Option<u64>>, // "name:tag" -> exact size. None = asked (or failed), use the estimate
    inflight: usize,

    pulls: Vec<PullRow>,
    query: String,
    filter: Filter,
    confirm: Option<String>,
}

// ---------------------------------------------------------------- http

pub(crate) fn agent(timeout: Option<Duration>) -> ureq::Agent {
    let cfg = ureq::Agent::config_builder().http_status_as_error(false).timeout_global(timeout).timeout_connect(Some(Duration::from_secs(5))).build();
    ureq::Agent::new_with_config(cfg)
}

pub(crate) const SHORT: Option<Duration> = Some(Duration::from_secs(30));

// body as text, or the server's error message.
pub(crate) fn finish(what: &str, res: Result<ureq::http::Response<ureq::Body>, ureq::Error>) -> Result<String, String> {
    let mut res = res.map_err(|e| format!("{what}: {e}"))?;
    let status = res.status().as_u16();
    let text = res.body_mut().with_config().limit(16 << 20).read_to_string().map_err(|e| format!("{what}: {e}"))?;
    if status >= 400 {
        let why = serde_json::from_str::<Value>(&text).ok().and_then(|v| v["error"].as_str().map(str::to_owned)).unwrap_or_else(|| format!("http {status}"));
        return Err(format!("{what}: {why}"));
    }
    Ok(text)
}

fn get_json(url: &str) -> Result<Value, String> {
    let text = finish(url, agent(SHORT).get(url).call())?;
    serde_json::from_str(&text).map_err(|e| format!("{url}: {e}"))
}

// opencode knows where ollama is if it's using it; otherwise the usual env var, otherwise the default port.
fn find_base(api: &Api, loc: &str) -> String {
    if let Ok(v) = api.get(&format!("/api/provider/ollama?{loc}"))
        && let Some(url) = v["data"]["settings"]["baseURL"].as_str()
    {
        return url.trim_end_matches('/').trim_end_matches("/v1").to_owned();
    }
    match std::env::var("OLLAMA_HOST") {
        Ok(h) if !h.trim().is_empty() => {
            let h = h.trim().trim_end_matches('/');
            if h.contains("://") { h.to_owned() } else { format!("http://{h}") }
        }
        _ => "http://127.0.0.1:11434".to_owned(),
    }
}

fn fetch_installed(base: &str) -> Result<Vec<Installed>, String> {
    let tags = get_json(&format!("{base}/api/tags"))?;
    let ps = get_json(&format!("{base}/api/ps")).unwrap_or(Value::Null);
    let running: &[Value] = ps["models"].as_array().map(|a| a.as_slice()).unwrap_or(&[]);
    let mut out: Vec<Installed> = tags["models"]
        .as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .filter_map(|m| {
            let name = m["name"].as_str()?.to_owned();
            let loaded = running.iter().find(|r| r["name"].as_str() == Some(&name)).map(|r| (r["size"].as_u64().unwrap_or(0), r["size_vram"].as_u64().unwrap_or(0)));
            let d = &m["details"];
            Some(Installed {
                name,
                size: m["size"].as_u64().unwrap_or(0),
                params: d["parameter_size"].as_str().unwrap_or("").to_owned(),
                quant: d["quantization_level"].as_str().unwrap_or("").to_owned(),
                loaded,
            })
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let i = s.find(start)? + start.len();
    let j = s[i..].find(end)?;
    Some(&s[i..i + j])
}

fn unescape(s: &str) -> String {
    s.replace("&#39;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ").replace("&amp;", "&")
}

// ollama.com has no json list of the library, so read it out of the page: one <li> per model, with
// the name in its link and little badge <span>s for capabilities and sizes. if the page changes
// shape this returns nothing and the tab falls back to "type a name".
fn parse_library(html: &str) -> Vec<LibModel> {
    let mut out: Vec<LibModel> = Vec::new();
    for block in html.split("<li ").skip(1) {
        let Some(name) = between(block, "href=\"/library/", "\"") else { continue };
        if name.is_empty() || out.iter().any(|m| m.name == name) {
            continue;
        }
        let desc = between(block, "<p class=\"max-w-lg", "</p>").and_then(|s| s.split_once('>')).map(|x| unescape(x.1.trim())).unwrap_or_default();
        let (mut caps, mut tags) = (Vec::new(), Vec::new());
        let mut rest = block;
        while let Some(i) = rest.find("px-2 py-0.5") {
            rest = &rest[i + 11..];
            let Some(text) = between(rest, ">", "</span>") else { break };
            let t = text.trim();
            if t.is_empty() || t.contains('<') {
                continue;
            }
            if est_size(t).is_some() { tags.push(t.to_owned()) } else { caps.push(t.to_owned()) }
        }
        out.push(LibModel { name: name.to_owned(), desc, caps, tags });
    }
    out
}

fn fetch_library() -> Result<Vec<LibModel>, String> {
    let html = finish("ollama.com", agent(SHORT).get(LIBRARY).call())?;
    let list = parse_library(&html);
    if list.is_empty() { Err("couldn't read the model list off ollama.com".to_owned()) } else { Ok(list) }
}

// "27b" -> rough download size, going by parameter count at the default 4-bit quantization
// (~0.68 GB per billion parameters). only used until the registry tells us the real number.
fn est_size(tag: &str) -> Option<u64> {
    let t = tag.to_lowercase();
    let per = match t.chars().last()? {
        'b' => 0.68e9,
        'm' => 0.68e6,
        _ => return None,
    };
    let num: String = t[..t.len() - 1].chars().skip_while(|c| !c.is_ascii_digit()).collect(); // "e2b" -> "2"
    let n: f64 = num.parse().ok()?;
    Some((n * per) as u64)
}

// exact download size of name:tag = the sum of its layers in the registry manifest.
fn fetch_size(key: &str) -> Option<u64> {
    let (name, tag) = key.split_once(':')?;
    let repo = if name.contains('/') { name.to_owned() } else { format!("library/{name}") };
    let url = format!("{REGISTRY}/{repo}/manifests/{tag}");
    let res = agent(SHORT).get(&url).header("Accept", "application/vnd.docker.distribution.manifest.v2+json").call();
    let v: Value = serde_json::from_str(&finish(&url, res).ok()?).ok()?;
    let total: u64 = v["layers"].as_array()?.iter().filter_map(|l| l["size"].as_u64()).sum();
    (total > 0).then_some(total)
}

fn delete_model(base: &str, name: &str) -> Result<(), String> {
    // a DELETE with a body, which ureq only sends if told to.
    let res = agent(SHORT).delete(format!("{base}/api/delete")).force_send_body().send_json(json!({ "model": name }));
    finish("delete", res).map(|_| ())
}

// an empty generate with keep_alive 0 is how you tell ollama to drop a model from memory.
fn unload_model(base: &str, name: &str) -> Result<(), String> {
    let res = agent(SHORT).post(format!("{base}/api/generate")).send_json(json!({ "model": name, "keep_alive": 0 }));
    finish("unload", res).map(|_| ())
}

// one long POST. ollama answers with a json line per progress tick until it's done.
fn pull_thread(base: String, name: String, cancel: Arc<AtomicBool>, tx: std::sync::mpsc::Sender<Msg>, ctx: egui::Context) {
    let send = |p: Pull| {
        let _ = tx.send(Msg::Settings(SMsg::Ollama(OMsg::Pull(name.clone(), p))));
        ctx.request_repaint();
    };
    let fail = |e: String| Pull { end: Some(Err(e)), ..Default::default() };

    let res = agent(None).post(format!("{base}/api/pull")).send_json(json!({ "model": name, "stream": true }));
    let res = match res {
        Ok(r) if r.status().as_u16() < 400 => r,
        other => return send(fail(finish("pull", other).err().unwrap_or_else(|| "pull failed".to_owned()))),
    };

    let mut layers: HashMap<String, (u64, u64)> = HashMap::new(); // digest -> (done, total)
    let mut cur = Pull { status: "starting".to_owned(), ..Default::default() };
    let mut last = Instant::now();
    let mut ok = false;
    for line in BufReader::new(res.into_body().into_reader()).lines() {
        if cancel.load(Ordering::Relaxed) {
            return send(fail("cancelled".to_owned())); // dropping the connection is what stops ollama
        }
        let Ok(line) = line else { break };
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        if let Some(e) = v["error"].as_str() {
            return send(fail(e.to_owned()));
        }
        cur.status = v["status"].as_str().unwrap_or("").to_owned();
        if let Some(d) = v["digest"].as_str() {
            layers.insert(d.to_owned(), (v["completed"].as_u64().unwrap_or(0), v["total"].as_u64().unwrap_or(0)));
        }
        cur.done = layers.values().map(|l| l.0).sum();
        cur.total = layers.values().map(|l| l.1).sum();
        ok |= cur.status == "success";
        // a pull ticks many times a second; the ui doesn't need all of them.
        if last.elapsed() > Duration::from_millis(120) {
            last = Instant::now();
            send(cur.clone());
        }
    }
    if ok {
        cur.done = cur.total; // the last tick of a layer often stops a bit short of its total
    }
    cur.end = Some(if ok { Ok(()) } else { Err("the connection to ollama dropped".to_owned()) });
    send(cur);
}

// ---------------------------------------------------------------- state changes

enum Op {
    Pull(String),
    Cancel(String),
    Dismiss(String),
    Delete(String),
    Unload(String),
}

impl App {
    // installed + loaded models (and the hardware, once). also kicks off the library fetch the first time.
    pub fn ollama_load(&mut self) {
        let want_hw = self.st.ol.hw.is_none();
        let loc = location(&self.dir());
        self.st.ol.polled = self.now();
        self.spawn(move |api| {
            let base = find_base(api, &loc);
            let hw = want_hw.then(detect_hw);
            let models = fetch_installed(&base);
            Msg::Settings(SMsg::Ollama(OMsg::Local { base, hw, models }))
        });
        if self.st.ol.lib == Lib::None {
            self.st.ol.lib = Lib::Loading;
            self.spawn(|_| Msg::Settings(SMsg::Ollama(OMsg::Library(fetch_library()))));
        }
    }

    pub fn on_ollama(&mut self, m: OMsg) {
        let o = &mut self.st.ol;
        match m {
            OMsg::Local { base, hw, models } => {
                o.base = base;
                o.loaded_once = true;
                if hw.is_some() {
                    o.hw = hw;
                }
                match models {
                    Ok(list) => {
                        o.installed = list;
                        o.error.clear();
                    }
                    Err(e) => o.error = e,
                }
            }
            OMsg::Library(Ok(list)) => {
                o.library = list;
                o.lib = Lib::Done;
            }
            OMsg::Library(Err(e)) => {
                o.lib_error = e;
                o.lib = Lib::Done;
            }
            OMsg::Size(key, size) => {
                o.inflight = o.inflight.saturating_sub(1);
                o.sizes.insert(key, size);
            }
            OMsg::Pull(name, p) => {
                let end = p.end.clone();
                if let Some(row) = o.pulls.iter_mut().find(|r| r.name == name) {
                    row.p = p;
                }
                match end {
                    Some(Ok(())) => {
                        o.pulls.retain(|r| r.name != name);
                        self.ollama_load();
                        self.meta_dirty = true; // opencode should list the new model
                    }
                    Some(Err(e)) if e == "cancelled" => o.pulls.retain(|r| r.name != name),
                    _ => {} // still going, or failed: a failed row stays up until dismissed
                }
            }
            OMsg::Changed => {
                self.ollama_load();
                self.meta_dirty = true;
            }
        }
    }

    fn ollama_exec(&mut self, op: Op) {
        let base = self.st.ol.base.clone();
        match op {
            Op::Pull(name) => {
                if self.st.ol.pulls.iter().any(|r| r.name == name) {
                    return;
                }
                let cancel = Arc::new(AtomicBool::new(false));
                self.st.ol.pulls.push(PullRow { name: name.clone(), p: Pull { status: "starting".to_owned(), ..Default::default() }, cancel: cancel.clone() });
                let (tx, ctx) = self.channel();
                std::thread::spawn(move || pull_thread(base, name, cancel, tx, ctx));
            }
            Op::Cancel(name) => {
                if let Some(r) = self.st.ol.pulls.iter().find(|r| r.name == name) {
                    r.cancel.store(true, Ordering::Relaxed);
                }
            }
            Op::Dismiss(name) => self.st.ol.pulls.retain(|r| r.name != name),
            Op::Delete(name) => self.spawn(move |_| match delete_model(&base, &name) {
                Ok(()) => Msg::Settings(SMsg::Ollama(OMsg::Changed)),
                Err(e) => Msg::Err(e),
            }),
            Op::Unload(name) => self.spawn(move |_| match unload_model(&base, &name) {
                Ok(()) => Msg::Settings(SMsg::Ollama(OMsg::Changed)),
                Err(e) => Msg::Err(e),
            }),
        }
    }

    // ---------------------------------------------------------------- drawing

    pub fn tab_ollama(&mut self, ui: &mut Ui) {
        let p = self.pal;
        // which models are loaded changes behind our back (the agent loads them, ollama times them out).
        if self.now() - self.st.ol.polled > 5.0 {
            self.ollama_load();
        }
        ui.ctx().request_repaint_after_secs(1.0);

        let mut ops: Vec<Op> = Vec::new();
        let mut want: Vec<String> = Vec::new(); // sizes we'd like looked up
        let o = &mut self.st.ol;
        let hw = o.hw.clone().unwrap_or_default();

        section(ui, &p, "This machine");
        if o.hw.is_none() {
            note(ui, &p, "checking hardware…");
        } else {
            for (name, bytes) in &hw.gpus {
                kv(ui, &p, name, &format!("{} vram", gib(*bytes)));
            }
            if hw.gpus.is_empty() {
                note(ui, &p, "no gpu found (looked for nvidia-smi, then amd's sysfs entries). models would run on the cpu.");
            }
            kv(ui, &p, "System RAM", &gib(hw.ram));
            if hw.gpus.len() > 1 {
                kv(ui, &p, "Total", &format!("{} vram", gib(hw.vram())));
            }
        }
        note(ui, &p, "fits vram: the whole model sits on the gpu, fast. vram + ram: part of it spills into system ram, works but slow. it's an estimate (weights + 10% + 1 GB for context), and a big context window needs more.");

        section(ui, &p, "Installed");
        if !o.error.is_empty() {
            ui.add(Label::new(RichText::new(&o.error).font(mono(12.0)).color(p.err)).wrap());
            note(ui, &p, &format!("is ollama running? looked at {}", o.base));
        } else if o.installed.is_empty() {
            note(ui, &p, if o.loaded_once { "nothing installed yet." } else { "loading…" });
        }
        for m in &o.installed {
            let f = fit(m.size, &hw);
            Sides::new().height(30.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    dot(ui, if m.loaded.is_some() { p.accent } else { p.faint });
                    ui.label(RichText::new(&m.name).font(mono(13.0)).color(p.text));
                    ui.label(RichText::new(gb(m.size)).size(12.5).color(p.dim));
                    match m.loaded {
                        Some((total, vram)) => {
                            let pct = (vram * 100).checked_div(total).unwrap_or(0);
                            ui.label(RichText::new(format!("loaded · {pct}% on gpu")).size(12.5).color(p.accent));
                        }
                        None => {
                            ui.label(RichText::new(format!("{} {}", m.params, m.quant)).size(12.0).color(p.faint));
                        }
                    }
                },
                |ui| {
                    if confirm_button(ui, &p, &mut o.confirm, &format!("rm/{}", m.name), "Remove") {
                        ops.push(Op::Delete(m.name.clone()));
                    }
                    if m.loaded.is_some() && solid_button(ui, &p, "Unload", false).on_hover_text("free the memory it's holding").clicked() {
                        ops.push(Op::Unload(m.name.clone()));
                    }
                    if o.hw.is_some() {
                        ui.label(RichText::new(fit_text(f, &hw)).size(12.5).color(fit_color(f, &p))).on_hover_text(format!("wants about {}", gib(need(m.size))));
                    }
                },
            );
        }

        section(ui, &p, "Download");
        for r in &o.pulls {
            let failed = matches!(&r.p.end, Some(Err(_)));
            Sides::new().height(28.0).shrink_left().truncate().show(
                ui,
                |ui| {
                    ui.label(RichText::new(&r.name).font(mono(13.0)).color(p.text));
                    let text = match &r.p.end {
                        Some(Err(e)) => e.clone(),
                        _ if r.p.total > 0 => format!("{} / {} · {}%", gb(r.p.done), gb(r.p.total), r.p.done * 100 / r.p.total),
                        _ => r.p.status.clone(),
                    };
                    ui.label(RichText::new(text).size(12.5).color(if failed { p.err } else { p.dim }));
                },
                |ui| {
                    if failed {
                        if solid_button(ui, &p, "Dismiss", false).clicked() {
                            ops.push(Op::Dismiss(r.name.clone()));
                        }
                        if solid_button(ui, &p, "Retry", false).on_hover_text("picks up where it stopped").clicked() {
                            ops.push(Op::Dismiss(r.name.clone()));
                            ops.push(Op::Pull(r.name.clone()));
                        }
                    } else if solid_button(ui, &p, "Cancel", false).on_hover_text("what's downloaded so far is kept, pulling again resumes").clicked() {
                        ops.push(Op::Cancel(r.name.clone()));
                    }
                },
            );
            let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), Sense::hover());
            ui.painter().rect_filled(bar, 2, p.raised);
            if r.p.total > 0 {
                let mut fill = bar;
                fill.set_width(bar.width() * (r.p.done as f32 / r.p.total as f32).clamp(0.0, 1.0));
                ui.painter().rect_filled(fill, 2, if failed { p.err } else { p.accent });
            }
        }

        field_box(ui, &p, |ui| {
            let hint = if o.library.is_empty() { "model name, like qwen3:14b".to_owned() } else { format!("search {} models, or type name:tag", o.library.len()) };
            ui.add(TextEdit::singleline(&mut o.query).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new(hint).color(p.faint)))
        });
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (f, label) in [(Filter::All, "All"), (Filter::Vram, "Fits vram"), (Filter::Split, "Needs vram + ram")] {
                if solid_button(ui, &p, label, o.filter == f).clicked() {
                    o.filter = f;
                }
            }
        });

        let raw = o.query.trim().to_owned();
        let q = raw.to_lowercase();
        let qname = q.split(':').next().unwrap_or(""); // "qwen3:14b" still finds qwen3
        let (mut shown, mut more) = (0, 0);
        for m in &o.library {
            if !qname.is_empty() && !m.name.contains(qname) && !m.desc.to_lowercase().contains(qname) {
                continue;
            }
            if m.tags.is_empty() && m.caps.iter().any(|c| c == "cloud") {
                continue; // runs on ollama's servers only, nothing to download
            }
            let latest = ["latest".to_owned()];
            let tags: &[String] = if m.tags.is_empty() { &latest } else { &m.tags };
            // (tag, size + whether it's exact, fit)
            #[allow(clippy::type_complexity)]
            let rows: Vec<(&String, Option<(u64, bool)>, Option<Fit>)> = tags
                .iter()
                .map(|t| {
                    let size = match o.sizes.get(&format!("{}:{t}", m.name)) {
                        Some(Some(b)) => Some((*b, true)),
                        _ => est_size(t).map(|b| (b, false)),
                    };
                    (t, size, size.map(|s| fit(s.0, &hw)))
                })
                .collect();
            let passes = |f: Option<Fit>| match o.filter {
                Filter::All => true,
                Filter::Vram => f == Some(Fit::Vram),
                Filter::Split => f == Some(Fit::Split),
            };
            if !rows.iter().any(|r| passes(r.2)) {
                continue;
            }
            if shown == SHOWN {
                more += 1;
                continue;
            }
            shown += 1;

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&m.name).font(FontId::new(14.0, medium())).color(p.text));
                ui.label(RichText::new(m.caps.join(" · ")).size(12.0).color(p.faint));
            });
            if !m.desc.is_empty() {
                ui.add(Label::new(RichText::new(first_line(&m.desc, 130)).size(12.5).color(p.dim)).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (tag, size, f) in &rows {
                    let key = format!("{}:{tag}", m.name);
                    let have = o.installed.iter().any(|i| i.name == key);
                    let busy = o.pulls.iter().any(|r| r.name == key);
                    let resp = size_chip(ui, &p, tag, *size, *f, &hw, have || busy, !passes(*f));
                    if !o.sizes.contains_key(&key) && ui.is_rect_visible(resp.rect) {
                        want.push(key.clone());
                    }
                    let tip = match (have, busy, size) {
                        (true, _, _) => "already installed".to_owned(),
                        (_, true, _) => "downloading".to_owned(),
                        (_, _, Some((b, exact))) => format!("download {key} · {}{} · wants about {} to run", if *exact { "" } else { "roughly " }, gb(*b), gib(need(*b))),
                        _ => format!("download {key}"),
                    };
                    if resp.on_hover_text(tip).clicked() && !have && !busy {
                        ops.push(Op::Pull(key));
                    }
                }
            });
        }
        if more > 0 {
            note(ui, &p, &format!("…and {more} more. type to narrow it down."));
        }
        match o.lib {
            Lib::Loading => note(ui, &p, "loading the model list from ollama.com…"),
            Lib::Done if !o.lib_error.is_empty() => note(ui, &p, &format!("{}. you can still type an exact name and pull it.", o.lib_error)),
            _ => {}
        }
        // anything the list doesn't have (a user's model, hf.co/..., a specific quantization) can be pulled by name.
        let listed = o.library.iter().any(|m| m.name == q);
        if !raw.is_empty() && !listed && (shown == 0 || raw.contains(':') || raw.contains('/')) {
            ui.add_space(4.0);
            if solid_button(ui, &p, &format!("Pull {raw}"), true).clicked() {
                ops.push(Op::Pull(raw.clone()));
            }
        }

        // ask the registry for real sizes of what's on screen, a few at a time. each answer triggers
        // a repaint, which comes back through here and asks for the next few.
        for key in want {
            if self.st.ol.inflight >= LOOKUPS {
                break;
            }
            if self.st.ol.sizes.contains_key(&key) {
                continue;
            }
            self.st.ol.sizes.insert(key.clone(), None);
            self.st.ol.inflight += 1;
            self.spawn(move |_| {
                let size = fetch_size(&key);
                Msg::Settings(SMsg::Ollama(OMsg::Size(key, size)))
            });
        }
        for op in ops {
            self.ollama_exec(op);
        }
    }
}

// ---------------------------------------------------------------- widgets

fn kv(ui: &mut Ui, p: &Pal, left: &str, right: &str) {
    Sides::new().height(24.0).shrink_left().truncate().show(
        ui,
        |ui| ui.label(RichText::new(left).size(14.0).color(p.text)),
        |ui| ui.label(RichText::new(right).font(mono(12.5)).color(p.dim)),
    );
}

// one downloadable size of a model: "27b 17.1 GB", outlined in the color of how well it fits.
// `taken` = installed or downloading: the text goes grey and only the outline keeps the fit color.
// `off` = filtered out: the whole thing goes dim.
#[allow(clippy::too_many_arguments)]
fn size_chip(ui: &mut Ui, p: &Pal, tag: &str, size: Option<(u64, bool)>, f: Option<Fit>, hw: &Hw, taken: bool, off: bool) -> egui::Response {
    let text = match size {
        Some((b, exact)) => format!("{tag}  {}{}", if exact { "" } else { "~" }, gb(b)),
        None => tag.to_owned(),
    };
    let known = hw.ram > 0; // no hardware info yet: no colors
    let edge = match f {
        Some(f) if known => fit_color(f, p),
        _ => p.faint,
    };
    let dim = |c: Color32| if off { c.gamma_multiply(0.4) } else { c };
    let (edge, col) = (dim(edge), dim(if taken { p.faint } else { edge }));
    let g = ui.painter().layout_no_wrap(text, sans(12.5), col);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 18.0, 24.0), Sense::click());
    let pt = ui.painter();
    if resp.hovered() && !taken {
        pt.rect_filled(r, 7, p.raised);
    }
    pt.rect_stroke(r, 7, Stroke::new(1.0, edge.gamma_multiply(0.55)), StrokeKind::Inside);
    pt.galley(pos2(r.left() + 9.0, r.center().y - g.size().y / 2.0), g, col);
    if taken { resp } else { resp.on_hover_cursor(egui::CursorIcon::PointingHand) }
}

// download sizes the way ollama prints them (decimal).
fn gb(bytes: u64) -> String {
    if bytes >= 1_000_000_000 { format!("{:.1} GB", bytes as f64 / 1e9) } else { format!("{:.0} MB", bytes as f64 / 1e6) }
}

// memory the way the hardware counts it (binary), so a 16 GB card reads 16.
fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GIB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_from_tags() {
        assert_eq!(est_size("27b"), Some(18_360_000_000));
        assert_eq!(est_size("e2b"), Some(1_360_000_000));
        assert_eq!(est_size("270m"), Some(183_600_000));
        assert_eq!(est_size("tools"), None);
        assert_eq!(est_size("latest"), None);
    }

    #[test]
    fn fit_classes() {
        let hw = Hw { gpus: vec![("a".into(), 16 * GIB), ("b".into(), 6 * GIB)], ram: 32 * GIB };
        assert!(fit(17 * GIB, &hw) == Fit::Vram); // 17 + 1.7 + 1 = 19.7 of 22
        assert!(fit(30 * GIB, &hw) == Fit::Split); // 34 of 22 + 28
        assert!(fit(60 * GIB, &hw) == Fit::No);
        let cpu = Hw { gpus: vec![], ram: 32 * GIB };
        assert!(fit(4 * GIB, &cpu) == Fit::Split);
    }

    #[test]
    fn library_page() {
        let html = r#"<ul><li  class="flex"><a href="/library/llama3.1" class="group">
            <p class="max-w-lg break-words">Meta&#39;s model, 8B &amp; 70B.</p>
            <span  class="inline-flex px-2 py-0.5 text-indigo-600">tools</span>
            <span  class="inline-flex px-2 py-0.5 text-blue-600">8b</span>
            <span  class="inline-flex px-2 py-0.5 text-blue-600">70b</span></a></li>
            <li class="flex"><a href="/library/nomic-embed-text"><span class="px-2 py-0.5">embedding</span></a></li></ul>"#;
        let list = parse_library(html);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "llama3.1");
        assert_eq!(list[0].desc, "Meta's model, 8B & 70B.");
        assert_eq!(list[0].caps, ["tools"]);
        assert_eq!(list[0].tags, ["8b", "70b"]);
        assert!(list[1].tags.is_empty());
    }

    // hits ollama.com and its registry: `cargo test -- --ignored` to check the scraper still matches the site.
    #[test]
    #[ignore]
    fn library_live() {
        let list = fetch_library().unwrap();
        assert!(list.len() > 50, "only {} models", list.len());
        let with_tags = list.iter().filter(|m| !m.tags.is_empty()).count();
        assert!(with_tags > list.len() / 2, "{with_tags} of {} have size tags", list.len());
        let m = list.iter().find(|m| !m.tags.is_empty()).unwrap();
        let key = format!("{}:{}", m.name, m.tags[0]);
        let size = fetch_size(&key).unwrap_or_else(|| panic!("no size for {key}"));
        eprintln!("{} models, {with_tags} with sizes. {key} = {} (estimate {})", list.len(), gb(size), gb(est_size(&m.tags[0]).unwrap()));
        for m in list.iter().take(6) {
            eprintln!("  {} [{}] {:?}", m.name, m.caps.join(","), m.tags);
        }
    }

    // downloads a ~46 MB model into the local ollama and deletes it again. needs ollama running.
    #[test]
    #[ignore]
    fn pull_and_delete_live() {
        let (base, name) = ("http://127.0.0.1:11434", "all-minilm:latest");
        let before = fetch_installed(base).unwrap();
        assert!(!before.iter().any(|m| m.name == name), "{name} is already installed, not touching it");

        let (tx, rx) = std::sync::mpsc::channel();
        pull_thread(base.to_owned(), name.to_owned(), Arc::new(AtomicBool::new(false)), tx, egui::Context::default());
        let mut ticks = 0;
        let mut last = Pull::default();
        while let Ok(Msg::Settings(SMsg::Ollama(OMsg::Pull(_, p)))) = rx.try_recv() {
            ticks += 1;
            last = p;
        }
        eprintln!("{ticks} progress messages, last: {} {}/{} {:?}", last.status, last.done, last.total, last.end);
        // clean up first, assert after: a failed assert must not leave the model behind.
        let installed = fetch_installed(base).unwrap().iter().any(|m| m.name == name);
        let deleted = delete_model(base, name);
        let gone = !fetch_installed(base).unwrap().iter().any(|m| m.name == name);

        assert!(matches!(last.end, Some(Ok(()))), "pull ended with {:?}", last.end);
        assert!(last.total > 0 && last.done == last.total);
        assert!(installed, "pull said success but the model isn't listed");
        assert!(deleted.is_ok() && gone, "delete: {deleted:?}");
    }
}
