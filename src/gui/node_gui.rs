//! gui/node_gui.rs — веб-панель одного узла: статус, routing table, туннели,
//! DHT-хранилище + действия PING / FIND_NODE / TUNNEL / STORE / FIND_VALUE.

use std::net::SocketAddr;
use std::sync::Arc;

use serde_json::json;

use super::http::{handler, serve, Request, Response};
use crate::dht::storage::RecordKey;
use crate::node::Node;
use crate::types::{now_ms, NodeId};

const PAGE: &str = include_str!("node.html");

fn parse_node_id(s: &str) -> Result<NodeId, String> {
    let b = hex::decode(s.trim()).map_err(|_| "target: ожидается hex".to_string())?;
    let arr: [u8; 32] = b.try_into().map_err(|_| "target: нужно 64 hex-символа".to_string())?;
    Ok(NodeId(arr))
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
            Response::json(json!({
                "node_id": node.own_contact.node_id.to_hex(),
                "addr":    format!("{}:{}", node.own_contact.host, node.own_contact.port),
                "level":   format!("{:?}", node.cfg.level),
                "k":       node.cfg.dht.k_bucket_size,
                "alpha":   node.cfg.dht.alpha,
                "routing": routing,
                "tunnels": tunnels,
                "store":   store,
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
                Ok(()) => Response::json(json!({ "ok": true })),
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
            let target = match parse_node_id(req.query.get("target").map(|s| s.as_str()).unwrap_or("")) {
                Ok(t) => t, Err(e) => return Response::error(400, &e),
            };
            let Some(text) = req.query.get("text").filter(|t| !t.is_empty()) else { return Response::error(400, "text пуст"); };
            match node.messenger.send_text(target, text).await {
                Ok(()) => Response::json(json!({ "ok": true })),
                Err(e) => Response::json(json!({ "ok": false, "error": e.to_string() })),
            }
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
