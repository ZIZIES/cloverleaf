// all the drawing. egui is immediate mode: every frame we walk the state in App and emit
// widgets for it, there's no retained widget tree to keep in sync.

use crate::app::{Action, App, FieldState, Palette, PaletteMode};
use crate::logo;
use crate::model::*;
use crate::theme::{self, Pal, medium, mono, sans, semibold};
use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align, Align2, CentralPanel, Color32, CornerRadius, FontId, Frame, Id, Key, KeyboardShortcut, Label, Layout, Margin, Modifiers, Panel, Pos2, Rect,
    Response, RichText, ScrollArea, Sense, Stroke, StrokeKind, TextEdit, Ui, UiBuilder, pos2, vec2,
};
use egui_commonmark::CommonMarkViewer;

const COLUMN: f32 = 780.0; // max width of the transcript / prompt column
const RAIL: f32 = 22.0; // left gutter the tool timeline sits in

impl App {
    pub fn draw(&mut self, ui: &mut Ui) {
        self.pump();
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);

        let p = self.pal;
        if let Some(e) = self.fatal.clone() {
            CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| splash(ui, &p, "couldn't start opencode", &e, false));
            return;
        }
        if !self.ready {
            CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| splash(ui, &p, "cloverleaf", "starting opencode…", true));
            ctx.request_repaint_after_secs(0.03);
            return;
        }

        if self.sidebar {
            Panel::left("sidebar")
                .exact_size(268.0)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::new().fill(p.side).inner_margin(Margin::symmetric(10, 12)))
                .show(ui, |ui| self.sidebar_ui(ui));
        }
        CentralPanel::default().frame(Frame::new().fill(p.bg)).show(ui, |ui| self.main_ui(ui));

        self.palette_ui(&ctx);
        self.settings_ui(&ctx);
        self.toast_ui(&ctx);

        // keep animating (spinner, pulsing dots) while something is running.
        if !self.busy.is_empty() {
            ctx.request_repaint_after_secs(0.03);
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (comma, k, n, b, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::Comma),
                i.consume_key(Modifiers::COMMAND, Key::K),
                i.consume_key(Modifiers::COMMAND, Key::N),
                i.consume_key(Modifiers::COMMAND, Key::B),
                i.key_pressed(Key::Escape),
            )
        });
        if k {
            self.palette = match self.palette {
                Some(_) => None,
                None => Some(Palette { mode: PaletteMode::All, query: String::new(), sel: 0 }),
            };
        }
        if comma {
            self.settings_open = !self.settings_open;
        }
        if n {
            self.run(Action::NewSession);
        }
        if b {
            self.run(Action::ToggleSidebar);
        }
        if esc && self.palette.is_none() && !self.settings_open && self.is_busy() {
            self.run(Action::Interrupt);
        }
    }

    // ---------------------------------------------------------------- sidebar

    fn sidebar_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let mut acts: Vec<Action> = Vec::new();

        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            logo::paint(ui.painter(), r, p.accent, p.cream, 0.0);
            ui.add_space(2.0);
            ui.label(RichText::new("cloverleaf").font(FontId::new(16.0, semibold())).color(p.text));
        });
        ui.add_space(8.0);

        if nav_row(ui, &p, "New session", "ctrl n", false, |pt, r, c| plus_icon(pt, r, c)).clicked() {
            acts.push(Action::NewSession);
        }
        if nav_row(ui, &p, "Search & commands", "ctrl k", false, |pt, r, c| search_icon(pt, r, c)).clicked() {
            acts.push(Action::Palette(PaletteMode::All));
        }
        ui.add_space(6.0);

        Frame::new().fill(p.inset).corner_radius(8).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
            ui.add(
                TextEdit::singleline(&mut self.search)
                    .frame(Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(sans(13.0))
                    .hint_text(RichText::new("filter sessions").color(p.faint)),
            );
        });
        ui.add_space(4.0);

        // group by project directory, most recently touched project first.
        let q = self.search.to_lowercase();
        let mut groups: Vec<(&str, Vec<&Session>)> = Vec::new();
        for s in &self.sessions {
            if !q.is_empty() && !s.title.to_lowercase().contains(&q) && !s.dir.to_lowercase().contains(&q) {
                continue;
            }
            match groups.iter_mut().find(|g| g.0 == s.dir) {
                Some(g) => g.1.push(s),
                None => groups.push((&s.dir, vec![s])),
            }
        }

        let footer = 40.0;
        let now_ms = now_ms();
        ScrollArea::vertical().id_salt("sessions").auto_shrink(false).max_height(ui.available_height() - footer).show(ui, |ui| {
            if groups.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new(if q.is_empty() { "no sessions yet" } else { "nothing matches" }).color(p.faint).size(13.0));
            }
            for (dir, list) in &groups {
                ui.add_space(10.0);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 20.0), Sense::click());
                let resp = resp.on_hover_text(format!("{dir}\nclick: new session here"));
                let c = if resp.hovered() { p.dim } else { p.faint };
                ui.painter().with_clip_rect(r).text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, basename(dir), FontId::new(11.5, medium()), c);
                if resp.clicked() {
                    acts.push(Action::SetDir((*dir).to_owned()));
                }

                for s in list {
                    let selected = self.active.as_deref() == Some(s.id.as_str());
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
                    if selected || resp.hovered() {
                        ui.painter().rect_filled(r, 8, if selected { p.raised } else { p.line_soft });
                    }
                    let waiting = self.perms.iter().any(|x| x.session == s.id) || self.forms.iter().any(|x| x.session == s.id);
                    let right = if waiting {
                        ui.painter().circle_filled(pos2(r.right() - 14.0, r.center().y), 4.0, p.accent);
                        ui.painter().circle_stroke(pos2(r.right() - 14.0, r.center().y), 6.5, Stroke::new(1.0, p.accent.gamma_multiply(0.5)));
                        r.right() - 26.0
                    } else if self.busy.contains(&s.id) {
                        let a = 0.45 + 0.55 * ((ui.input(|i| i.time) * 4.0).sin() as f32 * 0.5 + 0.5);
                        ui.painter().circle_filled(pos2(r.right() - 14.0, r.center().y), 3.5, p.accent.gamma_multiply(a));
                        r.right() - 26.0
                    } else {
                        let g = ui.painter().layout_no_wrap(ago(now_ms - s.updated), sans(11.5), p.faint);
                        let x = r.right() - 8.0 - g.size().x;
                        ui.painter().galley(pos2(x, r.center().y - g.size().y / 2.0), g, p.faint);
                        x - 8.0
                    };
                    let clip = Rect::from_min_max(r.min, pos2(right, r.bottom()));
                    let c = if selected { p.text } else { p.dim };
                    ui.painter().with_clip_rect(clip).text(pos2(r.left() + 8.0, r.center().y), Align2::LEFT_CENTER, s.label(), sans(13.5), c);
                    if resp.clicked() {
                        acts.push(Action::Open(s.id.clone()));
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Delete session").clicked() {
                            acts.push(Action::Delete(s.id.clone()));
                            ui.close();
                        }
                    });
                }
            }
        });

        // footer: settings. the hint doubles as the connection light.
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            let hint = if self.sse { "ctrl ," } else { "reconnecting…" };
            if nav_row(ui, &p, "Settings", hint, self.settings_open, |pt, r, c| gear_icon(pt, r, c)).clicked() {
                acts.push(Action::Settings);
            }
        });

        for a in acts {
            self.run(a);
        }
    }

    // ---------------------------------------------------------------- main column

    fn main_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;

        Panel::top("topbar").exact_size(44.0).show_separator_line(false).frame(Frame::new().inner_margin(Margin::symmetric(10, 0))).show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if icon_button(ui, &p, 30.0, |pt, r, c| sidebar_icon(pt, r, c)).on_hover_text("toggle sidebar (ctrl+b)").clicked() {
                    self.run(Action::ToggleSidebar);
                }
                if let Some(s) = self.session() {
                    let r = ui.available_rect_before_wrap();
                    let pt = ui.painter().with_clip_rect(r);
                    let cost = if s.cost > 0.0 { format!("${:.2}", s.cost) } else { String::new() };
                    let cg = pt.layout_no_wrap(cost, sans(12.5), p.faint);
                    let cx = r.right() - 6.0 - cg.size().x;
                    pt.galley(pos2(cx, r.center().y - cg.size().y / 2.0), cg, p.faint);
                    let tg = pt.layout_no_wrap(s.label().to_owned(), FontId::new(14.0, medium()), p.text);
                    let x = r.left() + 6.0;
                    let dx = x + tg.size().x + 10.0;
                    let clip = Rect::from_min_max(r.min, pos2(cx - 12.0, r.bottom()));
                    pt.with_clip_rect(clip).galley(pos2(x, r.center().y - tg.size().y / 2.0), tg, p.text);
                    pt.with_clip_rect(clip).text(pos2(dx, r.center().y + 0.5), Align2::LEFT_CENTER, tilde(&s.dir), sans(12.5), p.faint);
                }
            });
        });

        if self.active.is_none() {
            CentralPanel::default().frame(Frame::NONE).show(ui, |ui| self.home_ui(ui));
            return;
        }

        Panel::bottom("composer").show_separator_line(false).frame(Frame::new().inner_margin(Margin { left: 16, right: 16, top: 4, bottom: 14 })).show(
            ui,
            |ui| {
                column(ui, COLUMN, |ui| {
                    self.suggestions_ui(ui);
                    self.requests_ui(ui);
                    self.prompt_card(ui);
                });
            },
        );
        CentralPanel::default().frame(Frame::NONE).show(ui, |ui| self.transcript_ui(ui));
    }

    // no session open: logo, greeting, project picker, prompt.
    fn home_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let top = ((ui.available_height() - 360.0) * 0.42).max(12.0);
        ui.add_space(top);
        ui.vertical_centered(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(60.0, 60.0), Sense::hover());
            logo::paint(ui.painter(), r, p.accent, p.cream, 0.0);
            ui.add_space(14.0);
            ui.label(RichText::new("What are we building?").font(FontId::new(26.0, semibold())).color(p.text));
            ui.add_space(6.0);
            let dir = tilde(&self.new_dir);
            if chip(ui, &p, &format!("in {dir}"), true).on_hover_text("pick the project folder for the next session").clicked() {
                self.run(Action::Palette(PaletteMode::Dirs));
            }
        });
        ui.add_space(18.0);
        ui.scope_builder(UiBuilder::new().max_rect(ui.available_rect_before_wrap().shrink2(vec2(16.0, 0.0))), |ui| {
            column(ui, 700.0, |ui| {
                self.suggestions_ui(ui);
                self.prompt_card(ui);
            });
        });
    }

    fn transcript_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let stick = std::mem::take(&mut self.stick);
        let busy = self.is_busy();
        let sid = self.active.clone().unwrap_or_default();

        ScrollArea::vertical().id_salt(("transcript", &sid)).auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            ui.scope_builder(UiBuilder::new().max_rect(ui.available_rect_before_wrap().shrink2(vec2(16.0, 0.0))), |ui| {
                column(ui, COLUMN, |ui| {
                    ui.add_space(10.0);
                    if self.loading && self.items.is_empty() {
                        ui.add_space(30.0);
                        ui.vertical_centered(|ui| ui.label(RichText::new("loading…").color(p.faint)));
                    }

                    // take the list out of self so the row renderers can borrow self mutably.
                    let items = std::mem::take(&mut self.items);
                    let n = items.len();
                    let width = ui.available_width();
                    self.rail = None;
                    for (i, item) in items.iter().enumerate() {
                        // rows that are off screen and not changing just reserve their old height.
                        let live = busy && i + 4 >= n;
                        let top = ui.cursor().min;
                        let is_reply = matches!(item, Item::Assistant { .. });
                        let joined = is_reply && i > 0 && matches!(items[i - 1], Item::Assistant { .. });
                        if !is_reply {
                            self.rail = None;
                        }
                        if !live
                            && let Some(&(w, h)) = self.heights.get(item.id())
                            && w == width
                            && !ui.is_rect_visible(Rect::from_min_size(top, vec2(width, h)))
                        {
                            ui.allocate_space(vec2(width, h));
                            self.rail = None;
                            continue;
                        }
                        if i > 0 {
                            ui.add_space(if joined { 0.0 } else { 14.0 });
                        }
                        ui.push_id(item.id(), |ui| self.item_ui(ui, item, live));
                        self.heights.insert(item.id().to_owned(), (width, ui.cursor().min.y - top.y));
                    }
                    self.items = items;

                    if busy {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                            let t = ui.input(|i| i.time) as f32;
                            logo::paint_clover(ui.painter(), r.center(), 17.0, p.accent, t * 1.6, false);
                            ui.label(RichText::new("working…  esc to stop").size(13.0).color(p.faint));
                        });
                    }
                    ui.add_space(20.0);
                });
            });
            if stick {
                ui.scroll_to_cursor(Some(Align::BOTTOM));
            }
        });
    }

    // ---------------------------------------------------------------- transcript rows

    fn item_ui(&mut self, ui: &mut Ui, item: &Item, live: bool) {
        let p = self.pal;
        match item {
            Item::User { text, .. } => {
                ui.add_space(6.0);
                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                    Frame::new().fill(p.raised).corner_radius(14).inner_margin(Margin::symmetric(14, 9)).show(ui, |ui| {
                        ui.set_max_width((ui.available_width() * 0.82).max(120.0));
                        ui.with_layout(Layout::top_down(Align::Min), |ui| {
                            ui.add(Label::new(RichText::new(text).color(p.text)).wrap());
                        });
                    });
                });
            }
            Item::Assistant { parts, error, done, .. } => {
                // tool calls and thinking hang off a rail on the left; `prev` is the last dot drawn,
                // so consecutive rows get joined by a line and prose breaks the chain.
                let mut prev: Option<Pos2> = self.rail;
                let last = parts.len().saturating_sub(1);
                for (i, part) in parts.iter().enumerate() {
                    match part {
                        Part::Text(t) => {
                            if t.trim().is_empty() {
                                continue;
                            }
                            if prev.take().is_some() {
                                ui.add_space(6.0);
                            }
                            ui.push_id(i, |ui| {
                                CommonMarkViewer::new().syntax_theme_dark("cloverleaf").show(ui, &mut self.md, t);
                            });
                            ui.add_space(4.0);
                        }
                        Part::Reasoning(t) => {
                            if t.trim().is_empty() {
                                continue;
                            }
                            let thinking = live && !*done && i == last;
                            if !thinking && !self.show_thinking {
                                continue;
                            }
                            let key = format!("{}/r{}", item.id(), i);
                            let open = self.toggled.contains(&key);
                            let label = if thinking { "thinking" } else { "thought" };
                            let hint = first_line(t, 90);
                            let resp = rail_row(ui, &p, &mut prev, if thinking { Dot::Pulse } else { Dot::Idle }, label, false, &hint, "", open);
                            if resp.clicked() {
                                flip(&mut self.toggled, &key);
                            }
                            if open {
                                indented(ui, |ui| {
                                    ui.add(Label::new(RichText::new(t.trim()).size(13.5).color(p.dim).italics()).wrap());
                                });
                            }
                        }
                        Part::Tool(tool) => self.tool_ui(ui, tool, &mut prev),
                    }
                }
                if !error.is_empty() {
                    failure(ui, &p, error);
                    prev = None;
                }
                self.rail = prev;
            }
            Item::Shell { command, output, running, exit, id } => {
                let mut prev = None;
                let dot = if *running {
                    Dot::Pulse
                } else if exit.unwrap_or(0) != 0 {
                    Dot::Bad
                } else {
                    Dot::Good
                };
                let open = !self.toggled.contains(id); // open by default
                let status = match exit {
                    Some(c) if *c != 0 => format!("exit {c}"),
                    _ => String::new(),
                };
                if rail_row(ui, &p, &mut prev, dot, "!", true, command, &status, open).clicked() {
                    flip(&mut self.toggled, id);
                }
                if open && !output.is_empty() {
                    indented(ui, |ui| code_box(ui, &p, id, |ui| mono_text(ui, output, p.dim, 400)));
                }
            }
            Item::Notice { text, .. } => {
                ui.vertical_centered(|ui| ui.label(RichText::new(text).size(12.5).color(p.faint)));
            }
            Item::Failure { text, .. } => failure(ui, &p, text),
        }
    }

    fn tool_ui(&mut self, ui: &mut Ui, tool: &Tool, prev: &mut Option<Pos2>) {
        let p = self.pal;
        let dir = self.dir();
        let inp = &tool.input;
        let path = |k: &str| rel(inp[k].as_str().unwrap_or(""), &dir);

        // one-line summary of what the call is doing.
        let mut status = String::new();
        let summary = match tool.name.as_str() {
            _ if tool.status == ToolStatus::Streaming => first_line(&tool.raw_input, 120),
            "shell" => first_line(inp["command"].as_str().unwrap_or(""), 200),
            "read" | "write" => path("path"),
            "edit" => {
                let (a, d) = patch_stats(&tool_patch(tool));
                if a + d > 0 {
                    status = format!("+{a} −{d}");
                }
                path("path")
            }
            "glob" | "grep" => inp["pattern"].as_str().unwrap_or("").to_owned(),
            "websearch" => inp["query"].as_str().unwrap_or("").to_owned(),
            "webfetch" => inp["url"].as_str().unwrap_or("").to_owned(),
            "execute" => first_line(inp["code"].as_str().unwrap_or(""), 120),
            "question" => first_line(inp["questions"][0]["question"].as_str().unwrap_or(""), 120),
            _ => {
                let s = inp.to_string();
                if s == "null" || s == "{}" { String::new() } else { first_line(&s, 120) }
            }
        };

        let dot = match tool.status {
            ToolStatus::Streaming | ToolStatus::Running => Dot::Pulse,
            ToolStatus::Done => Dot::Good,
            ToolStatus::Error => Dot::Bad,
        };
        match tool.status {
            ToolStatus::Error => status = "failed".to_owned(),
            ToolStatus::Done if status.is_empty() && tool.completed > tool.created && tool.created > 0.0 => {
                let secs = (tool.completed - tool.created) / 1000.0;
                if secs >= 1.0 {
                    status = format!("{secs:.1}s");
                }
            }
            _ => {}
        }

        // edits show their diff by default, everything else starts folded.
        let default_open = (tool.name == "edit" && self.open_diffs) || tool.status == ToolStatus::Error;
        let open = default_open != self.toggled.contains(&tool.id);
        if rail_row(ui, &p, prev, dot, &tool.name, true, &summary, &status, open).clicked() {
            flip(&mut self.toggled, &tool.id);
        }
        if !open {
            return;
        }

        indented(ui, |ui| {
            code_box(ui, &p, &tool.id, |ui| {
                match tool.name.as_str() {
                    "shell" => {
                        mono_text(ui, &format!("$ {}", inp["command"].as_str().unwrap_or("")), p.text, 40);
                    }
                    "edit" => {
                        let patch = tool_patch(tool);
                        if patch.is_empty() {
                            let old = inp["oldString"].as_str().unwrap_or("");
                            let new = inp["newString"].as_str().unwrap_or("");
                            let fake: String = old.lines().map(|l| format!("-{l}\n")).chain(new.lines().map(|l| format!("+{l}\n"))).collect();
                            ui.add(Label::new(diff_job(&fake, &p)).wrap());
                        } else {
                            ui.add(Label::new(diff_job(&patch, &p)).wrap());
                        }
                    }
                    "write" => mono_text(ui, inp["content"].as_str().unwrap_or(""), p.dim, 80),
                    "read" | "glob" | "grep" | "websearch" | "webfetch" => {}
                    "execute" => mono_text(ui, inp["code"].as_str().unwrap_or(""), p.text, 80),
                    _ if tool.status == ToolStatus::Streaming => mono_text(ui, &tool.raw_input, p.dim, 80),
                    _ => {
                        if !inp.is_null() {
                            mono_text(ui, &serde_json::to_string_pretty(inp).unwrap_or_default(), p.dim, 80);
                        }
                    }
                }
                if !tool.error.is_empty() {
                    mono_text(ui, &tool.error, p.err, 80);
                }
                let out = tool.output.trim_end();
                if !out.is_empty() && tool.name != "edit" {
                    mono_text(ui, out, p.dim, 300);
                }
            });
        });
    }

    // ---------------------------------------------------------------- composer

    // "/..." typed with no space yet: show matching commands above the prompt.
    fn suggestions_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let Some(q) = self.prompt.strip_prefix('/') else { return };
        if q.contains(char::is_whitespace) {
            return;
        }
        let builtin = [
            ("new", "start a new session"),
            ("models", "switch model"),
            ("agents", "switch agent"),
            ("sessions", "jump to a session"),
            ("compact", "summarize the context so far"),
        ];
        let mut rows: Vec<(String, String)> = builtin.iter().map(|(n, d)| ((*n).to_owned(), (*d).to_owned())).collect();
        rows.extend(self.commands.iter().map(|c| (c.name.clone(), c.description.clone())));
        rows.retain(|(n, _)| n.starts_with(q));
        if rows.is_empty() {
            return;
        }
        let mut fill = None;
        Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.line)).corner_radius(12).inner_margin(6).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (name, desc) in rows.iter().take(8) {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 7, p.raised);
                }
                let g = ui.painter().layout_no_wrap(format!("/{name}"), mono(13.0), p.text);
                let x = r.left() + 8.0 + g.size().x + 12.0;
                ui.painter().galley(pos2(r.left() + 8.0, r.center().y - g.size().y / 2.0), g, p.text);
                ui.painter().with_clip_rect(r).text(pos2(x, r.center().y), Align2::LEFT_CENTER, desc, sans(13.0), p.faint);
                if resp.clicked() {
                    fill = Some(format!("/{name} "));
                }
            }
        });
        ui.add_space(6.0);
        if let Some(f) = fill {
            self.run(Action::FillPrompt(f));
        }
    }

    // permission prompts + questions the agent is blocked on.
    fn requests_ui(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let sid = self.active.clone().unwrap_or_default();
        let card = Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.accent.gamma_multiply(0.7))).corner_radius(14).inner_margin(Margin::symmetric(14, 12));

        for perm in self.perms.clone().iter().filter(|x| x.session == sid) {
            card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Permission needed").font(FontId::new(14.0, semibold())).color(p.text));
                    ui.label(RichText::new(&perm.action).font(mono(13.0)).color(p.accent));
                });
                if !perm.message.is_empty() {
                    ui.label(RichText::new(&perm.message).size(13.5).color(p.dim));
                }
                for r in perm.resources.iter().take(6) {
                    ui.add(Label::new(RichText::new(r).font(mono(12.5)).color(p.dim)).wrap());
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if solid_button(ui, &p, "Allow once", true).clicked() {
                        self.reply_permission(perm, "once");
                    }
                    if solid_button(ui, &p, "Always allow", false).clicked() {
                        self.reply_permission(perm, "always");
                    }
                    if solid_button(ui, &p, "Reject", false).clicked() {
                        self.reply_permission(perm, "reject");
                    }
                });
            });
            ui.add_space(8.0);
        }

        for form in self.forms.clone().iter().filter(|x| x.session == sid) {
            card.show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !form.title.is_empty() {
                    ui.label(RichText::new(&form.title).font(FontId::new(14.0, semibold())).color(p.text));
                }
                ScrollArea::vertical().id_salt(&form.id).max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    for f in &form.fields {
                        let st = self.form_state.entry(format!("{}/{}", form.id, f.key)).or_default();
                        field_ui(ui, &p, f, st);
                        ui.add_space(6.0);
                    }
                });
                ui.horizontal(|ui| {
                    if solid_button(ui, &p, "Submit", true).clicked() {
                        self.reply_form(form);
                    }
                    if solid_button(ui, &p, "Dismiss", false).clicked() {
                        self.cancel_form(form);
                    }
                });
            });
            ui.add_space(8.0);
        }
    }

    fn prompt_card(&mut self, ui: &mut Ui) {
        let p = self.pal;
        let busy = self.is_busy();
        let id = Id::unique("prompt");
        let focused = ui.memory(|m| m.has_focus(id));
        let border = if focused { p.accent.gamma_multiply(0.55) } else { p.line };
        let mut acts: Vec<Action> = Vec::new();
        let mut send = false;

        Frame::new().fill(p.panel).stroke(Stroke::new(1.0, border)).corner_radius(18).inner_margin(Margin { left: 16, right: 10, top: 12, bottom: 10 }).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let resp = ScrollArea::vertical()
                .id_salt("prompt_scroll")
                .max_height(240.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.add(
                        TextEdit::multiline(&mut self.prompt)
                            .id(id)
                            .frame(Frame::NONE)
                            .desired_width(f32::INFINITY)
                            .desired_rows(2)
                            .return_key(KeyboardShortcut::new(Modifiers::SHIFT, Key::Enter))
                            .hint_text(RichText::new("ask for anything   / commands   ! shell").color(p.faint)),
                    )
                })
                .inner;
            if std::mem::take(&mut self.focus_prompt) {
                resp.request_focus();
            }
            if resp.has_focus() && ui.input(|i| i.key_pressed(Key::Enter) && !i.modifiers.shift) {
                send = true;
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if chip(ui, &p, &self.current_agent(), false).on_hover_text("agent").clicked() {
                    acts.push(Action::Palette(PaletteMode::Agents));
                }
                let model = self.current_model();
                let name = model.as_ref().map(|m| self.model_info(m).map(|i| i.name.clone()).unwrap_or_else(|| m.id.clone())).unwrap_or_else(|| "model".to_owned());
                if chip(ui, &p, &name, false).on_hover_text("model").clicked() {
                    acts.push(Action::Palette(PaletteMode::Models));
                }
                let info = model.as_ref().and_then(|m| self.model_info(m));
                if info.is_some_and(|i| !i.variants.is_empty()) {
                    let v = model.as_ref().and_then(|m| m.variant.clone()).unwrap_or_else(|| "default".to_owned());
                    if chip(ui, &p, &v, false).on_hover_text("reasoning effort").clicked() {
                        acts.push(Action::Palette(PaletteMode::Variants));
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let empty = self.prompt.trim().is_empty();
                    let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
                    if busy && empty {
                        ui.painter().circle_filled(r.center(), 16.0, if resp.hovered() { p.raised_hover } else { p.raised });
                        ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(10.0, 10.0)), 2, p.text);
                        if resp.on_hover_text("stop (esc)").clicked() {
                            acts.push(Action::Interrupt);
                        }
                    } else {
                        let (bg, fg) = match (empty, resp.hovered()) {
                            (true, _) => (p.raised, p.faint),
                            (false, true) => (p.accent_hover, p.on_accent),
                            (false, false) => (p.accent, p.on_accent),
                        };
                        ui.painter().circle_filled(r.center(), 16.0, bg);
                        arrow_up(ui.painter(), r, fg);
                        if resp.clicked() && !empty {
                            send = true;
                        }
                    }

                    // context window meter
                    let used = self.context_used();
                    if used > 0 {
                        ui.add_space(6.0);
                        let limit = info.map(|i| i.context).unwrap_or(0);
                        let text = if limit > 0 { format!("{} · {}%", kilo(used), (used * 100 / limit).min(100)) } else { kilo(used) };
                        ui.label(RichText::new(text).size(12.0).color(p.faint)).on_hover_text("context window used");
                    }
                    if let Some((rate, live)) = self.tok_rate() {
                        ui.add_space(6.0);
                        let text = format!("{}{rate:.0} tok/s", if live { "~" } else { "" });
                        ui.label(RichText::new(text).size(12.0).color(p.faint)).on_hover_text("generation speed of the last step: output tokens over the time it spent streaming. ~ means a live estimate");
                    }
                });
            });
        });

        if send {
            self.send();
            self.focus_prompt = true;
        }
        for a in acts {
            self.run(a);
        }
    }

    // ---------------------------------------------------------------- palette

    fn palette_entries(&self, mode: PaletteMode) -> Vec<(String, String, Action)> {
        let mut out: Vec<(String, String, Action)> = Vec::new();
        let mut add = |label: &str, hint: &str, a: Action| out.push((label.to_owned(), hint.to_owned(), a));
        let models = |out: &mut Vec<(String, String, Action)>| {
            for m in &self.models {
                out.push((m.name.clone(), format!("{}/{}", m.provider, m.id), Action::SetModel(ModelRef { id: m.id.clone(), provider: m.provider.clone(), variant: None })));
            }
        };
        let sessions = |out: &mut Vec<(String, String, Action)>| {
            for s in &self.sessions {
                out.push((s.label().to_owned(), basename(&s.dir).to_owned(), Action::Open(s.id.clone())));
            }
        };
        match mode {
            PaletteMode::All => {
                add("New session", "ctrl n", Action::NewSession);
                add("Switch model…", "", Action::Palette(PaletteMode::Models));
                add("Switch agent…", "", Action::Palette(PaletteMode::Agents));
                add("Reasoning effort…", "", Action::Palette(PaletteMode::Variants));
                add("Open project folder…", "", Action::Palette(PaletteMode::Dirs));
                add("Toggle sidebar", "ctrl b", Action::ToggleSidebar);
                add("Settings", "ctrl ,", Action::Settings);
                if let Some(sid) = &self.active {
                    add("Compact session", "", Action::Compact);
                    add("Stop", "esc", Action::Interrupt);
                    add("Delete this session", "", Action::Delete(sid.clone()));
                }
                for (i, a) in theme::ACCENTS.iter().enumerate() {
                    add(&format!("Accent: {}", a.0), "", Action::Accent(i));
                }
                for c in &self.commands {
                    add(&format!("/{}", c.name), &c.description, Action::FillPrompt(format!("/{} ", c.name)));
                }
                sessions(&mut out);
            }
            PaletteMode::Models => models(&mut out),
            PaletteMode::Variants => {
                if let Some(cur) = self.current_model() {
                    let variants = self.model_info(&cur).map(|i| i.variants.clone()).unwrap_or_default();
                    out.push(("default".to_owned(), String::new(), Action::SetModel(ModelRef { variant: None, ..cur.clone() })));
                    for v in variants {
                        out.push((v.clone(), String::new(), Action::SetModel(ModelRef { variant: Some(v), ..cur.clone() })));
                    }
                }
            }
            PaletteMode::Agents => {
                for a in &self.agents {
                    out.push((a.id.clone(), first_line(&a.description, 80), Action::SetAgent(a.id.clone())));
                }
            }
            PaletteMode::Sessions => sessions(&mut out),
            PaletteMode::Dirs => {
                let mut seen: Vec<&str> = vec![&self.new_dir];
                for s in &self.sessions {
                    if !seen.contains(&s.dir.as_str()) {
                        seen.push(&s.dir);
                    }
                }
                for d in seen {
                    out.push((tilde(d), String::new(), Action::SetDir(d.to_owned())));
                }
            }
        }
        out
    }

    fn palette_ui(&mut self, ctx: &egui::Context) {
        let Some(mut pl) = self.palette.take() else { return };
        let p = self.pal;

        let mut entries = self.palette_entries(pl.mode);
        let q = pl.query.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        entries.retain(|(label, hint, _)| {
            let hay = format!("{label} {hint}").to_lowercase();
            words.iter().all(|w| hay.contains(w))
        });
        // typing a real path in folder mode offers it directly.
        if pl.mode == PaletteMode::Dirs && !pl.query.trim().is_empty() {
            let path = expand(pl.query.trim());
            if std::path::Path::new(&path).is_dir() {
                let path = std::fs::canonicalize(&path).map(|x| x.to_string_lossy().into_owned()).unwrap_or(path);
                entries.insert(0, (format!("Use {}", tilde(&path)), String::new(), Action::SetDir(path)));
            }
        }
        if pl.sel >= entries.len() {
            pl.sel = entries.len().saturating_sub(1);
        }

        let hint = match pl.mode {
            PaletteMode::All => "search sessions, models, commands…",
            PaletteMode::Models => "switch model",
            PaletteMode::Variants => "reasoning effort",
            PaletteMode::Agents => "switch agent",
            PaletteMode::Sessions => "jump to session",
            PaletteMode::Dirs => "project folder (type a path or pick one)",
        };

        let mut chosen: Option<Action> = None;
        let frame = Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.line)).corner_radius(14).inner_margin(10);
        let res = egui::Modal::new(Id::unique("palette")).frame(frame).backdrop_color(Color32::from_black_alpha(130)).show(ctx, |ui| {
            // the agent list is a handful of entries: a small box that fits them, not the full-size palette.
            let small = pl.mode == PaletteMode::Agents;
            ui.set_width((if small { 340.0_f32 } else { 560.0 }).min(ctx.content_rect().width() - 40.0));
            let (up, down, enter) = ui.input_mut(|i| {
                (i.consume_key(Modifiers::NONE, Key::ArrowUp), i.consume_key(Modifiers::NONE, Key::ArrowDown), i.consume_key(Modifiers::NONE, Key::Enter))
            });
            if down && pl.sel + 1 < entries.len() {
                pl.sel += 1;
            }
            if up {
                pl.sel = pl.sel.saturating_sub(1);
            }

            let te = ui.add(
                TextEdit::singleline(&mut pl.query)
                    .frame(Frame::new().inner_margin(Margin::symmetric(6, 6)))
                    .desired_width(f32::INFINITY)
                    .font(sans(16.0))
                    .hint_text(RichText::new(hint).color(p.faint)),
            );
            te.request_focus();
            if te.changed() {
                pl.sel = 0;
            }
            ui.add_space(2.0);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0, p.line_soft);
            ui.add_space(4.0);

            ScrollArea::vertical().max_height(372.0).auto_shrink([false, small]).show(ui, |ui| {
                if !small {
                    ui.set_min_height(372.0); // fixed height, so the box doesn't jump around while you type
                }
                ui.spacing_mut().item_spacing.y = 2.0;
                if entries.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("  nothing matches").color(p.faint));
                }
                for (i, (label, hint, action)) in entries.iter().enumerate() {
                    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                    let sel = i == pl.sel;
                    if sel || resp.hovered() {
                        ui.painter().rect_filled(r, 8, if sel { p.raised } else { p.line_soft });
                    }
                    // no room for the description in the small box: it'd show up as a chopped-off tail.
                    let g = ui.painter().layout_no_wrap(if small { String::new() } else { hint.clone() }, sans(12.5), p.faint);
                    let hx = r.right() - 10.0 - g.size().x;
                    let clip = Rect::from_min_max(r.min, pos2((hx - 10.0).max(r.left() + 120.0), r.bottom()));
                    ui.painter().with_clip_rect(clip).text(pos2(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, label, sans(14.0), p.text);
                    ui.painter().with_clip_rect(Rect::from_min_max(pos2(clip.right() + 8.0, r.top()), r.max)).galley(pos2(hx, r.center().y - g.size().y / 2.0), g, p.faint);
                    if sel && (up || down) {
                        ui.scroll_to_rect(r, None);
                    }
                    if resp.clicked() || (sel && enter) {
                        chosen = Some(action.clone());
                    }
                }
            });
        });

        if let Some(a) = chosen {
            self.focus_prompt = true;
            self.run(a); // may open a follow-up palette (e.g. "Switch model…")
        } else if res.should_close() {
            self.focus_prompt = true;
        } else {
            self.palette = Some(pl);
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = self.toast.clone() else { return };
        let age = self.now() - at;
        if age > 7.0 {
            self.toast = None;
            return;
        }
        let p = self.pal;
        egui::Area::new(Id::unique("toast")).anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0)).order(egui::Order::Tooltip).show(ctx, |ui| {
            let resp = Frame::new().fill(p.panel).stroke(Stroke::new(1.0, p.err.gamma_multiply(0.6))).corner_radius(10).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_max_width(420.0);
                ui.add(Label::new(RichText::new(&text).size(13.0).color(p.text)).wrap());
            });
            if resp.response.interact(Sense::click()).clicked() {
                self.toast = None;
            }
        });
        ctx.request_repaint_after_secs(1.0);
    }
}

