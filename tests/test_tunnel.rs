//! tests/test_tunnel.rs — Тесты туннеля T6.1–T6.5 (машина состояний).

use p2p_overlay::tunnel::state::TunnelState;
use p2p_overlay::tunnel::hop::TunnelHop;
use p2p_overlay::tunnel::session::TunnelSession;
use p2p_overlay::types::{Contact, NodeId};
use p2p_overlay::config::NodeConfig;
use std::sync::Arc;

fn make_contact(id: u8, port: u16) -> Contact {
    Contact::new(NodeId([id; 32]), vec![id; 32], format!("127.0.0.1:{port}").parse().unwrap())
}

fn make_cfg() -> Arc<NodeConfig> {
    Arc::new(NodeConfig::default())
}

// T6.1: Допустимые переходы состояний
#[test]
fn t6_1_valid_transitions() {
    assert!(TunnelState::Building.can_transition_to(&TunnelState::Active));
    assert!(TunnelState::Building.can_transition_to(&TunnelState::Dead));
    assert!(TunnelState::Active.can_transition_to(&TunnelState::Degraded));
    assert!(TunnelState::Active.can_transition_to(&TunnelState::Closing));
    assert!(TunnelState::Degraded.can_transition_to(&TunnelState::Active));
    assert!(TunnelState::Degraded.can_transition_to(&TunnelState::Dead));
    assert!(TunnelState::Closing.can_transition_to(&TunnelState::Dead));
}

// T6.2: Недопустимые переходы
#[test]
fn t6_2_invalid_transitions() {
    assert!(!TunnelState::Dead.can_transition_to(&TunnelState::Active));
    assert!(!TunnelState::Dead.can_transition_to(&TunnelState::Building));
    assert!(!TunnelState::Active.can_transition_to(&TunnelState::Building));
    assert!(!TunnelState::Closing.can_transition_to(&TunnelState::Active));
}

// T6.3: Состояние is_usable
#[test]
fn t6_3_usable_states() {
    assert!(TunnelState::Active.is_usable());
    assert!(TunnelState::Degraded.is_usable());
    assert!(!TunnelState::Building.is_usable());
    assert!(!TunnelState::Closing.is_usable());
    assert!(!TunnelState::Dead.is_usable());
}

// T6.4: TunnelSession — переход Building→Active→Degraded
#[tokio::test]
async fn t6_4_session_state_machine() {
    let hops = vec![
        TunnelHop::new(make_contact(0x01, 7001), 0),
        TunnelHop::new(make_contact(0x02, 7002), 1),
    ];
    let (session, _rx) = TunnelSession::new(hops, make_cfg());

    assert_eq!(session.state().await, TunnelState::Building);

    let ok = session.transition(TunnelState::Active).await;
    assert!(ok, "Building→Active must be allowed");
    assert_eq!(session.state().await, TunnelState::Active);

    let ok = session.transition(TunnelState::Degraded).await;
    assert!(ok, "Active→Degraded must be allowed");
    assert_eq!(session.state().await, TunnelState::Degraded);
}

// T6.5: Отправка в не-usable туннель — ошибка без паники
#[tokio::test]
async fn t6_5_send_to_building_fails() {
    let hops = vec![TunnelHop::new(make_contact(0x01, 7001), 0)];
    let (session, _rx) = TunnelSession::new(hops, make_cfg());
    // Туннель в состоянии Building — отправка должна вернуть Err
    let result = session.send(b"hello".to_vec()).await;
    assert!(result.is_err(), "Send to Building tunnel must fail");
}

// T6.6: Illegal transition — возвращает false, не паникует
#[tokio::test]
async fn t6_6_illegal_transition_no_panic() {
    let hops = vec![TunnelHop::new(make_contact(0x01, 7001), 0)];
    let (session, _rx) = TunnelSession::new(hops, make_cfg());
    session.transition(TunnelState::Active).await;
    // Dead → Building — недопустимо
    session.transition(TunnelState::Dead).await;
    let ok = session.transition(TunnelState::Building).await;
    assert!(!ok, "Dead→Building must be rejected");
    // Состояние не изменилось — осталось Dead
    assert_eq!(session.state().await, TunnelState::Dead);
}
