//! gui/node_gui.rs — веб-панель одного узла: статус, routing table, туннели,
//! DHT-хранилище + действия PING / FIND_NODE / TUNNEL / STORE / FIND_VALUE,
//! сообщения (NodeID или `@псевдоним`), история, псевдонимы.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;

use super::http::{handler, serve, Request, Response};
use crate::dht::storage::RecordKey;
use crate::node::Node;
use crate::types::{now_ms, NodeId};

const PAGE: &str = include_str!("node.html");

/// Номер безопасности пары узлов (как «safety number»): 60 цифр, одинаковые у обоих собеседников.
/// Совпадение при личной сверке значит, что ключ собеседника не подменён (NodeID = SHA-256 ключа).
pub fn safety_number(a: &NodeId, b: &NodeId) -> String {
    use sha2::{Digest, Sha256};
    let (lo, hi) = if a.0 <= b.0 { (a, b) } else { (b, a) };
    let mut h = Sha256::new();
    h.update(b"p2p-overlay-safety-v1");
    h.update(lo.0);
    h.update(hi.0);
    let d = h.finalize();
    // 6 кусков по 5 байт → 10-значные числа → 12 групп по 5 цифр
    let mut groups = vec![];
    for c in d[..30].chunks(5) {
        let n = c.iter().fold(0u64, |acc, &x| (acc << 8) | x as u64) % 10_000_000_000;
        let s = format!("{n:010}");
        groups.push(s[..5].to_owned());
        groups.push(s[5..].to_owned());
    }
    groups.join(" ")
}

fn parse_node_id(s: &str) -> Result<NodeId, String> {
    let b = hex::decode(s.trim()).map_err(|_| "target: ожидается hex".to_string())?;
    let arr: [u8; 32] = b.try_into().map_err(|_| "target: нужно 64 hex-символа".to_string())?;
    Ok(NodeId(arr))
}

/// Получатель: 64 hex NodeID либо `@псевдоним` (разрешается через DHT).
async fn resolve_target(node: &Node, s: &str) -> Result<NodeId, String> {
    match s.trim().strip_prefix('@') {
        Some(alias) => node.dht.resolve_alias(alias).await.map_err(|e| e.to_string()),
        None => parse_node_id(s),
    }
}

pub async fn run(node: Arc<Node>, port: u16) -> anyhow::Result<()> {
    serve(port, handler(move |req| route(node.clone(), req))).await
}