// ---------------------------------------------------------------- widgets

fn splash(ui: &mut Ui, p: &Pal, title: &str, sub: &str, spin: bool) {
    ui.add_space((ui.available_height() * 0.36).max(10.0));
    ui.vertical_centered(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
        let t = if spin { ui.input(|i| i.time) as f32 * 1.2 } else { 0.0 };
        logo::paint(ui.painter(), r, p.accent, p.cream, t);
        ui.add_space(14.0);
        ui.label(RichText::new(title).font(FontId::new(20.0, semibold())).color(p.text));
        ui.add_space(2.0);
        ui.add(Label::new(RichText::new(sub).color(p.dim)).wrap());
    });
}

// lay `add` out in a centered column at most `max` wide.
fn column(ui: &mut Ui, max: f32, add: impl FnOnce(&mut Ui)) {
    let full = ui.available_rect_before_wrap();
    let w = full.width().min(max);
    let left = full.left() + (full.width() - w) / 2.0;
    let rect = Rect::from_min_max(pos2(left, full.top()), pos2(left + w, full.bottom()));
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        ui.set_width(w);
        add(ui);
    });
}

// content that belongs to a rail row: shifted right so it lines up with the row's text.
fn indented(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let mut r = ui.available_rect_before_wrap();
    r.min.x += RAIL;
    ui.scope_builder(UiBuilder::new().max_rect(r), |ui| {
        ui.set_width(r.width());
        add(ui);
    });
    ui.add_space(2.0);
}

