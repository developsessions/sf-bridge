//! Checks the bridge the way the phone's NetEngine does: connect, authenticate, ask one position for
//! a best move. Run it while sf-bridge is running, from the folder with token.txt.
//!
//!   cargo run --release --bin test-tcp
//!   TEST_NODES=1000000 TEST_HOST=192.168.1.10 cargo run --release --bin test-tcp

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use sf_bridge::{port, read_line, read_token, token_path};

const FEN: &str = "r1bq1rk1/pp2ppbp/2np1np1/8/2BNP3/2N1BP2/PPPQ2PP/R3K2R w KQ - 0 10";

fn env<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn main() {
    if let Err(e) = run() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let token = read_token(&token_path()).map_err(|e| format!("token.txt: {e}"))?;
    let host: String = env("TEST_HOST", "127.0.0.1".to_string());
    let port = port();
    let nodes: u64 = env("TEST_NODES", 300_000);
    let threads: u32 = env("TEST_THREADS", 8);

    let started = Instant::now();
    let mut sock = TcpStream::connect((host.as_str(), port)).map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    sock.set_nodelay(true).ok();
    println!("connected to {host}:{port}");
    let mut reader = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
    let mut next = move || -> Result<String, String> {
        loop {
            match read_line(&mut reader).map_err(|e| e.to_string())? {
                Some(l) if l.trim().is_empty() => continue,
                Some(l) => return Ok(l.trim().to_string()),
                None => return Err("connection closed".into()),
            }
        }
    };
    let mut send = |line: &str| {
        sock.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| e.to_string())
    };

    println!("server: {}", next()?);
    send(&format!("auth {token}"))?;
    let reply = next()?;
    println!("server: {reply}");
    if reply != "auth ok" {
        return Err(
            "AUTH FAILED: the token here and on the phone must be the one in token.txt".into(),
        );
    }
    send("uci")?;
    while next()? != "uciok" {}
    send(&format!("setoption name Threads value {threads}"))?;
    send("setoption name Hash value 256")?;
    send("setoption name MultiPV value 3")?;
    send("isready")?;
    while next()? != "readyok" {}
    send("ucinewgame")?;
    send(&format!("position fen {FEN}"))?;
    send(&format!("go nodes {nodes}"))?;
    println!("engine ready, searching {nodes} nodes");
    loop {
        let line = next()?;
        if let Some(rest) = line.strip_prefix("bestmove") {
            println!("bestmove: {}", rest.split_whitespace().next().unwrap_or(""));
            println!(
                "total: {:.2} s from connecting",
                started.elapsed().as_secs_f64()
            );
            send("quit")?;
            return Ok(());
        }
        if line.starts_with("info ") {
            let w: Vec<&str> = line.split_whitespace().collect();
            let after = |key: &str| w.iter().position(|x| *x == key).map(|i| i + 1);
            if let (Some(d), Some(s), Some(p)) = (after("depth"), after("score"), after("pv")) {
                if s + 1 < w.len() && d < w.len() {
                    println!(
                        "  d{} {} {} | pv {}",
                        w[d],
                        w[s],
                        w[s + 1],
                        w[p..].iter().take(6).copied().collect::<Vec<_>>().join(" ")
                    );
                }
            }
        }
    }
}
