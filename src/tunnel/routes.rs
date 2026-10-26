//! tunnel/routes.rs — таблица пересылки relay-узла: tunnel_id → следующий hop.
//!
//! Заполняется при TUNNEL_BUILD, используется при TUNNEL_DATA (A5).

use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::RwLock;

use crate::types::now_ms;

const MAX_ROUTES: usize = 4096;

#[derive(Default)]
pub struct TunnelRoutes {
    map: RwLock<HashMap<[u8; 16], (SocketAddr, u64)>>,
}

impl TunnelRoutes {
    pub fn new() -> Self { Self::default() }

    /// Запомнить маршрут на `ttl_seconds`. Возвращает false, если таблица переполнена.
    pub async fn insert(&self, tid: [u8; 16], next: SocketAddr, ttl_seconds: u64) -> bool {
        let mut m = self.map.write().await;
        let now = now_ms();
        if m.len() >= MAX_ROUTES {
            m.retain(|_, (_, exp)| *exp > now);
            if m.len() >= MAX_ROUTES { return false; }
        }
        m.insert(tid, (next, now + ttl_seconds * 1000));
        true
    }

    pub async fn next_hop(&self, tid: &[u8; 16]) -> Option<SocketAddr> {
        let m = self.map.read().await;
        m.get(tid).filter(|(_, exp)| *exp > now_ms()).map(|(a, _)| *a)
    }
}