#[derive(Clone, Copy)]
enum Dot {
    Idle,
    Pulse,
    Good,
    Bad,
}

// one line on the timeline rail: dot, label, dim summary, right-aligned status. whole row is a button.
#[allow(clippy::too_many_arguments)]
fn rail_row(ui: &mut Ui, p: &Pal, prev: &mut Option<Pos2>, dot: Dot, label: &str, mono_label: bool, summary: &str, status: &str, open: bool) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::click());
    let pt = ui.painter();
    let text_rect = Rect::from_min_max(pos2(r.left() + RAIL - 6.0, r.top()), r.max);
    if resp.hovered() || open {
        pt.rect_filled(text_rect, 7, if resp.hovered() { p.raised } else { p.line_soft });
    }

    let c = pos2(r.left() + 8.0, r.center().y);
    if let Some(a) = *prev {
        pt.line_segment([pos2(a.x, a.y + 7.0), pos2(c.x, c.y - 7.0)], Stroke::new(1.5, p.raised_hover));
    }
    *prev = Some(c);
    let col = match dot {
        Dot::Idle => p.faint,
        Dot::Good => p.good,
        Dot::Bad => p.err,
        Dot::Pulse => {
            let a = 0.4 + 0.6 * ((ui.input(|i| i.time) * 5.0).sin() as f32 * 0.5 + 0.5);
            p.accent.gamma_multiply(a)
        }
    };
    pt.circle_filled(c, 3.5, col);

    let lf = if mono_label { mono(13.0) } else { sans(13.5) };
    let lg = pt.layout_no_wrap(label.to_owned(), lf, p.text);
    let sg = pt.layout_no_wrap(status.to_owned(), sans(12.0), p.faint);
    let x0 = r.left() + RAIL + 2.0;
    let sx = r.right() - 10.0 - sg.size().x;
    let x1 = x0 + lg.size().x + 10.0;
    pt.galley(pos2(x0, r.center().y - lg.size().y / 2.0), lg, p.text);
    pt.galley(pos2(sx, r.center().y - sg.size().y / 2.0), sg, p.faint);
    let clip = Rect::from_min_max(pos2(x1, r.top()), pos2((sx - 10.0).max(x1), r.bottom()));
    pt.with_clip_rect(clip).text(pos2(x1, r.center().y), Align2::LEFT_CENTER, summary, mono(12.5), p.dim);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn code_box(ui: &mut Ui, p: &Pal, salt: &str, add: impl FnOnce(&mut Ui)) {
    Frame::new().fill(p.inset).stroke(Stroke::new(1.0, p.line_soft)).corner_radius(8).inner_margin(Margin::symmetric(10, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ScrollArea::vertical().id_salt(("box", salt)).max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            add(ui);
        });
    });
}

