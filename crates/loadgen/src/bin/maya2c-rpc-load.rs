//! `maya2c-rpc-load --targets HOST:PORT[,HOST:PORT…] [--threads T] [--secs S]`
//!
//! Mixed read load against one or more nodes' JSON-RPC (Master Prompt 14 §5):
//! `get_tip_height`, `get_supply` and `get_balance` in a 2:1:1 ratio, over
//! persistent HTTP/1.1 connections, each thread pinned round-robin to one
//! target. Prints requests per second, errors, and p50/p99 latency.
//!
//! Standard library only, like the rest of this crate: a load generator with
//! its own async runtime would measure that runtime as much as the server.
//! Read the result together with the host: a curve taken with every node and
//! the generator on one machine measures per-request server cost, not
//! horizontal scaling.

#![allow(clippy::cast_precision_loss)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

const METHODS: [&str; 4] = [
    r#"{"jsonrpc":"2.0","id":1,"method":"get_tip_height","params":[]}"#,
    r#"{"jsonrpc":"2.0","id":2,"method":"get_supply","params":[]}"#,
    r#"{"jsonrpc":"2.0","id":3,"method":"get_tip_height","params":[]}"#,
    r#"{"jsonrpc":"2.0","id":4,"method":"get_balance","params":["1111111111111111111111111111111111111111111111111111111111111111"]}"#,
];

/// One request on an open connection; returns whether it got a JSON-RPC result.
fn call(reader: &mut BufReader<TcpStream>, host: &str, body: &str) -> std::io::Result<bool> {
    let req = format!(
        "POST / HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    reader.get_mut().write_all(req.as_bytes())?;
    let mut len = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
        if line == "\r\n" {
            break;
        }
    }
    let mut body = vec![0; len];
    reader.read_exact(&mut body)?;
    Ok(body.windows(8).any(|w| w == b"\"result\""))
}

/// One worker: its own connection, for `secs`. Returns (latencies µs, errors).
fn worker(target: &str, secs: u64, offset: usize) -> (Vec<u32>, u64) {
    let mut lat = Vec::new();
    let mut errors = 0;
    let Ok(stream) = TcpStream::connect(target) else {
        return (lat, 1);
    };
    let _ = stream.set_nodelay(true);
    let mut reader = BufReader::new(stream);
    let end = Instant::now() + Duration::from_secs(secs);
    let mut i = offset;
    while Instant::now() < end {
        let t = Instant::now();
        match call(&mut reader, target, METHODS[i % METHODS.len()]) {
            Ok(true) => lat.push(u32::try_from(t.elapsed().as_micros()).unwrap_or(u32::MAX)),
            Ok(false) => errors += 1,
            Err(_) => {
                errors += 1;
                break;
            }
        }
        i += 1;
    }
    (lat, errors)
}

fn main() -> Result<(), String> {
    let (mut targets, mut threads, mut secs) = (Vec::new(), 8usize, 10u64);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let v = args.next().ok_or(format!("{a} needs a value"))?;
        match a.as_str() {
            "--targets" => targets = v.split(',').map(str::to_string).collect(),
            "--threads" => threads = v.parse().map_err(|e| format!("{e}"))?,
            "--secs" => secs = v.parse().map_err(|e| format!("{e}"))?,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if targets.is_empty() {
        return Err("--targets is required".into());
    }
    let handles: Vec<_> = (0..threads)
        .map(|t| {
            let target = targets[t % targets.len()].clone();
            std::thread::spawn(move || worker(&target, secs, t))
        })
        .collect();
    let mut all = Vec::new();
    let mut errors = 0;
    for h in handles {
        let (lat, e) = h.join().map_err(|_| "worker panicked")?;
        all.extend(lat);
        errors += e;
    }
    all.sort_unstable();
    let pct = |p: usize| all.get(all.len() * p / 100).copied().unwrap_or(0);
    println!(
        "targets {} threads {threads} secs {secs}: {:.0} req/s, {errors} errors, p50 {} µs, p99 {} µs",
        targets.len(),
        all.len() as f64 / secs as f64,
        pct(50),
        pct(99)
    );
    Ok(())
}
