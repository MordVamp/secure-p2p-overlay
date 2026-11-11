//! Адрес, который узел сообщает пирам (--advertise): подключение других устройств (телефон ↔ ПК).

use p2p_overlay::node::{advertised_host, lan_ip};

#[test]
fn concrete_listen_host_wins() {
    assert_eq!(advertised_host("127.0.1.5", "auto"), "127.0.1.5");
    assert_eq!(advertised_host("192.168.1.10", "10.0.0.1"), "192.168.1.10");
}

#[test]
fn unspecified_listen_uses_advertise_or_loopback() {
    assert_eq!(advertised_host("0.0.0.0", ""), "127.0.0.1");
    assert_eq!(advertised_host("0.0.0.0", "  "), "127.0.0.1");
    assert_eq!(advertised_host("0.0.0.0", "192.168.10.7"), "192.168.10.7");
}

#[test]
fn auto_never_returns_unusable_address() {
    let a = advertised_host("0.0.0.0", "auto");
    let ip: std::net::IpAddr = a.parse().expect("auto → IP-адрес");
    assert!(!ip.is_unspecified());
    if let Some(l) = lan_ip() {
        assert!(!l.is_loopback() && !l.is_unspecified());
        assert_eq!(a, l.to_string());
    }
}
