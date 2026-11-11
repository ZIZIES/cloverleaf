// owns the `opencode serve` child process. the gui is just an http client of it.
//
// set CLOVERLEAF_SERVER=http://host:port (+ CLOVERLEAF_PASSWORD) to attach to a server
// that's already running instead of spawning one.

use std::io::Read;
use std::process::{Child, Command, Stdio};

pub struct Server {
    child: Option<Child>,
    pub base: String,
    pub auth: String, // full "Basic xxxx" header value
}

impl Server {
    pub fn start() -> Result<Server, String> {
        if let Ok(base) = std::env::var("CLOVERLEAF_SERVER") {
            let pw = std::env::var("CLOVERLEAF_PASSWORD").unwrap_or_default();
            return Ok(Server { child: None, base: base.trim_end_matches('/').to_owned(), auth: basic("opencode", &pw) });
        }

        // ask the kernel for a free port, then hand it to opencode.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("couldn't find a free port: {e}"))?
            .port();
        let pw = random_hex(24);

        let child = Command::new("opencode")
            .args(["serve", "--hostname", "127.0.0.1", "--port", &port.to_string()])
            .env("OPENCODE_SERVER_PASSWORD", &pw)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("couldn't run `opencode serve` ({e}). is opencode on your PATH?"))?;

        Ok(Server { child: Some(child), base: format!("http://127.0.0.1:{port}"), auth: basic("opencode", &pw) })
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            // SIGTERM first so opencode can shut its mcp servers down, SIGKILL if it ignores us.
            let _ = Command::new("kill").args(["-TERM", &c.id().to_string()]).status();
            for _ in 0..20 {
                if let Ok(Some(_)) = c.try_wait() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn random_hex(n: usize) -> String {
    let mut buf = vec![0u8; n];
    let ok = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut buf)).is_ok();
    if !ok {
        // not a real secret then, but the server only listens on loopback anyway.
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (t >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(151);
        }
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

fn basic(user: &str, pw: &str) -> String {
    format!("Basic {}", base64(format!("{user}:{pw}").as_bytes()))
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}
