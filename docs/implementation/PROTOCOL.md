# Спецификация протокола

## Формат кадра
Заголовок: 24 байта, big-endian.

```
Offset  Size  Field           Description
0       1     version         Версия протокола (= 1)
1       1     msg_type        Тип сообщения (таблица ниже)
2       2     flags           Битовые флаги (IS_RESPONSE=0x1, IS_ERROR=0x2, MORE_FRAGS=0x4)
4       16    request_id      UUID v4 — сопоставление запрос/ответ, дедупликация
20      4     payload_length  Длина payload в байтах (0..65536), BE
24      var   payload         MessagePack-сериализованные данные
```

`payload_length` проверяется до выделения буфера. При превышении MAX_FRAME_PAYLOAD=65536 → ERROR-кадр.

## Таблица типов сообщений

| Hex  | Константа             | Payload-структура |
|------|-----------------------|-------------------|
| 0x01 | PING                  | `{nonce: u64}` |
| 0x02 | PONG                  | `{nonce: u64}` |
| 0x03 | FIND_NODE_REQUEST     | `{target: [u8;32], alpha: u8}` |
| 0x04 | FIND_NODE_RESPONSE    | `{contacts: [Contact]}` |
| 0x05 | STORE_REQUEST         | `{key: [u8;32], record: NodeRecord}` |
| 0x06 | STORE_RESPONSE        | `{ok: bool, reason: Option<str>}` |
| 0x07 | FIND_VALUE_REQUEST    | `{key: [u8;32]}` |
| 0x08 | FIND_VALUE_RESPONSE   | `{value: Option<NodeRecord>, contacts: [Contact]}` |
| 0x10 | TUNNEL_BUILD          | `{tunnel_id: [u8;16], route: [NodeId], ttl: u32}` |
| 0x11 | TUNNEL_BUILD_OK       | `{tunnel_id: [u8;16]}` |
| 0x12 | TUNNEL_BUILD_FAIL     | `{tunnel_id: [u8;16], reason: str}` |
| 0x13 | TUNNEL_DATA           | `{tunnel_id: [u8;16], seq: u64, ciphertext: bytes}` |
| 0x14 | TUNNEL_ACK            | `{tunnel_id: [u8;16], seq: u64}` |
| 0x15 | TUNNEL_CLOSE          | `{tunnel_id: [u8;16]}` |
| 0x20 | APP_MESSAGE           | `{message_id: [u8;16], to: NodeId, ciphertext: bytes}` |
| 0x21 | APP_ACK               | `{message_id: [u8;16], ok: bool}` |
| 0xFF | ERROR                 | `{code: u16, description: str}` |

## Ключ DHT-записи
```
key = SHA-256("node:" ‖ node_id)   // упрощённый уровень
// продвинутый: SHA-256("alias:" ‖ normalize(alias))
```


## Примеры payload (§9 ТЗ)

Все значения сериализуются через MessagePack. Ниже показаны логические структуры.

### PING
```json
{
  "sender": {
    "node_id": "a1b2c3...32bytes",
    "identity_algorithm": "ed25519",
    "identity_public_key": "d4e5f6...32bytes",
    "host": "127.0.0.1",
    "port": 7001,
    "last_seen_ms": 1727430000000,
    "last_verified_ms": 0
  },
  "timestamp_ms": 1727430000123
}
```

### PONG
```json
{
  "responder": { "node_id": "...", "host": "127.0.0.1", "port": 7002, "..." },
  "ping_timestamp_ms": 1727430000123,
  "responder_timestamp_ms": 1727430000145
}
```

### FIND_NODE_REQUEST
```json
{
  "sender": { "node_id": "...", "host": "127.0.0.1", "port": 7001, "..." },
  "target_node_id": "ff00aa11...32bytes"
}
```

### FIND_NODE_RESPONSE
```json
{
  "responder": { "node_id": "...", "host": "127.0.0.1", "port": 7002, "..." },
  "target_node_id": "ff00aa11...32bytes",
  "contacts": [
    { "node_id": "fe10bb22...", "host": "127.0.0.1", "port": 7003, "..." },
    { "node_id": "fd20cc33...", "host": "127.0.0.1", "port": 7004, "..." }
  ]
}
```
*Контакты отсортированы по возрастанию XOR-расстояния до `target_node_id`. Количество ≤ K_BUCKET_SIZE.*

### ERROR
```json
{
  "code": "BAD_VERSION",
  "description": "unsupported protocol version: 42"
}
```
*Код ошибки не содержит стека вызовов, путей к файлам или секретов (§10 ТЗ).*

---

## Handshake-транскрипт (продвинутый уровень — TODO)
```
transcript = version(1B) ‖ role_initiator(1B) ‖ role_responder(1B)
           ‖ NodeID_A(32B) ‖ NodeID_B(32B)
           ‖ IdentityPK_A(32B) ‖ IdentityPK_B(32B)
           ‖ EphemeralPK_A(32B) ‖ EphemeralPK_B(32B)
           ‖ nonce_A(32B) ‖ nonce_B(32B)
           ‖ session_id(16B)
```
