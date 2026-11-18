//! A small client for the core's socket, used by the organs that run as their own processes
//! (reach, mail), so each keeps its own privileges.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

pub struct Client {
    pub path: PathBuf,
}

impl Client {
    pub fn new(path: PathBuf) -> Self {
        Client { path }
    }

    pub fn request(&self, op: &str, fields: Value) -> std::io::Result<Value> {
        let mut req = json!({"op": op});
        if let Value::Object(m) = fields {
            for (k, v) in m {
                req[k] = v;
            }
        }
        let mut s = UnixStream::connect(&self.path)?;
        writeln!(s, "{req}")?;
        let mut line = String::new();
        BufReader::new(s).read_line(&mut line)?;
        serde_json::from_str(&line).map_err(|e| std::io::Error::other(e.to_string()))
    }

    /// Every event from now on, as they happen (blocks; ends if the core goes away).
    pub fn follow(&self) -> std::io::Result<impl Iterator<Item = Value>> {
        let mut s = UnixStream::connect(&self.path)?;
        writeln!(s, "{}", json!({"op": "follow"}))?;
        let mut reader = BufReader::new(s);
        let mut first = String::new();
        reader.read_line(&mut first)?;
        Ok(reader.lines().map_while(Result::ok).filter_map(|l| serde_json::from_str(&l).ok()))
    }

    pub fn wait_until_up(&self) {
        while self.request("ping", json!({})).is_err() {
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    }
}
