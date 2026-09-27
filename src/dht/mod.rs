//! dht/ — Kademlia DHT.
//! routing_table: k-bucket + RoutingTable  (✅ Фаза 4)
//! lookup:        итеративный lookup       (✅ Фаза 4)
//! node:          DhtNode, join/find_node  (✅ Фаза 5)

pub mod routing_table;
pub mod lookup;
pub mod node;