// monospace block, cut off after `max_lines` so one giant output can't tank the frame time.
fn mono_text(ui: &mut Ui, text: &str, color: Color32, max_lines: usize) {
    let mut end = 0;
    let mut lines = 0;
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            lines += 1;
            if lines == max_lines {
                end = i;
                break;
            }
        }
    }
    let total = text.lines().count();
    let shown = if lines == max_lines && total > max_lines { &text[..end] } else { text };
    ui.add(Label::new(RichText::new(shown).font(mono(12.5)).color(color)).wrap());
    if shown.len() < text.len() {
        ui.label(RichText::new(format!("… {} more lines", total - max_lines)).size(12.0).color(ui.visuals().weak_text_color()));
    }
}

fn failure(ui: &mut Ui, p: &Pal, text: &str) {
    Frame::new().fill(p.err.gamma_multiply(0.10)).stroke(Stroke::new(1.0, p.err.gamma_multiply(0.45))).corner_radius(10).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(Label::new(RichText::new(text).size(13.5).color(p.err)).wrap());
    });
}

// unified diff -> one colored text layout.
fn diff_job(patch: &str, p: &Pal) -> LayoutJob {
    let mut job = LayoutJob::default();
    for line in patch.lines() {
        if line.starts_with("Index: ") || line.starts_with("====") || line.starts_with("--- ") || line.starts_with("+++ ") {
            continue;
        }
        let (fg, bg) = match line.as_bytes().first() {
            Some(b'+') => (p.good, p.good.gamma_multiply(0.13)),
            Some(b'-') => (p.err, p.err.gamma_multiply(0.13)),
            Some(b'@') => (p.faint, Color32::TRANSPARENT),
            _ => (p.dim, Color32::TRANSPARENT),
        };
        job.append(&format!("{line}\n"), 0.0, TextFormat { font_id: mono(12.5), color: fg, background: bg, ..Default::default() });
    }
    job
}

