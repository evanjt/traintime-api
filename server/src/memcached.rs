use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

/// A call slower than this is treated as a failure. Memcached sits one hop
/// away in the cluster; a slow answer is worth less than an upstream fetch.
const OP_TIMEOUT: Duration = Duration::from_millis(50);
/// How long Memcached is skipped after a failure before it is tried again.
const SKIP_FOR: Duration = Duration::from_secs(30);
/// Idle connections kept per pod. More than this are dropped after use.
const POOL_SIZE: usize = 8;
/// Memcached's own limit; longer keys are simply not shared.
const MAX_KEY_LEN: usize = 250;

type Conn = BufReader<TcpStream>;

/// Cache shared between replicas over the Memcached text protocol. Every
/// call is bounded by `OP_TIMEOUT`, a failure skips the server for
/// `SKIP_FOR`, and nothing here returns an error: a miss is the worst case.
#[derive(Clone)]
pub struct Memcached {
    addr: String,
    idle: Arc<Mutex<Vec<Conn>>>,
    skip_until: Arc<Mutex<Option<Instant>>>,
}

impl Memcached {
    /// `host:port`, with or without a `memcached://` scheme.
    pub fn new(url: &str) -> Self {
        let addr = url
            .trim()
            .trim_start_matches("memcached://")
            .trim_end_matches('/');
        Self {
            addr: addr.to_string(),
            idle: Arc::new(Mutex::new(Vec::new())),
            skip_until: Arc::new(Mutex::new(None)),
        }
    }

    pub fn addr(&self) -> &str {
        &self.addr
    }

    pub async fn get(&self, key: &str) -> Option<String> {
        if !valid_key(key) {
            return None;
        }
        self.call(|conn| get(conn, key.to_string())).await.flatten()
    }

    /// A TTL of zero would mean "never expire" to Memcached, so it is not
    /// written: the in-pod cache treats it as already expired.
    pub async fn set(&self, key: &str, value: &str, ttl_secs: u64) {
        if !valid_key(key) || ttl_secs == 0 {
            return;
        }
        self.call(|conn| set(conn, key.to_string(), value.to_string(), ttl_secs))
            .await;
    }

    fn skipping(&self) -> bool {
        let mut skip = self.skip_until.lock().unwrap();
        match *skip {
            Some(until) if until > Instant::now() => true,
            Some(_) => {
                *skip = None;
                false
            }
            None => false,
        }
    }

    fn mark_failed(&self) {
        *self.skip_until.lock().unwrap() = Some(Instant::now() + SKIP_FOR);
    }

    /// Runs one command on a pooled connection inside the timeout. A failed
    /// or slow call drops its connection and starts the skip window.
    async fn call<T, F, Fut>(&self, op: F) -> Option<T>
    where
        F: FnOnce(Conn) -> Fut,
        Fut: Future<Output = (Conn, std::io::Result<T>)>,
    {
        if self.skipping() {
            return None;
        }
        let pooled = self.idle.lock().unwrap().pop();
        let attempt = async {
            let conn = match pooled {
                Some(c) => c,
                None => BufReader::new(TcpStream::connect(&self.addr).await?),
            };
            let (conn, out) = op(conn).await;
            let out = out?;
            let mut idle = self.idle.lock().unwrap();
            if idle.len() < POOL_SIZE {
                idle.push(conn);
            }
            Ok::<T, std::io::Error>(out)
        };
        match tokio::time::timeout(OP_TIMEOUT, attempt).await {
            Ok(Ok(v)) => Some(v),
            Ok(Err(e)) => {
                println!("memcached {}: {e}", self.addr);
                self.mark_failed();
                None
            }
            Err(_) => {
                println!("memcached {}: timeout after {:?}", self.addr, OP_TIMEOUT);
                self.mark_failed();
                None
            }
        }
    }
}

fn valid_key(key: &str) -> bool {
    !key.is_empty() && key.len() <= MAX_KEY_LEN && !key.bytes().any(|b| b <= b' ' || b == 0x7f)
}

fn protocol_error(what: &str) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("memcached protocol: {what}"),
    )
}

async fn read_line(conn: &mut Conn) -> std::io::Result<String> {
    let mut line = String::new();
    if conn.read_line(&mut line).await? == 0 {
        return Err(protocol_error("connection closed"));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_string())
}

async fn get(mut conn: Conn, key: String) -> (Conn, std::io::Result<Option<String>>) {
    let out = get_on(&mut conn, &key).await;
    (conn, out)
}

async fn set(
    mut conn: Conn,
    key: String,
    value: String,
    ttl_secs: u64,
) -> (Conn, std::io::Result<()>) {
    let out = set_on(&mut conn, &key, &value, ttl_secs).await;
    (conn, out)
}

async fn get_on(conn: &mut Conn, key: &str) -> std::io::Result<Option<String>> {
    conn.get_mut()
        .write_all(format!("get {key}\r\n").as_bytes())
        .await?;
    let header = read_line(conn).await?;
    if header == "END" {
        return Ok(None);
    }
    // VALUE <key> <flags> <bytes>
    let len: usize = header
        .strip_prefix("VALUE ")
        .and_then(|rest| rest.split(' ').nth(2))
        .and_then(|n| n.parse().ok())
        .ok_or_else(|| protocol_error(&header))?;
    let mut data = vec![0u8; len + 2];
    conn.read_exact(&mut data).await?;
    data.truncate(len);
    if read_line(conn).await? != "END" {
        return Err(protocol_error("missing END"));
    }
    String::from_utf8(data)
        .map(Some)
        .map_err(|_| protocol_error("value is not UTF-8"))
}

