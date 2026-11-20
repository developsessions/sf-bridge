//! sf-bridge — lets the patched chess.com on your phone run Game Review on the Stockfish of your own
//! computer instead of the phone's.
//!
//! Plain UCI over a raw TCP socket, with one handshake in front:
//!
//! ```text
//! server: sf-bridge 1 cores 12
//! client: auth <token>
//! server: auth ok                  (or "auth fail", and the connection is closed)
//! ... then plain UCI in both directions, one command per line, verbatim
//! ```
//!
//! Every connection gets its own Stockfish. Nothing leaves your network: the bridge listens on one
//! port and makes no outbound connections.

use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{IpAddr, Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use sf_bridge::{port, read_line, read_token, token_path};

/// A connection that never authenticates is dropped, so a port scan does not tie up sockets.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Simultaneous connections (each authenticated one is a Stockfish process).
const MAX_CONNECTIONS: usize = 16;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

struct Slot;
impl Slot {
    fn take() -> Option<Slot> {
        if ACTIVE.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            ACTIVE.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(Slot)
    }
}
impl Drop for Slot {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

fn cores() -> usize {
    thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

/// Token: generated on first start and kept in token.txt, so the phone is told only once.
fn load_or_create_token(path: &Path) -> io::Result<String> {
    if path.exists() {
        return read_token(path);
    }
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    write_private(path, &token)?;
    Ok(token)
}

/// Writes the token readable by the owner only (it is the password), where the OS supports that.
fn write_private(path: &Path, contents: &str) -> io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut opts, 0o600);
    opts.open(path)?.write_all(contents.as_bytes())
}

/// `SF_PATH`, else `./stockfish[.exe]`, else the first `stockfish[.exe]` on the PATH.
fn find_stockfish() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("SF_PATH") {
        return Some(PathBuf::from(p));
    }
    let name = if cfg!(windows) {
        "stockfish.exe"
    } else {
        "stockfish"
    };
    let local = Path::new(".").join(name);
    if local.is_file() {
        return Some(local);
    }
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(name))
            .find(|p| p.is_file())
    })
}

/// Comparison that does not stop at the first differing byte.
fn token_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// Every non-loopback IPv4 address of this machine (the phone wants one of them).
fn lan_addresses() -> Vec<Ipv4Addr> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.ip() {
            IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_link_local() => Some(ip),
            _ => None,
        })
        .collect()
}

struct Engine {
    child: Arc<Mutex<Child>>,
    stdin: ChildStdin,
}

fn start_engine(sf: &Path, socket: &TcpStream) -> io::Result<Engine> {
    let mut cmd = Command::new(sf);
    if let Some(dir) = sf.parent().filter(|d| !d.as_os_str().is_empty()) {
        cmd.current_dir(dir);
    }
    // Stockfish's banner goes to stderr: dropped, so the client only ever sees UCI.
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let child = Arc::new(Mutex::new(child));

    let mut out_sock = socket.try_clone()?;
    let child_for_out = Arc::clone(&child);
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if !line.ends_with(b"\n") {
                        line.push(b'\n');
                    }
                    if out_sock.write_all(&line).is_err() {
                        // The phone is gone; the reader side shuts the engine down.
                        return;
                    }
                }
            }
        }
        // Give it a moment to exit by itself, then make sure; never block the lock indefinitely.
        let mut status = None;
        for _ in 0..100 {
            status = child_for_out
                .lock()
                .ok()
                .and_then(|mut c| c.try_wait().ok().flatten());
            if status.is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }
        if status.is_none() {
            if let Ok(mut c) = child_for_out.lock() {
                let _ = c.kill();
                status = c.wait().ok();
            }
        }
        let code = status.and_then(|s| s.code());
        let code = code.map_or("?".to_string(), |c| c.to_string());
        let _ = out_sock
            .write_all(format!("info string bridge: stockfish exited (code {code})\n").as_bytes());
        let _ = out_sock.shutdown(Shutdown::Both);
    });
    Ok(Engine { child, stdin })
}

fn stop_engine(mut engine: Engine) {
    let _ = engine.stdin.write_all(b"quit\n");
    drop(engine.stdin);
    let child = engine.child;
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(2));
        if let Ok(mut c) = child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    });
}