fn tool_patch(tool: &Tool) -> String {
    tool.meta["files"].as_array().map(|a| a.iter().filter_map(|f| f["patch"].as_str()).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

fn patch_stats(patch: &str) -> (usize, usize) {
    let (mut a, mut d) = (0, 0);
    for l in patch.lines() {
        if l.starts_with('+') && !l.starts_with("+++") {
            a += 1;
        } else if l.starts_with('-') && !l.starts_with("---") {
            d += 1;
        }
    }
    (a, d)
}

fn field_ui(ui: &mut Ui, p: &Pal, f: &Field, st: &mut FieldState) {
    if !f.title.is_empty() {
        ui.label(RichText::new(&f.title).font(FontId::new(13.5, medium())).color(p.text));
    }
    if !f.description.is_empty() {
        ui.add(Label::new(RichText::new(&f.description).size(13.0).color(p.dim)).wrap());
    }
    let multi = f.kind == "multiselect";
    match f.kind.as_str() {
        "boolean" => {
            ui.checkbox(&mut st.flag, "yes");
        }
        "external" => {
            ui.hyperlink(&f.url);
        }
        _ => {
            for o in &f.options {
                let on = st.picked.contains(&o.value);
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if o.description.is_empty() { 28.0 } else { 44.0 }), Sense::click());
                if on || resp.hovered() {
                    ui.painter().rect_filled(r, 8, if on { p.raised } else { p.line_soft });
                }
                let c = pos2(r.left() + 14.0, r.top() + 14.0);
                if multi {
                    let b = Rect::from_center_size(c, vec2(12.0, 12.0));
                    ui.painter().rect_stroke(b, 3, Stroke::new(1.5, if on { p.accent } else { p.faint }), StrokeKind::Inside);
                    if on {
                        ui.painter().rect_filled(b.shrink(3.0), 1, p.accent);
                    }
                } else {
                    ui.painter().circle_stroke(c, 6.0, Stroke::new(1.5, if on { p.accent } else { p.faint }));
                    if on {
                        ui.painter().circle_filled(c, 3.0, p.accent);
                    }
                }
                let pt = ui.painter().with_clip_rect(r);
                pt.text(pos2(r.left() + 30.0, r.top() + 14.0), Align2::LEFT_CENTER, &o.label, sans(13.5), p.text);
                if !o.description.is_empty() {
                    pt.text(pos2(r.left() + 30.0, r.top() + 31.0), Align2::LEFT_CENTER, &o.description, sans(12.5), p.dim);
                }
                if resp.clicked() {
                    if multi {
                        if on {
                            st.picked.retain(|v| v != &o.value);
                        } else {
                            st.picked.push(o.value.clone());
                        }
                    } else {
                        st.picked = vec![o.value.clone()];
                        st.text.clear();
                    }
                }
            }
            if f.options.is_empty() || f.custom {
                Frame::new().fill(p.inset).corner_radius(8).inner_margin(Margin::symmetric(8, 5)).show(ui, |ui| {
                    let hint = if f.options.is_empty() { "your answer" } else { "or type your own" };
                    let r = ui.add(TextEdit::singleline(&mut st.text).frame(Frame::NONE).desired_width(f32::INFINITY).hint_text(RichText::new(hint).color(p.faint)));
                    if r.changed() && !multi && !st.text.is_empty() {
                        st.picked.clear();
                    }
                });
            }
        }
    }
}

