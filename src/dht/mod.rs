//! dht/ — Kademlia DHT.
//!
//! Фаза 4: routing_table (k-bucket), lookup (итеративный), node (join/find),
//!         storage (STORE/FIND_VALUE с TTL)

pub mod routing_table;
pub mod lookup;
pub mod node;
pub mod storage;
