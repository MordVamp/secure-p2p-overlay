//! api.rs — клиент локального HTTP API узла (`p2p-node --gui-port`).
//!
//! Только std: сервер узла — HTTP/1.1 c `Connection: close`, параметры в query,
//! ответы — JSON. Слушает только 127.0.0.1.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use serde_json::Value;

/// Выполнить запрос к API узла на `127.0.0.1:port`.
pub fn call(port: u16, method: &str, path: &str, query: &[(&str, &str)], timeout: Duration) -> Result<Value, String> {
    let mut target = path.to_owned();
    if !query.is_empty() {
        target.push('?');
        target.push_str(&query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect::<Vec<_>>().join("&"));
    }
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(2))
        .map_err(|e| format!("нет связи с узлом 127.0.0.1:{port}: {e}"))?;
    s.set_read_timeout(Some(timeout)).ok();
    s.set_write_timeout(Some(Duration::from_secs(2))).ok();
    write!(s, "{method} {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
        .map_err(|e| format!("запрос: {e}"))?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("ответ: {e}"))?;
    let text = String::from_utf8_lossy(&buf);
    let (head, body) = text.split_once("\r\n\r\n").ok_or("некорректный HTTP-ответ")?;
    let status: u16 = head.split_whitespace().nth(1).and_then(|x| x.parse().ok()).unwrap_or(0);
    let v: Value = serde_json::from_str(body).map_err(|e| format!("JSON: {e}"))?;
    if status != 200 {
        return Err(v["error"].as_str().unwrap_or("ошибка HTTP").to_owned());
    }
    Ok(v)
}

pub fn get(port: u16, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    call(port, "GET", path, query, Duration::from_secs(10))
}

/// POST; отправка сообщения ждёт APP_ACK до 8 с плюс построение маршрута.
pub fn post(port: u16, path: &str, query: &[(&str, &str)]) -> Result<Value, String> {
    call(port, "POST", path, query, Duration::from_secs(40))
}

/// Ответ API с `ok: false` превращается в ошибку с текстом узла.
pub fn ok(v: Result<Value, String>) -> Result<Value, String> {
    let v = v?;
    if v.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(v["error"].as_str().unwrap_or("операция не выполнена").to_owned());
    }
    Ok(v)
}

fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Строка из JSON-поля (пустая, если нет).
pub fn s(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(x) => x.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn u(v: &Value, k: &str) -> Option<u64> { v[k].as_u64() }

pub fn short(id: &str) -> String { id.chars().take(8).collect() }

pub fn is_node_id(s: &str) -> bool { s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()) }