pub fn section(ui: &mut Ui, p: &Pal, title: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(title).font(FontId::new(11.5, medium())).color(p.faint));
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0, p.line_soft);
}

// one settings line: label on the left, control(s) on the right.
// the right side is laid out right-to-left, so `add` puts its rightmost widget first.
pub fn setting(ui: &mut Ui, p: &Pal, label: &str, add: impl FnOnce(&mut Ui)) {
    egui::Sides::new().height(28.0).show(ui, |ui| ui.label(RichText::new(label).size(14.0).color(p.text)), add);
}

pub fn note(ui: &mut Ui, p: &Pal, text: &str) {
    ui.add(Label::new(RichText::new(text).size(12.0).color(p.faint)).wrap());
}

// ios-style switch. returns true when flipped.
pub fn toggle(ui: &mut Ui, p: &Pal, on: &mut bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let track = if *on { p.accent } else { p.raised_hover };
    ui.painter().rect_filled(r, 10, track);
    let x = egui::lerp((r.left() + 10.0)..=(r.right() - 10.0), t);
    ui.painter().circle_filled(pos2(x, r.center().y), 7.0, if *on { p.on_accent } else { p.dim });
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub fn solid_button(ui: &mut Ui, p: &Pal, text: &str, primary: bool) -> Response {
    let (fill, fg) = if primary { (p.accent, p.on_accent) } else { (p.raised, p.text) };
    ui.add(egui::Button::new(RichText::new(text).font(FontId::new(13.5, medium())).color(fg)).fill(fill).corner_radius(8))
}

// small pill button. `outlined` draws a border (used on the home screen).
pub fn chip(ui: &mut Ui, p: &Pal, text: &str, outlined: bool) -> Response {
    let g = ui.painter().layout_no_wrap(text.to_owned(), sans(12.5), p.dim);
    let (r, resp) = ui.allocate_exact_size(vec2(g.size().x + 30.0, 26.0), Sense::click());
    let pt = ui.painter();
    if resp.hovered() {
        pt.rect_filled(r, 8, p.raised);
    }
    if outlined {
        pt.rect_stroke(r, 8, Stroke::new(1.0, p.line), StrokeKind::Inside);
    }
    let c = if resp.hovered() { p.text } else { p.dim };
    pt.galley_with_override_text_color(pos2(r.left() + 10.0, r.center().y - g.size().y / 2.0), g, c);
    chevron(pt, pos2(r.right() - 12.0, r.center().y + 0.5), c);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn nav_row(ui: &mut Ui, p: &Pal, text: &str, hint: &str, selected: bool, icon: impl FnOnce(&egui::Painter, Rect, Color32)) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    let pt = ui.painter();
    if selected || resp.hovered() {
        pt.rect_filled(r, 8, p.raised);
    }
    let c = if resp.hovered() { p.text } else { p.dim };
    icon(pt, Rect::from_center_size(pos2(r.left() + 17.0, r.center().y), vec2(16.0, 16.0)), c);
    pt.text(pos2(r.left() + 34.0, r.center().y), Align2::LEFT_CENTER, text, sans(13.5), if resp.hovered() { p.text } else { p.text.gamma_multiply(0.9) });
    pt.text(pos2(r.right() - 8.0, r.center().y), Align2::RIGHT_CENTER, hint, sans(11.5), p.faint);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn icon_button(ui: &mut Ui, p: &Pal, size: f32, icon: impl FnOnce(&egui::Painter, Rect, Color32)) -> Response {
    let (r, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 8, p.raised);
    }
    icon(ui.painter(), Rect::from_center_size(r.center(), vec2(16.0, 16.0)), if resp.hovered() { p.text } else { p.dim });
    resp
}

// ---------------------------------------------------------------- icons (drawn, so no icon font needed)

fn plus_icon(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.6, c);
    pt.line_segment([pos2(r.center().x, r.top() + 2.0), pos2(r.center().x, r.bottom() - 2.0)], s);
    pt.line_segment([pos2(r.left() + 2.0, r.center().y), pos2(r.right() - 2.0, r.center().y)], s);
}

