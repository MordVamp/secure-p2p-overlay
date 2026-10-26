//! gui/http.rs — минимальный HTTP/1.1 сервер без внешних зависимостей.
//!
//! Только localhost. Защита: проверка Host (DNS-rebinding), для POST —
//! проверка Origin (CSRF со сторонних страниц).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{debug, info, warn};

pub struct Request {
    pub method: String,
    pub path:   String,
    pub query:  HashMap<String, String>,
}

pub struct Response {
    pub status: u16,
    pub ctype:  &'static str,
    pub body:   String,
}

impl Response {
    pub fn html(body: &str) -> Self { Self { status: 200, ctype: "text/html; charset=utf-8", body: body.to_owned() } }
    pub fn json(v: serde_json::Value) -> Self { Self { status: 200, ctype: "application/json", body: v.to_string() } }
    pub fn error(status: u16, msg: &str) -> Self {
        Self { status, ctype: "application/json", body: serde_json::json!({ "error": msg }).to_string() }
    }
}

pub type Handler = Arc<dyn Fn(Request) -> BoxFuture<'static, Response> + Send + Sync>;

/// Обернуть async-функцию в `Handler`.
pub fn handler<F, Fut>(f: F) -> Handler
where
    F: Fn(Request) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Response> + Send + 'static,
{
    Arc::new(move |r| Box::pin(f(r)))
}

pub async fn serve(port: u16, h: Handler) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let listener = TcpListener::bind(&addr).await.with_context(|| format!("GUI bind {addr}"))?;
    info!("GUI: http://{addr}/");
    loop {
        let (sock, _) = listener.accept().await?;
        let h = h.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_conn(sock, h).await { debug!("GUI conn: {e}"); }
        });
    }
}

async fn handle_conn(mut s: TcpStream, h: Handler) -> Result<()> {
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(5), s.read(&mut chunk)).await??;
        if n == 0 { return Ok(()); }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") { break; }
        if buf.len() > 16 * 1024 { return Ok(()); }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    let mut lines = text.split("\r\n");
    let first = lines.next().unwrap_or("");
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let target = parts.next().unwrap_or("/");
    let (path, qs) = target.split_once('?').unwrap_or((target, ""));

    let mut host = String::new();
    let mut origin: Option<String> = None;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            match k.trim().to_ascii_lowercase().as_str() {
                "host"   => host = v.trim().to_owned(),
                "origin" => origin = Some(v.trim().to_owned()),
                _ => {}
            }
        }
    }

    let host_ok = |x: &str| {
        let h = x.rsplit_once(':').map(|(a, _)| a).unwrap_or(x);
        h == "127.0.0.1" || h == "localhost"
    };
    let resp = if !host_ok(&host) {
        warn!("GUI: rejected Host={host}");
        Response::error(403, "bad host")
    } else if method == "POST" && origin.as_deref().map(|o| !host_ok(o.trim_start_matches("http://"))).unwrap_or(false) {
        warn!("GUI: rejected Origin={origin:?}");
        Response::error(403, "bad origin")
    } else if method != "GET" && method != "POST" {
        Response::error(405, "method not allowed")
    } else {
        let query = qs.split('&').filter(|p| !p.is_empty()).map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (url_decode(k), url_decode(v))
        }).collect();
        h(Request { method, path: path.to_owned(), query }).await
    };

    let reason = match resp.status { 200 => "OK", 400 => "Bad Request", 403 => "Forbidden", 404 => "Not Found", 405 => "Method Not Allowed", _ => "Error" };
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n",
        resp.status, reason, resp.ctype, resp.body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(resp.body.as_bytes()).await?;
    s.shutdown().await.ok();
    Ok(())
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => { out.push(b' '); i += 1; }
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) { out.push(v); i += 3; }
                else { out.push(b'%'); i += 1; }
            }
            c => { out.push(c); i += 1; }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
