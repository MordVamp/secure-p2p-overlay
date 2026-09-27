//! rpc/pending.rs — Трекер ожидающих RPC-ответов по request_id (§7 ТЗ).

use std::collections::HashMap;
use tokio::sync::{Mutex, oneshot};
use tracing::warn;
use crate::transport::framing::Frame;

pub struct PendingRpc {
    map: Mutex<HashMap<[u8; 16], oneshot::Sender<Frame>>>,
}

impl PendingRpc {
    pub fn new() -> Self { Self { map: Mutex::new(HashMap::new()) } }

    pub async fn register(&self, request_id: [u8; 16]) -> oneshot::Receiver<Frame> {
        let (tx, rx) = oneshot::channel();
        self.map.lock().await.insert(request_id, tx);
        rx
    }

    pub async fn deliver(&self, request_id: [u8; 16], frame: Frame) {
        if let Some(tx) = self.map.lock().await.remove(&request_id) {
            let _ = tx.send(frame);
        } else {
            warn!("PendingRpc: no waiter for {:x?}", &request_id[..4]);
        }
    }

    pub async fn pending_count(&self) -> usize { self.map.lock().await.len() }
}

impl Default for PendingRpc { fn default() -> Self { Self::new() } }
