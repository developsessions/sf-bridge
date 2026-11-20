//! What the app's "Benchmark your computer" button does, from the command line: one connection, one
//! Stockfish, then 100k / 300k / 1M / 5M nodes on one position each. Start-up is timed separately.
//!
//!   cargo run --release --bin bench-tcp

use std::io::{BufReader, Write};
use std::net::TcpStream;
use std::time::Instant;

use sf_bridge::{port, read_line, read_token, token_path};

const BUDGETS: [u64; 4] = [100_000, 300_000, 1_000_000, 5_000_000];
const FEN: &str = "r1bq1rk1/pp2ppbp/2np1np1/8/2BNP3/2N1BP2/PPPQ2PP/R3K2R w KQ - 0 10";

fn main() {
    if let Err(e) = run() {
        eprintln!("ERROR: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let token = read_token(&token_path()).map_err(|e| format!("token.txt: {e}"))?;
    let host = std::env::var("TEST_HOST").unwrap_or_else(|_| "127.0.0.1".into());

    let opened = Instant::now();
    let mut sock = TcpStream::connect((host.as_str(), port())).map_err(|e| e.to_string())?;
    sock.set_nodelay(true).ok();
    sock.set_read_timeout(Some(std::time::Duration::from_secs(120)))
        .ok();
    let mut reader = BufReader::new(sock.try_clone().map_err(|e| e.to_string())?);
    let mut until = move |prefix: &str| -> Result<String, String> {
        loop {
            match read_line(&mut reader).map_err(|e| e.to_string())? {
                Some(l) if l.trim().starts_with(prefix) => return Ok(l.trim().to_string()),
                Some(_) => {}
                None => return Err("connection closed".into()),
            }
        }
    };
    let mut send = |line: &str| {
        sock.write_all(format!("{line}\n").as_bytes())
            .map_err(|e| e.to_string())
    };

    let greeting = until("sf-bridge")?;
    let cores: usize = greeting
        .split_whitespace()
        .skip_while(|w| *w != "cores")
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(8);
    send(&format!("auth {token}"))?;
    until("auth ok").map_err(|_| "auth failed".to_string())?;
    send("uci")?;
    until("uciok")?;
    let threads: usize = std::env::var("TEST_THREADS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(cores);
    let hash = (cores * 32).clamp(128, 1024);
    send(&format!("setoption name Threads value {threads}"))?;
    send(&format!("setoption name Hash value {hash}"))?;
    send("setoption name MultiPV value 1")?;
    send("isready")?;
    until("readyok")?;
    let startup = opened.elapsed().as_secs_f64();

    let mut times = Vec::new();
    for nodes in BUDGETS {
        send("ucinewgame")?;
        send(&format!("position fen {FEN}"))?;
        let t = Instant::now();
        send(&format!("go nodes {nodes}"))?;
        until("bestmove")?;
        times.push(t.elapsed().as_secs_f64());
    }
    send("quit")?;

    println!("Startup (once per review): {startup:.2} s");
    println!("threads {threads}, hash {hash} MB\n");
    for (n, t) in BUDGETS.iter().zip(&times) {
        let label = if *n >= 1_000_000 {
            format!("{}M", n / 1_000_000)
        } else {
            format!("{}k", n / 1_000)
        };
        println!("{label:<4} nodes a move: {t:.2} s");
    }
    Ok(())
}