fn search_icon(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.6, c);
    let o = pos2(r.center().x - 1.5, r.center().y - 1.5);
    pt.circle_stroke(o, 4.8, s);
    pt.line_segment([pos2(o.x + 3.6, o.y + 3.6), pos2(r.right() - 1.5, r.bottom() - 1.5)], s);
}

fn sidebar_icon(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.5, c);
    let b = r.shrink2(vec2(0.5, 2.0));
    pt.rect_stroke(b, CornerRadius::same(3), s, StrokeKind::Inside);
    pt.line_segment([pos2(b.left() + 5.5, b.top()), pos2(b.left() + 5.5, b.bottom())], s);
}

fn gear_icon(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(1.6, c);
    let o = r.center();
    pt.circle_stroke(o, 4.2, s);
    for i in 0..8 {
        let a = i as f32 * std::f32::consts::FRAC_PI_4;
        let d = vec2(a.cos(), a.sin());
        pt.line_segment([o + d * 5.2, o + d * 7.4], Stroke::new(2.2, c));
    }
    pt.circle_filled(o, 1.4, c);
}

fn arrow_up(pt: &egui::Painter, r: Rect, c: Color32) {
    let s = Stroke::new(2.0, c);
    let (x, y) = (r.center().x, r.center().y);
    pt.line_segment([pos2(x, y + 6.0), pos2(x, y - 5.5)], s);
    pt.line_segment([pos2(x - 5.0, y - 0.8), pos2(x, y - 6.0)], s);
    pt.line_segment([pos2(x + 5.0, y - 0.8), pos2(x, y - 6.0)], s);
}

