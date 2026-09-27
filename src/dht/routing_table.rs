//! dht/routing_table.rs — K-bucket и таблица маршрутизации Kademlia.
//!
//! Каждый k-bucket хранит до K_BUCKET_SIZE контактов в LRU-порядке
//! (голова = наиболее недавний, хвост = наименее недавний).
//! При вытеснении: проверяем наименее недавний контакт (PING);
//! если он жив — отбрасываем новый; если нет — заменяем им.

use std::collections::VecDeque;

use crate::types::{Contact, NodeId};

/// Один k-bucket.
#[derive(Debug, Default)]
pub struct KBucket {
    contacts:   VecDeque<Contact>,
    capacity:   usize,
}

impl KBucket {
    pub fn new(capacity: usize) -> Self {
        Self { contacts: VecDeque::new(), capacity }
    }

    pub fn len(&self) -> usize { self.contacts.len() }
    pub fn is_full(&self) -> bool { self.contacts.len() >= self.capacity }

    /// Содержит ли bucket контакт с данным node_id?
    pub fn contains(&self, node_id: &NodeId) -> bool {
        self.contacts.iter().any(|c| &c.node_id == node_id)
    }

    /// Обновить или добавить контакт.
    /// Возвращает `Some(lru_contact)` если bucket полон и нужна PING-проверка.
    pub fn update(&mut self, contact: Contact) -> Option<Contact> {
        // Если уже есть — переместить в голову (самый свежий)
        if let Some(pos) = self.contacts.iter().position(|c| c.node_id == contact.node_id) {
            self.contacts.remove(pos);
            self.contacts.push_back(contact);
            return None;
        }

        if !self.is_full() {
            self.contacts.push_back(contact);
            return None;
        }

        // Bucket полон — возвращаем LRU (хвост) для PING-проверки
        self.contacts.front().cloned()
    }

    /// Удалить контакт по node_id (вызывается если PING LRU провалился).
    /// Затем добавляем новый.
    pub fn evict_lru_and_insert(&mut self, new_contact: Contact) {
        self.contacts.pop_front();
        self.contacts.push_back(new_contact);
    }

    /// Удалить контакт (узел ушёл offline).
    pub fn remove(&mut self, node_id: &NodeId) {
        self.contacts.retain(|c| &c.node_id != node_id);
    }

    pub fn iter(&self) -> impl Iterator<Item = &Contact> {
        self.contacts.iter()
    }

    /// Снимок для журнала/экспорта метрик (размер bucket).
    pub fn snapshot(&self) -> Vec<Contact> {
        self.contacts.iter().cloned().collect()
    }
}

// ── Таблица маршрутизации ─────────────────────────────────────────────────────

/// Таблица маршрутизации: 256 k-bucket (по числу бит SHA-256 NodeID).
pub struct RoutingTable {
    own_id:   NodeId,
    buckets:  Vec<KBucket>,
    k:        usize,
}

impl RoutingTable {
    pub fn new(own_id: NodeId, k: usize) -> Self {
        let buckets = (0..=255).map(|_| KBucket::new(k)).collect();
        Self { own_id, buckets, k }
    }

    /// Индекс bucket для данного node_id (длина общего префикса XOR).
    fn bucket_index(&self, node_id: &NodeId) -> usize {
        self.own_id.bucket_index(node_id)
    }

    /// Добавить/обновить контакт. Возвращает LRU-контакт если нужна PING-проверка.
    pub fn update(&mut self, contact: Contact) -> Option<Contact> {
        if contact.node_id == self.own_id {
            return None; // себя не добавляем
        }
        let idx = self.bucket_index(&contact.node_id);
        self.buckets[idx].update(contact)
    }

    /// Удалить контакт (PING провален или явно исключён).
    pub fn remove(&mut self, node_id: &NodeId) {
        let idx = self.bucket_index(node_id);
        self.buckets[idx].remove(node_id);
    }

    /// Вытеснить LRU и добавить новый контакт.
    pub fn evict_lru_and_insert(&mut self, node_id: &NodeId, new_contact: Contact) {
        let idx = self.bucket_index(node_id);
        self.buckets[idx].evict_lru_and_insert(new_contact);
    }

    /// Найти K ближайших к target контактов (не включая собственный ID).
    pub fn find_closest(&self, target: &NodeId, count: usize) -> Vec<Contact> {
        let mut all: Vec<&Contact> = self.buckets.iter()
            .flat_map(|b| b.iter())
            .collect();

        // Сортировка по XOR-расстоянию до target
        all.sort_by_key(|c| c.node_id.xor_distance(target));
        all.into_iter().take(count).cloned().collect()
    }

    /// Суммарное число контактов во всех bucket.
    pub fn total_contacts(&self) -> usize {
        self.buckets.iter().map(|b| b.len()).sum()
    }

    /// Число заполненных (len > 0) bucket.
    pub fn non_empty_buckets(&self) -> usize {
        self.buckets.iter().filter(|b| b.len() > 0).count()
    }

    /// Проверить, находится ли контакт в таблице.
    pub fn contains(&self, node_id: &NodeId) -> bool {
        let idx = self.bucket_index(node_id);
        self.buckets[idx].contains(node_id)
    }

    /// Snapshot всех непустых bucket (для метрик и логов).
    pub fn snapshot(&self) -> Vec<(usize, Vec<Contact>)> {
        self.buckets.iter()
            .enumerate()
            .filter(|(_, b)| b.len() > 0)
            .map(|(i, b)| (i, b.snapshot()))
            .collect()
    }

    pub fn own_id(&self) -> &NodeId { &self.own_id }
    pub fn k(&self) -> usize { self.k }
}
