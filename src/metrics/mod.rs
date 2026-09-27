//! metrics/ — Сбор и экспорт метрик эксперимента (Фаза 8).
//!
//! Собирает: RTT PING, статистику lookup, состояние routing table, туннели.
//! Экспорт: CSV (для воспроизводимого эксперимента) + JSON (для анализа).
//!
//! Все данные пишутся в `metrics_dir` (из конфига). По умолчанию ./metrics/

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::{debug, warn};

use crate::types::{NodeId, now_ms};

// ── Записи метрик ────────────────────────────────────────────────────────────

/// Одно наблюдение PING RTT.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingRecord {
    pub ts_ms:       u64,
    pub from_node:   String,   // hex[:8]
    pub to_node:     String,
    pub rtt_ms:      u64,
    pub success:     bool,
}

/// Результат одного итеративного lookup.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LookupRecord {
    pub ts_ms:         u64,
    pub lookup_id:     String,
    pub initiator:     String,   // hex[:8]
    pub target:        String,
    pub found:         bool,
    pub rpc_count:     usize,
    pub iterations:    usize,
    pub duration_ms:   u64,
    pub closest_count: usize,
}

/// Результат построения туннеля.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelBuildRecord {
    pub ts_ms:          u64,
    pub tunnel_id:      String,
    pub initiator:      String,
    pub target:         String,
    pub hops:           usize,
    pub success:        bool,
    pub build_time_ms:  u64,
    pub failure_reason: Option<String>,
}

/// Событие восстановления туннеля (DEGRADED→ACTIVE или DEAD→rebuild).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelRecoveryRecord {
    pub ts_ms:         u64,
    pub tunnel_id:     String,
    pub from_state:    String,
    pub to_state:      String,
    pub recovery_ms:   u64,
    pub success:       bool,
}

/// Снапшот routing table одного узла.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingSnapshotRecord {
    pub ts_ms:           u64,
    pub node_id:         String,
    pub total_contacts:  usize,
    pub non_empty_buckets: usize,
    pub has_full_knowledge: bool,  // знает ли всех (признак вырождения)
    pub node_count_estimate: usize,
}

// ── MetricsCollector ─────────────────────────────────────────────────────────

pub struct MetricsCollector {
    dir:      PathBuf,
    node_id:  String,
}

impl MetricsCollector {
    pub fn new(dir: &Path, node_id: &NodeId) -> Self {
        let _ = fs::create_dir_all(dir);
        Self { dir: dir.to_path_buf(), node_id: node_id.short() }
    }

    // ── Запись в CSV ─────────────────────────────────────────────────────────

    pub fn record_ping(&self, r: &PingRecord) {
        let path = self.dir.join(format!("ping_{}.csv", self.node_id));
        self.append_csv(&path, &format!(
            "{},{},{},{},{}\n",
            r.ts_ms, r.from_node, r.to_node, r.rtt_ms,
            if r.success { 1 } else { 0 }
        ), "ts_ms,from_node,to_node,rtt_ms,success\n");
    }

    pub fn record_lookup(&self, r: &LookupRecord) {
        let path = self.dir.join(format!("lookup_{}.csv", self.node_id));
        self.append_csv(&path, &format!(
            "{},{},{},{},{},{},{},{},{}\n",
            r.ts_ms, r.lookup_id, r.initiator, r.target,
            if r.found { 1 } else { 0 },
            r.rpc_count, r.iterations, r.duration_ms, r.closest_count
        ), "ts_ms,lookup_id,initiator,target,found,rpc_count,iterations,duration_ms,closest_count\n");
    }

    pub fn record_tunnel_build(&self, r: &TunnelBuildRecord) {
        let path = self.dir.join(format!("tunnel_build_{}.csv", self.node_id));
        self.append_csv(&path, &format!(
            "{},{},{},{},{},{},{},{}\n",
            r.ts_ms, r.tunnel_id, r.initiator, r.target,
            r.hops, if r.success { 1 } else { 0 },
            r.build_time_ms,
            r.failure_reason.as_deref().unwrap_or("")
        ), "ts_ms,tunnel_id,initiator,target,hops,success,build_time_ms,failure_reason\n");
    }

    pub fn record_tunnel_recovery(&self, r: &TunnelRecoveryRecord) {
        let path = self.dir.join(format!("tunnel_recovery_{}.csv", self.node_id));
        self.append_csv(&path, &format!(
            "{},{},{},{},{},{}\n",
            r.ts_ms, r.tunnel_id, r.from_state, r.to_state,
            r.recovery_ms, if r.success { 1 } else { 0 }
        ), "ts_ms,tunnel_id,from_state,to_state,recovery_ms,success\n");
    }

    pub fn record_routing_snapshot(&self, r: &RoutingSnapshotRecord) {
        let path = self.dir.join(format!("routing_snapshot_{}.csv", self.node_id));
        self.append_csv(&path, &format!(
            "{},{},{},{},{},{}\n",
            r.ts_ms, r.node_id, r.total_contacts, r.non_empty_buckets,
            if r.has_full_knowledge { 1 } else { 0 },
            r.node_count_estimate
        ), "ts_ms,node_id,total_contacts,non_empty_buckets,has_full_knowledge,node_count_estimate\n");
    }

    // ── JSON снапшот (полный экспорт состояния узла) ─────────────────────────

    pub fn snapshot_to_json(&self, data: serde_json::Value) {
        let path = self.dir.join(format!("snapshot_{}_{}.json", self.node_id, now_ms()));
        match serde_json::to_string_pretty(&data) {
            Ok(s) => { let _ = fs::write(&path, s); }
            Err(e) => warn!("metrics json: {e}"),
        }
    }

    // ── Внутреннее ───────────────────────────────────────────────────────────

    fn append_csv(&self, path: &Path, line: &str, header: &str) {
        let write_header = !path.exists();
        let mut f = match OpenOptions::new().create(true).append(true).open(path) {
            Ok(f) => f,
            Err(e) => { warn!("metrics csv open {:?}: {e}", path); return; }
        };
        if write_header { let _ = f.write_all(header.as_bytes()); }
        let _ = f.write_all(line.as_bytes());
        debug!("metrics → {:?}", path.file_name().unwrap_or_default());
    }
}