fn handle(mut socket: TcpStream, token: &str, sf: &Path) {
    let remote = socket.peer_addr().map_or("?".into(), |a| a.to_string());
    println!("[tcp] {remote} connected, waiting for auth");
    let _ = socket.set_nodelay(true);
    let _ = socket.set_read_timeout(Some(AUTH_TIMEOUT));
    // The core count goes in the greeting, so the phone's "Auto" threads can mean this machine's cores.
    if socket
        .write_all(format!("sf-bridge 1 cores {}\n", cores()).as_bytes())
        .is_err()
    {
        return;
    }
    let Ok(read_half) = socket.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(read_half);

    // Auth: the first line decides.
    match read_line(&mut reader) {
        Ok(Some(line)) => {
            let presented = line
                .trim()
                .strip_prefix("auth")
                .filter(|r| r.starts_with(char::is_whitespace))
                .map(str::trim);
            let ok = presented.is_some_and(|t| {
                !t.is_empty() && !t.contains(char::is_whitespace) && token_eq(t, token)
            });
            if !ok {
                let _ = socket.write_all(b"auth fail\n");
                let _ = socket.shutdown(Shutdown::Both);
                println!("[tcp] {remote} — wrong token");
                return;
            }
        }
        Ok(None) => {
            println!("[tcp] {remote} disconnected");
            return;
        }
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            println!("[tcp] {remote} did not authenticate in 10 s");
            return;
        }
        Err(_) => return,
    }
    let _ = socket.set_read_timeout(None);
    if socket.write_all(b"auth ok\n").is_err() {
        return;
    }
    println!("[tcp] {remote} authenticated");

    let mut engine = match start_engine(sf, &socket) {
        Ok(e) => e,
        Err(e) => {
            let _ = socket
                .write_all(format!("info string bridge: cannot start stockfish: {e}\n").as_bytes());
            return;
        }
    };

    // Client -> engine, verbatim, one line each.
    let mut alive = true;
    while let Ok(Some(line)) = read_line(&mut reader) {
        if alive
            && engine
                .stdin
                .write_all(format!("{line}\n").as_bytes())
                .and_then(|_| engine.stdin.flush())
                .is_err()
        {
            alive = false; // the engine has already exited; nowhere to send this
        }
    }
    stop_engine(engine);
    println!("[tcp] {remote} disconnected");
}

const USAGE: &str = "Usage: sf-bridge [--port <1-65535>]\n\n  -p, --port <n>   port to listen on (default: $TCP_PORT, else 7892)\n  -h, --help       this text\n\nOther settings: SF_PATH (Stockfish), TOKEN_FILE (default ./token.txt).";

/// `--port N` / `-p N` / `--port=N`; `Ok(None)` when not given. Takes precedence over `TCP_PORT`.
fn port_from_args() -> Result<Option<u16>, String> {
    let mut args = std::env::args().skip(1);
    let mut port = None;
    while let Some(arg) = args.next() {
        let value = match arg.as_str() {
            "-h" | "--help" => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            "-p" | "--port" => args
                .next()
                .ok_or_else(|| format!("{arg} needs a value\n{USAGE}"))?,
            _ => match arg.strip_prefix("--port=") {
                Some(v) => v.to_string(),
                None => return Err(format!("unknown argument: {arg}\n{USAGE}")),
            },
        };
        match value.parse::<u16>() {
            Ok(p) if p > 0 => port = Some(p),
            _ => return Err(format!("not a valid port: {value}")),
        }
    }
    Ok(port)
}

fn main() {
    let port = match port_from_args() {
        Ok(p) => p.unwrap_or_else(port),
        Err(msg) => {
            eprintln!("{msg}");
            std::process::exit(2);
        }
    };
    let token_file = token_path();
    let token = match load_or_create_token(&token_file) {
        Ok(t) if !t.is_empty() => t,
        Ok(_) => {
            return eprintln!(
                "{} is empty; delete it to get a new token.",
                token_file.display()
            )
        }
        Err(e) => return eprintln!("Cannot use {}: {e}", token_file.display()),
    };
    // Absolute, because the engine is started inside its own folder and a relative path would then
    // be resolved from there.
    let sf = match find_stockfish()
        .and_then(|p| std::path::absolute(p).ok())
        .filter(|p| p.is_file())
    {
        Some(p) => p,
        None => {
            eprintln!("No Stockfish found.");
            eprintln!("Put stockfish next to where you start sf-bridge (see README.md), or set SF_PATH to it.");
            std::process::exit(1);
        }
    };

    let listener = match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("Cannot listen on port {port}: {e}");
            std::process::exit(1);
        }
    };
    println!("sf-bridge: UCI over TCP on port {port}");
    for ip in lan_addresses() {
        println!("  on your network: {ip}:{port}  (the address the phone wants)");
    }
    println!("  cores:           {}", cores());
    let chars: Vec<char> = token.chars().collect();
    let shown = if chars.len() > 14 {
        format!(
            "{}…{}",
            chars[..8].iter().collect::<String>(),
            chars[chars.len() - 6..].iter().collect::<String>()
        )
    } else {
        token.clone()
    };
    let full = fs::canonicalize(&token_file).unwrap_or(token_file);
    println!(
        "  token:           {shown}   (whole one in {})",
        full.display()
    );
    println!("Waiting for the phone. Ctrl+C to stop.");

    let token = Arc::new(token);
    let sf = Arc::new(sf);
    for stream in listener.incoming() {
        match stream {
            Ok(socket) => {
                let Some(slot) = Slot::take() else {
                    eprintln!("[tcp] too many connections, dropping one");
                    continue;
                };
                let (token, sf) = (Arc::clone(&token), Arc::clone(&sf));
                thread::spawn(move || {
                    let _slot = slot;
                    handle(socket, &token, &sf);
                });
            }
            Err(e) => eprintln!("[tcp] accept failed: {e}"),
        }
    }
}