async fn set_on(conn: &mut Conn, key: &str, value: &str, ttl_secs: u64) -> std::io::Result<()> {
    let mut msg = format!("set {key} 0 {ttl_secs} {}\r\n", value.len()).into_bytes();
    msg.extend_from_slice(value.as_bytes());
    msg.extend_from_slice(b"\r\n");
    conn.get_mut().write_all(&msg).await?;
    match read_line(conn).await?.as_str() {
        "STORED" => Ok(()),
        other => Err(protocol_error(other)),
    }
}

#[cfg(test)]
pub mod fake {
    //! An in-memory Memcached speaking enough of the text protocol for the
    //! server's get and set, so tests need no daemon.
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    pub struct Fake {
        pub addr: String,
        pub store: Arc<Mutex<HashMap<String, (String, u64)>>>,
        pub accepted: Arc<AtomicUsize>,
    }

    /// `hang` accepts connections and never answers them.
    pub async fn start(hang: bool) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let store: Arc<Mutex<HashMap<String, (String, u64)>>> = Default::default();
        let accepted = Arc::new(AtomicUsize::new(0));
        let (s, a) = (store.clone(), accepted.clone());
        tokio::spawn(async move {
            loop {
                let (sock, _) = listener.accept().await.unwrap();
                a.fetch_add(1, Ordering::SeqCst);
                if hang {
                    tokio::spawn(async move {
                        let _keep = sock;
                        std::future::pending::<()>().await;
                    });
                    continue;
                }
                let store = s.clone();
                tokio::spawn(async move {
                    let mut conn = BufReader::new(sock);
                    loop {
                        let mut line = String::new();
                        if conn.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        let parts: Vec<&str> = line.trim_end().split(' ').collect();
                        match parts.as_slice() {
                            ["get", key] => {
                                let hit = store.lock().unwrap().get(*key).cloned();
                                let reply = match hit {
                                    Some((v, _)) => {
                                        format!("VALUE {key} 0 {}\r\n{v}\r\nEND\r\n", v.len())
                                    }
                                    None => "END\r\n".to_string(),
                                };
                                conn.get_mut().write_all(reply.as_bytes()).await.unwrap();
                            }
                            ["set", key, _flags, ttl, len] => {
                                let n: usize = len.parse().unwrap();
                                let mut data = vec![0u8; n + 2];
                                conn.read_exact(&mut data).await.unwrap();
                                data.truncate(n);
                                store.lock().unwrap().insert(
                                    key.to_string(),
                                    (String::from_utf8(data).unwrap(), ttl.parse().unwrap()),
                                );
                                conn.get_mut().write_all(b"STORED\r\n").await.unwrap();
                            }
                            _ => conn.get_mut().write_all(b"ERROR\r\n").await.unwrap(),
                        }
                    }
                });
            }
        });
        Fake {
            addr,
            store,
            accepted,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::*;

    #[tokio::test]
    async fn set_then_get_round_trips_with_the_ttl() {
        let server = fake::start(false).await;
        let mc = Memcached::new(&format!("memcached://{}", server.addr));
        assert_eq!(mc.get("k").await, None);
        mc.set("k", "{\"a\":1}\r\nmore", 60).await;
        assert_eq!(mc.get("k").await.as_deref(), Some("{\"a\":1}\r\nmore"));
        assert_eq!(server.store.lock().unwrap()["k"].1, 60);
    }

    #[tokio::test]
    async fn connections_are_reused() {
        let server = fake::start(false).await;
        let mc = Memcached::new(&server.addr);
        for _ in 0..5 {
            mc.get("k").await;
        }
        assert_eq!(server.accepted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn zero_ttl_and_bad_keys_are_not_written() {
        let server = fake::start(false).await;
        let mc = Memcached::new(&server.addr);
        mc.set("k", "v", 0).await;
        mc.set("has space", "v", 60).await;
        mc.set(&"x".repeat(251), "v", 60).await;
        assert!(server.store.lock().unwrap().is_empty());
        assert_eq!(server.accepted.load(Ordering::SeqCst), 0);
    }

    // Scenario: Memcached is unreachable, or accepts and never answers.
    // Expected behaviour: the first call gives up inside the timeout, the
    // next ones skip the server entirely, and no call ever fails.
    #[tokio::test]
    async fn unreachable_server_is_skipped_after_one_failure() {
        let mc = Memcached::new("127.0.0.1:1");
        let started = Instant::now();
        assert_eq!(mc.get("k").await, None);
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(mc.skipping());
        mc.set("k", "v", 60).await;
        assert_eq!(mc.get("k").await, None);
    }

    #[tokio::test]
    async fn hung_server_times_out_and_is_skipped() {
        let server = fake::start(true).await;
        let mc = Memcached::new(&server.addr);
        let started = Instant::now();
        assert_eq!(mc.get("k").await, None);
        let first = started.elapsed();
        assert!(
            first >= OP_TIMEOUT && first < Duration::from_secs(1),
            "{first:?}"
        );
        for _ in 0..3 {
            mc.get("k").await;
        }
        assert_eq!(server.accepted.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn skip_window_ends() {
        let server = fake::start(false).await;
        let mc = Memcached::new(&server.addr);
        *mc.skip_until.lock().unwrap() = Some(Instant::now() - Duration::from_millis(1));
        mc.set("k", "v", 60).await;
        assert_eq!(mc.get("k").await.as_deref(), Some("v"));
    }
}
