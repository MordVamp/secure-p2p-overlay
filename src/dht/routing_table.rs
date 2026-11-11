//! dht/routing_table.rs — K-bucket и таблица маршрутизации Kademlia.
//!
//! Каждый k-bucket хранит до K_BUCKET_SIZE контактов в LRU-порядке
//! (голова = наиболее недавний, хвост = наименее недавний).
//! При вытеснении: проверяем наименее недавний контакт (PING);
//! если он жив — отбрасываем новый; если нет — заменяем им.

use std::collections::VecDeque;

use crate::types::{Contact, NodeId};

/// Сколько узлов держит кэш замены одной корзины (Kademlia §2.4).
pub const REPLACEMENTS: usize = 8;

/// Один k-bucket.
#[derive(Debug, Default)]
pub struct KBucket {
    contacts:   VecDeque<Contact>,
    capacity:   usize,
    /// Кэш замены (Kademlia §2.4): узлы, не поместившиеся в полную корзину или увиденные в ответах
    /// FIND_NODE (самые свежие — в конце). Занимают место удалённого контакта и служат кандидатами
    /// в ретрансляторы — иначе при малом k у недавно вошедшего узла их может не хватить.
    replacements: VecDeque<Contact>,
}

impl KBucket {
    pub fn new(capacity: usize) -> Self {
        Self { contacts: VecDeque::new(), capacity, replacements: VecDeque::new() }
    }

    /// Запомнить узел в кэше замены. Уже находящийся в корзине — не дублируется.
    pub fn note_replacement(&mut self, contact: Contact) {
        if self.contains(&contact.node_id) { return; }
        self.replacements.retain(|c| c.node_id != contact.node_id);
        self.replacements.push_back(contact);
        while self.replacements.len() > REPLACEMENTS { self.replacements.pop_front(); }
    }

    pub fn replacements(&self) -> impl Iterator<Item = &Contact> { self.replacements.iter() }

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
            self.replacements.retain(|c| c.node_id != contact.node_id);
            self.contacts.push_back(contact);
            return None;
        }

        // Bucket полон — новый узел ждёт в кэше замены, LRU (хвост) возвращается для PING-проверки
        self.note_replacement(contact);
        self.contacts.front().cloned()
    }

    /// Удалить контакт по node_id (вызывается если PING LRU провалился).
    /// Затем добавляем новый.
    pub fn evict_lru_and_insert(&mut self, new_contact: Contact) {
        self.contacts.pop_front();
        self.replacements.retain(|c| c.node_id != new_contact.node_id);
        self.contacts.push_back(new_contact);
    }

    /// Удалить контакт (узел ушёл offline); его место занимает самый свежий узел из кэша замены.
    pub fn remove(&mut self, node_id: &NodeId) {
        let n = self.contacts.len();
        self.contacts.retain(|c| &c.node_id != node_id);
        self.replacements.retain(|c| &c.node_id != node_id);
        if self.contacts.len() < n && !self.is_full() {
            if let Some(c) = self.replacements.pop_back() { self.contacts.push_back(c); }
        }
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

    /// Запомнить узел, увиденный в ответе FIND_NODE (пара NodeID/ключ проверена, живость — нет):
    /// только в кэш замены его корзины — в саму таблицу попадают лишь ответившие узлы.
    pub fn note_seen(&mut self, contact: Contact) {
        if contact.node_id == self.own_id { return; }
        let idx = self.bucket_index(&contact.node_id);
        self.buckets[idx].note_replacement(contact);
    }

    /// Кандидаты в ретрансляторы: контакты таблицы и кэшей замены по возрастанию XOR-расстояния до `target`.
    pub fn relay_candidates(&self, target: &NodeId) -> Vec<Contact> {
        let mut all: Vec<(bool, &Contact)> = self.buckets.iter()
            .flat_map(|b| b.iter().map(|c| (false, c)).chain(b.replacements().map(|c| (true, c))))
            .collect();
        all.sort_by_key(|(spare, c)| (c.node_id.xor_distance(target), *spare));
        let mut seen = std::collections::HashSet::new();
        all.into_iter().filter(|(_, c)| seen.insert(c.node_id)).map(|(_, c)| c.clone()).collect()
    }

    /// Узлов в кэшах замены (для статуса).
    pub fn replacement_count(&self) -> usize { self.buckets.iter().map(|b| b.replacements.len()).sum() }

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

// ── A1: Bucket refresh ────────────────────────────────────────────────────────

impl RoutingTable {
    /// Для каждого непустого bucket генерирует случайный NodeId в диапазоне bucket[i].
    /// Это используется для периодического refresh (A1) — lookup этих ID заполняет
    /// таблицу актуальными контактами и вытесняет мёртвые через PING.
    ///
    /// Алгоритм: берём own_id, флипаем бит на позиции (255 - bucket_index),
    /// остальные биты — случайные. Получаем ID, гарантированно попадающий в бакет i.
    pub fn random_targets_per_bucket(&self, own_id: &NodeId) -> Vec<NodeId> {
        use rand::Rng;
        let mut rng = rand::thread_rng();
        let mut targets = Vec::new();

        for (bucket_idx, bucket) in self.buckets.iter().enumerate() {
            if bucket.len() == 0 { continue; }

            // bit_pos: старший бит XOR-расстояния для bucket[i]
            let bit_pos = 255 - bucket_idx;

            // Начинаем с own_id
            let mut target = own_id.0;

            // Флипаем определяющий бит
            let byte_idx = bit_pos / 8;
            let bit_in_byte = 7 - (bit_pos % 8);
            target[byte_idx] ^= 1 << bit_in_byte;

            // Рандомизируем оставшиеся биты (менее значимые)
            for b in target[byte_idx + 1..].iter_mut() {
                *b = rng.gen();
            }

            targets.push(NodeId(target));
        }

        targets
    }
}