fn chevron(pt: &egui::Painter, c: Pos2, col: Color32) {
    let s = Stroke::new(1.3, col);
    pt.line_segment([pos2(c.x - 3.0, c.y - 1.5), pos2(c.x, c.y + 1.5)], s);
    pt.line_segment([pos2(c.x + 3.0, c.y - 1.5), pos2(c.x, c.y + 1.5)], s);
}

// ---------------------------------------------------------------- small helpers

fn flip(set: &mut std::collections::HashSet<String>, key: &str) {
    if !set.remove(key) {
        set.insert(key.to_owned());
    }
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

fn ago(ms: f64) -> String {
    let s = (ms / 1000.0).max(0.0);
    if s < 60.0 {
        "now".to_owned()
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0) as u64)
    } else if s < 86400.0 {
        format!("{}h", (s / 3600.0) as u64)
    } else if s < 86400.0 * 30.0 {
        format!("{}d", (s / 86400.0) as u64)
    } else {
        format!("{}mo", (s / 86400.0 / 30.0) as u64)
    }
}

fn kilo(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1}k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().filter(|s| !s.is_empty()).unwrap_or("/")
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_default()
}

// /home/you/x -> ~/x
pub fn tilde(path: &str) -> String {
    let h = home();
    match path.strip_prefix(&h) {
        Some(rest) if !h.is_empty() && (rest.is_empty() || rest.starts_with('/')) => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

fn expand(path: &str) -> String {
    match path.strip_prefix('~') {
        Some(rest) => format!("{}{rest}", home()),
        None => path.to_owned(),
    }
}

// show a path relative to the session dir when it's inside it.
fn rel(path: &str, dir: &str) -> String {
    match path.strip_prefix(dir) {
        Some(rest) if rest.starts_with('/') => rest[1..].to_owned(),
        _ => tilde(path),
    }
}

pub fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() > max { format!("{}…", line.chars().take(max).collect::<String>()) } else { line.to_owned() }
}