async fn route(node: Arc<Node>, req: Request) -> Response {
    let post = req.method == "POST";
    match (req.path.as_str(), post) {
        ("/", false) => Response::html(PAGE),
        ("/api/status", false) => {
            let routing = node.dht.export_routing_table().await;
            let tunnels = node.tunnel_mgr.export_json().await;
            let store   = node.dht.export_store().await;
            let pool    = crate::rpc::pool::stats().await;
            Response::json(json!({
                "node_id": node.own_contact.node_id.to_hex(),
                "addr":    format!("{}:{}", node.own_contact.host, node.own_contact.port),
                // ссылка-приглашение (QR в desktop/Android): адрес для bootstrap + NodeID для контакта
                "link":    format!("p2p-overlay://{}:{}?id={}", node.own_contact.host, node.own_contact.port,
                                   node.own_contact.node_id.to_hex()),
                "level":   format!("{:?}", node.cfg.level),
                "channel": node.cfg.security.channel,
                "k":       node.cfg.dht.k_bucket_size,
                "alpha":   node.cfg.dht.alpha,
                "routing": routing,
                "tunnels": tunnels,
                "store":   store,
                "bootstrap": node.dht.bootstrap_json(),
                // что умеет узел: клиенты (desktop-приложение) включают только поддержанные разделы
                "features": { "messages": true, "aliases": true, "history": true, "files": true, "calls": true,
                              "events": true, "typing": true, "read_receipts": true, "groups": true,
                              // звонки: 1:1 и групповые (до 5), звук Opus через локальный аудиоканал
                              "call_signaling": true, "group_calls": true,
                              "mailbox": node.cfg.mailbox.enabled },
                // режим «только клиент»: узел не ретранслирует чужие туннели
                "relay":   node.cfg.tunnel.relay,
                "typing":  node.messenger.typing_now().await,
                "outbox":  node.messenger.outbox_summary().await,
                "events_seq": node.messenger.events.last_seq(),
                // соединения между узлами: пул (transport.reuse_connections) — открыто всего / запросов по открытому / открыто сейчас
                "conn_pool": { "enabled": crate::rpc::pool::enabled(), "scope": crate::rpc::pool::scope(),
                               "opened": pool.0, "reused": pool.1, "open": pool.2 },
                "now_ms":  now_ms(),
            }))
        }
        ("/api/ping", true) => {
            let Some(addr) = req.query.get("addr").and_then(|a| a.parse::<SocketAddr>().ok())
                else { return Response::error(400, "addr: ожидается host:port"); };
            let t0 = now_ms();
            match node.dht.rpc_client.ping(
                addr, node.cfg.transport.connect_timeout_ms,
                node.cfg.transport.read_timeout_ms, &node.own_contact,
            ).await {
                Ok(p) => Response::json(json!({ "ok": true, "responder": p.responder.to_string(), "rtt_ms": now_ms() - t0 })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/lookup", true) => {
            let target = match parse_node_id(req.query.get("target").map(|s| s.as_str()).unwrap_or("")) {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            match node.dht.find_node(&target).await {
                Ok(r) => Response::json(json!({
                    "ok": true, "rpc_count": r.rpc_count, "iterations": r.iterations,
                    "duration_ms": r.duration_ms, "lookup_id": r.lookup_id,
                    "target_known_before": r.target_known_before, "found_target": r.found_target,
                    "queried_by_iter": r.queried_by_iter,
                    "closest": r.closest.iter().map(|c| json!({"node_id": c.node_id.to_hex(), "addr": format!("{}:{}", c.host, c.port)})).collect::<Vec<_>>(),
                })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/tunnel", true) => {
            let target = match parse_node_id(req.query.get("target").map(|s| s.as_str()).unwrap_or("")) {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            match node.tunnel_mgr.get_or_build(&target).await {
                Ok(built) => Response::json(json!({ "ok": true, "reused": built.is_none(), "build_ms": built })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/store", true) => {
            let (Some(k), Some(v)) = (req.query.get("key"), req.query.get("value"))
                else { return Response::error(400, "нужны key и value"); };
            if k.is_empty() || v.len() > 4096 { return Response::error(400, "key пуст или value > 4096"); }
            match node.dht.put(&node.identity, RecordKey::from_str(k), v.clone().into_bytes(),
                               node.cfg.dht.ttl_seconds).await {
                Ok(n)  => Response::json(json!({ "ok": true, "stored_on": n })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/find", false) => {
            let Some(k) = req.query.get("key") else { return Response::error(400, "нужен key"); };
            match node.dht.get(&RecordKey::from_str(k)).await {
                Some(r) => Response::json(json!({ "found": true, "value": String::from_utf8_lossy(&r.value),
                                                  "publisher": r.publisher.short(), "ttl_left": r.remaining_ttl_secs() })),
                None    => Response::json(json!({ "found": false })),
            }
        }
        ("/api/send", true) => {
            let Some(text) = req.query.get("text").filter(|t| !t.is_empty()) else { return Response::error(400, "text пуст"); };
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::json(json!({ "ok": false, "delivered": false, "error": e })),
            };
            // ждём APP_ACK: «доставлено» = получатель расшифровал, проверил подпись и подтвердил
            match node.messenger.send_text_acked(target, text, std::time::Duration::from_secs(8)).await {
                Ok(r) => Response::json(json!({
                    "ok": true, "delivered": !r.queued, "target": target.to_hex(), "message_id": r.message_id, "rtt_ms": r.rtt_ms,
                    "lookup_ms": r.lookup_ms, "tunnel_reused": r.tunnel_build_ms.is_none(),
                    "tunnel_build_ms": r.tunnel_build_ms, "tunnel_send_ms": r.tunnel_send_ms,
                    "rebuilt_after_failure": r.rebuilt_after_failure, "failover": r.failover,
                    "detect_ms": r.detect_ms, "switch_ms": r.switch_ms, "wire_bytes": r.wire_bytes,
                    "queued": r.queued, "queued_reason": r.queued_reason,
                })),
                Err(e)  => Response::json(json!({ "ok": false, "delivered": false, "error": e.to_string() })),
            }
        }
        ("/api/send_file", true) => {
            let Some(path) = req.query.get("path").filter(|p| !p.is_empty()) else { return Response::error(400, "нужен path"); };
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::json(json!({ "ok": false, "error": e })),
            };
            match node.messenger.send_file(target, std::path::Path::new(path)).await {
                Ok(r)  => Response::json(json!({ "ok": true, "report": r })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/events", false) => {
            let since = req.query.get("since").and_then(|s| s.parse().ok()).unwrap_or(0u64);
            let wait = req.query.get("timeout").and_then(|s| s.parse().ok()).unwrap_or(25u64).min(30);
            let (last, ev) = node.messenger.events.wait(since, std::time::Duration::from_secs(wait)).await;
            Response::json(json!({ "seq": last, "events": ev }))
        }
        ("/api/typing", true) => {
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            match node.messenger.send_typing(target).await {
                Ok(sent) => Response::json(json!({ "ok": true, "sent": sent })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/read", true) => {
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            let ids: Vec<String> = req.query.get("ids").map(|s| s.split(',').map(|x| x.trim().to_owned())
                .filter(|x| !x.is_empty()).collect()).unwrap_or_default();
            match node.messenger.send_read(target, &ids).await {
                Ok(()) => Response::json(json!({ "ok": true })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/send_group", true) => {
            let (Some(gid), Some(text)) = (req.query.get("group_id"), req.query.get("text").filter(|t| !t.is_empty()))
                else { return Response::error(400, "нужны group_id и text"); };
            let name = req.query.get("name").cloned().unwrap_or_default();
            let mut members = vec![];
            for m in req.query.get("members").map(|s| s.as_str()).unwrap_or("").split(',').filter(|m| !m.trim().is_empty()) {
                match parse_node_id(m) { Ok(id) => members.push(id), Err(e) => return Response::error(400, &e) }
            }
            if members.is_empty() || members.len() > 32 { return Response::error(400, "участников: 1..32"); }
            match node.messenger.send_group(gid, &name, &members, text, std::time::Duration::from_secs(8)).await {
                Ok(r) => Response::json(json!({ "ok": true, "results": r })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/history/delete", true) => {
            let Some(peer) = req.query.get("peer").filter(|p| !p.is_empty()) else { return Response::error(400, "нужен peer"); };
            match node.messenger.delete_history(peer).await {
                Ok(n) => Response::json(json!({ "ok": true, "removed": n })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/watch", true) => {
            for p in req.query.get("peers").map(|s| s.as_str()).unwrap_or("").split(',').filter(|p| !p.trim().is_empty()) {
                if let Ok(id) = parse_node_id(p) { node.messenger.watch_peer(id).await; }
            }
            Response::json(json!({ "ok": true }))
        }
        ("/api/mailbox/poll", true) => Response::json(json!({ "ok": true, "received": node.messenger.poll_mailboxes().await })),
        ("/api/safety", false) => {
            let peer = match parse_node_id(req.query.get("peer").map(|s| s.as_str()).unwrap_or("")) {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            Response::json(json!({ "peer": peer.to_hex(), "safety": safety_number(&node.own_contact.node_id, &peer) }))
        }
        // план звонков, шаг 0: измерительный поток (как голос — 50 пакетов/с) и результаты у получателя
        ("/api/probe", true) => {
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            let q = |k: &str, d: u64| req.query.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
            let stream = req.query.get("stream").cloned().unwrap_or_else(|| "probe".into());
            let (count, interval, size) = (q("count", 250).min(5000) as u32, q("interval_ms", 20).max(1), q("size", 80) as usize);
            match node.messenger.probe_stream(target, &stream, count, interval, size).await {
                Ok(v) => Response::json(json!({ "ok": true, "report": v })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/probe_results", false) => {
            let stream = req.query.get("stream").cloned().unwrap_or_else(|| "probe".into());
            Response::json(json!({ "stream": stream, "packets": node.messenger.probe_results(&stream).await }))
        }
        // звонки (шаги 2–4): сигнализация, ключ звонка, медиакадры; звук — шаг 5 (пока синтетический поток)
        ("/api/call/start", true) => {
            // групповой: members=<id,id>&name=…[&chat=<id группового чата>]; иначе target=<NodeID или @псевдоним>
            let r = if let Some(m) = req.query.get("members") {
                let mut ids = vec![];
                for x in m.split(',').map(str::trim).filter(|x| !x.is_empty()) {
                    match parse_node_id(x) { Ok(id) => ids.push(id), Err(e) => return Response::error(400, &e) }
                }
                let name = req.query.get("name").map(|s| s.as_str()).unwrap_or("Групповой звонок");
                node.messenger.call_start_group(name, ids, req.query.get("chat").cloned()).await
            } else {
                let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                    Ok(t) => t, Err(e) => return Response::error(400, &e),
                };
                node.messenger.call_start(target).await
            };
            match r {
                Ok(id) => Response::json(json!({ "ok": true, "call_id": id })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/call/accept", true) | ("/api/call/reject", true) | ("/api/call/hangup", true) => {
            let Some(id) = req.query.get("call_id") else { return Response::error(400, "нужен call_id"); };
            let r = match req.path.as_str() {
                "/api/call/accept" => node.messenger.call_accept(id).await,
                "/api/call/reject" => node.messenger.call_end(id, "rejected").await,
                _ => node.messenger.call_end(id, "hangup").await,
            };
            match r {
                Ok(()) => Response::json(json!({ "ok": true })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/call/media", true) => {
            let Some(id) = req.query.get("call_id") else { return Response::error(400, "нужен call_id"); };
            let q = |k: &str, d: u64| req.query.get(k).and_then(|v| v.parse().ok()).unwrap_or(d);
            match node.messenger.call_send_media(id, q("count", 250).min(30_000) as u32, q("interval_ms", 20).max(1),
                                                 q("size", 80) as usize).await {
                Ok(v) => Response::json(json!({ "ok": true, "report": v })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/calls", false) => Response::json(json!({
            "sessions": node.messenger.call_sessions_json().await, "calls": node.messenger.calls_json().await,
            "mic_level": node.messenger.mic_level(),
        })),
        // локальный аудиоканал клиента: UDP 127.0.0.1:port, токен — в каждом кадре (app/voice.rs)
        ("/api/call/audio", false) => match node.messenger.audio_link().await {
            Ok((port, token)) => Response::json(json!({ "ok": true, "port": port, "token": hex::encode(token),
                "rate": crate::app::voice::RATE, "frame_samples": crate::app::voice::FRAME, "codec": crate::app::voice::CODEC })),
            Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
        },
        // тестовый источник вместо микрофона: тон freq Гц на seconds с; в ответ — какие из listen слышны
        ("/api/call/tone", true) => {
            let f = |k: &str, d: f64| req.query.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
            let listen: Vec<f64> = req.query.get("listen").map(|l| l.split(',').filter_map(|x| x.trim().parse().ok()).collect())
                .unwrap_or_else(|| vec![440.0, 554.0, 660.0, 784.0, 880.0]);
            let rep = node.messenger.call_tone(f("freq", 440.0), f("seconds", 5.0).clamp(0.1, 120.0), &listen).await;
            Response::json(json!({ "ok": true, "report": rep }))
        }
        ("/api/relays", false) => Response::json(json!({ "profiles": node.tunnel_mgr.profiles_json().await })),
        ("/api/files", false) => {
            let (t, dir) = node.messenger.transfers().await;
            Response::json(json!({ "downloads_dir": dir.display().to_string(), "transfers": t }))
        }
        ("/api/history", false) => {
            let limit = req.query.get("limit").and_then(|l| l.parse().ok()).unwrap_or(100usize).min(10_000);
            Response::json(json!({ "entries": node.messenger.history(limit).await }))
        }
        ("/api/alias", true) => {
            let Some(name) = req.query.get("name") else { return Response::error(400, "нужен name"); };
            match node.dht.publish_alias(&node.identity, name).await {
                Ok(n)  => Response::json(json!({ "ok": true, "alias": name, "stored_on": n })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/resolve", false) => {
            let Some(name) = req.query.get("alias") else { return Response::error(400, "нужен alias"); };
            match node.dht.resolve_alias(name).await {
                Ok(id) => Response::json(json!({ "ok": true, "alias": name, "node_id": id.to_hex() })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
        }
        ("/api/route", false) => {
            let target = match resolve_target(&node, req.query.get("target").map(|s| s.as_str()).unwrap_or("")).await {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            Response::json(node.messenger.route_diag(&target).await)
        }
        ("/api/inbox", false) => {
            let msgs: Vec<_> = node.messenger.recent().await.iter().map(|m| json!({
                "from": m.from.to_hex(), "text": m.text().unwrap_or("<binary>"), "ts": m.timestamp_ms,
            })).collect();
            Response::json(json!({ "messages": msgs }))
        }
        _ => Response::error(404, "not found"),
    }
}
