//! tunnel/routes.rs — таблица пересылки relay-узла: tunnel_id → следующий hop.
//!
//! Заполняется при TUNNEL_BUILD, используется при TUNNEL_DATA (A5).

use std::collections::HashMap;
use std::net::SocketAddr;

use tokio::sync::RwLock;

use crate::types::now_ms;

const MAX_ROUTES: usize = 4096;
/// Маршрутов одного инициатора: иначе один узел занимает всю таблицу ретранслятора и тот никому не строит туннели.
pub const MAX_ROUTES_PER_INITIATOR: usize = 64;

#[derive(Default)]
pub struct TunnelRoutes {
    map: RwLock<HashMap<[u8; 16], (SocketAddr, Option<crate::types::NodeId>, u64)>>,
    /// tunnel_id → инициатор (для квоты).
    owners: RwLock<HashMap<[u8; 16], (crate::types::NodeId, u64)>>,
}

impl TunnelRoutes {
    pub fn new() -> Self { Self::default() }

    /// Запомнить маршрут на `ttl_seconds`. Возвращает false, если таблица переполнена.
    pub async fn insert(&self, tid: [u8; 16], next: SocketAddr, ttl_seconds: u64) -> bool {
        self.insert_with_id(tid, next, None, ttl_seconds).await
    }

    /// То же с NodeID следующего hop — при пересылке он сверяется с NodeID из рукопожатия.
    pub async fn insert_with_id(&self, tid: [u8; 16], next: SocketAddr, next_id: Option<crate::types::NodeId>, ttl_seconds: u64) -> bool {
        let mut m = self.map.write().await;
        let now = now_ms();
        if m.len() >= MAX_ROUTES {
            m.retain(|_, (_, _, exp)| *exp > now);
            if m.len() >= MAX_ROUTES { return false; }
        }
        m.insert(tid, (next, next_id, now + ttl_seconds * 1000));
        true
    }

    /// С квотой на инициатора: false — таблица переполнена или у инициатора уже `MAX_ROUTES_PER_INITIATOR` живых маршрутов.
    pub async fn insert_for(&self, initiator: crate::types::NodeId, tid: [u8; 16], next: SocketAddr,
                            next_id: Option<crate::types::NodeId>, ttl_seconds: u64) -> bool {
        let now = now_ms();
        {
            let mut o = self.owners.write().await;
            o.retain(|_, (_, exp)| *exp > now);
            if o.values().filter(|(id, _)| *id == initiator).count() >= MAX_ROUTES_PER_INITIATOR { return false; }
            o.insert(tid, (initiator, now + ttl_seconds * 1000));
        }
        if self.insert_with_id(tid, next, next_id, ttl_seconds).await { return true; }
        self.owners.write().await.remove(&tid);
        false
    }

    /// Есть ли живой маршрут для этого tunnel_id (защита от повторного/дублирующего BUILD).
    pub async fn contains(&self, tid: &[u8; 16]) -> bool {
        self.next_hop(tid).await.is_some()
    }

    pub async fn next_hop(&self, tid: &[u8; 16]) -> Option<SocketAddr> {
        self.next_hop_with_id(tid).await.map(|x| x.0)
    }

    pub async fn next_hop_with_id(&self, tid: &[u8; 16]) -> Option<(SocketAddr, Option<crate::types::NodeId>)> {
        let m = self.map.read().await;
        m.get(tid).filter(|(_, _, exp)| *exp > now_ms()).map(|(a, id, _)| (*a, *id))
    }
}
