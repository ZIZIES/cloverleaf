// custom openai-compatible providers: any server that speaks the openai chat api (vllm, llama.cpp,
// a hosted one opencode has no entry for, a friend's box).
//
// opencode's http api can't add a provider, only its config files can. so cloverleaf keeps a config
// document of its own, ~/.config/cloverleaf/providers.json, and hands it to `opencode serve` at
// launch through OPENCODE_CONFIG (see server.rs). that's why adding or removing one restarts the
// server, and why the tui doesn't see them.
//
// this file is the one place cloverleaf stores an api key. a provider that only exists in config
// has no entry in opencode's key store, so the key has to sit next to it. the file is made 0600,
// and the key is only ever read back by opencode, never loaded into the ui.

use crate::ollama::{SHORT, agent, finish};
use serde_json::{Value, json};

const PACKAGE: &str = "@opencode/ai/providers/openai-compatible";

pub struct Custom {
    pub id: String,
    pub name: String,
    pub url: String,
    pub models: usize,
}

pub fn path() -> std::path::PathBuf {
    crate::app::state_path().with_file_name("providers.json")
}

fn read() -> Value {
    let doc: Value = std::fs::read_to_string(path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null);
    if doc["providers"].is_object() { doc } else { json!({ "providers": {} }) }
}

fn write(doc: &Value) -> Result<(), String> {
    let p = path();
    let err = |e: std::io::Error| format!("{}: {e}", p.display());
    // no providers left: no file, so the server isn't handed an empty document for nothing.
    if doc["providers"].as_object().is_none_or(|o| o.is_empty()) {
        return match std::fs::remove_file(&p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(err(e)),
            _ => Ok(()),
        };
    }
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    let text = serde_json::to_string_pretty(doc).unwrap_or_default();
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // 0600 from the moment it exists, and again in case it already existed with looser bits.
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p).map_err(err)?;
        f.set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(err)?;
        f.write_all(text.as_bytes()).map_err(err)
    }
    #[cfg(not(unix))]
    std::fs::write(&p, text).map_err(err)
}

// what the settings window shows. deliberately leaves the keys on disk.
pub fn list() -> Vec<Custom> {
    let doc = read();
    let mut out: Vec<Custom> = doc["providers"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(id, v)| Custom {
                    id: id.clone(),
                    name: v["name"].as_str().unwrap_or(id).to_owned(),
                    url: v["settings"]["baseURL"].as_str().unwrap_or("").to_owned(),
                    models: v["models"].as_object().map(|m| m.len()).unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|c| c.name.to_lowercase());
    out
}

// "My vLLM box" -> "my-vllm-box": the provider id opencode knows it by.
fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_owned()
}

// the standard "what models do you have" call: GET <base>/models -> { data: [{ id }, ...] }.
// a few servers answer with a bare array, or call the field `models`, or `name` instead of `id`.
fn fetch_models(url: &str, key: &str) -> Result<Vec<String>, String> {
    let full = format!("{url}/models");
    let mut req = agent(SHORT).get(&full);
    if !key.is_empty() {
        req = req.header("Authorization", format!("Bearer {key}"));
    }
    let v: Value = serde_json::from_str(&finish(&full, req.call())?).map_err(|_| format!("{full} didn't answer with json. is that the right base url? it usually ends in /v1"))?;
    let items = [&v["data"], &v["models"], &v].into_iter().find_map(|x| x.as_array()).cloned().unwrap_or_default();
    let ids: Vec<String> = items.iter().filter_map(|m| m.as_str().or(m["id"].as_str()).or(m["name"].as_str()).map(str::to_owned)).collect();
    if ids.is_empty() {
        return Err(format!("{full} listed no models. type them in the models field instead"));
    }
    Ok(ids)
}

fn models_json(ids: &[String]) -> Value {
    Value::Object(ids.iter().map(|id| (id.clone(), json!({ "name": id }))).collect())
}

// `models` is the optional hand-typed list ("a, b, c"); empty = ask the server.
// `taken` = provider ids opencode already has, which a custom one must not shadow.
pub fn add(name: &str, url: &str, key: &str, models: &str, taken: &[String]) -> Result<(), String> {
    let (name, key) = (name.trim(), key.trim());
    let url = url.trim().trim_end_matches('/');
    let id = slug(name);
    if id.is_empty() {
        return Err("it needs a name".to_owned());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("the base url has to start with http:// or https://".to_owned());
    }
    let mut doc = read();
    if taken.contains(&id) || doc["providers"].get(&id).is_some() {
        return Err(format!("there's already a provider called `{id}`. pick another name"));
    }
    let typed: Vec<String> = models.split(',').map(|m| m.trim().to_owned()).filter(|m| !m.is_empty()).collect();
    let ids = if typed.is_empty() { fetch_models(url, key)? } else { typed };
    doc["providers"][&id] = json!({
        "name": name,
        "package": PACKAGE,
        "settings": { "baseURL": url, "apiKey": key },
        "models": models_json(&ids),
    });
    write(&doc)
}

pub fn remove(id: &str) -> Result<(), String> {
    let mut doc = read();
    if let Some(o) = doc["providers"].as_object_mut() {
        o.remove(id);
    }
    write(&doc)
}

// ask the server for its model list again, with the key that's already on disk.
pub fn refresh(id: &str) -> Result<(), String> {
    let mut doc = read();
    let entry = &doc["providers"][id];
    let (url, key) = (entry["settings"]["baseURL"].as_str().unwrap_or("").to_owned(), entry["settings"]["apiKey"].as_str().unwrap_or("").to_owned());
    if url.is_empty() {
        return Err(format!("no provider called `{id}`"));
    }
    doc["providers"][id]["models"] = models_json(&fetch_models(&url, &key)?);
    write(&doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slug("My vLLM box"), "my-vllm-box");
        assert_eq!(slug("  together.ai  "), "together-ai");
        assert_eq!(slug("--"), "");
    }

    // the whole file round trip against a real openai-style endpoint (ollama's), in a throwaway
    // config dir so your real providers.json is never touched. needs ollama running.
    #[test]
    #[ignore]
    fn add_refresh_remove_live() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("cloverleaf-test-{}", std::process::id()));
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };
        let url = "http://127.0.0.1:11434/v1/";

        assert!(add("", url, "", "", &[]).is_err());
        assert!(add("Probe Box", "127.0.0.1:11434", "", "", &[]).is_err());
        assert!(add("Probe Box", url, "", "", &["probe-box".to_owned()]).is_err());
        assert!(add("Probe Box", "http://127.0.0.1:9/v1", "", "", &[]).is_err()); // nothing listens there
        assert!(!path().exists(), "a failed add must not write anything");

        add("Probe Box", url, "sk-test", "", &[]).unwrap();
        let l = list();
        assert_eq!((l.len(), l[0].id.as_str(), l[0].url.as_str()), (1, "probe-box", "http://127.0.0.1:11434/v1"));
        assert!(l[0].models > 0);
        assert_eq!(std::fs::metadata(path()).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(add("probe box", url, "", "", &[]).is_err(), "same id twice");

        add("Typed", url, "", "a, b ,,c", &[]).unwrap();
        assert_eq!(list().iter().find(|c| c.id == "typed").unwrap().models, 3);
        refresh("typed").unwrap();
        assert_eq!(list().iter().find(|c| c.id == "typed").unwrap().models, l[0].models);

        remove("typed").unwrap();
        remove("probe-box").unwrap();
        assert!(!path().exists(), "the file goes away with the last provider");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
