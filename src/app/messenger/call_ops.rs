//! Звонки в Messenger: сигнализация (шаг 3), ключ звонка и медиа (шаги 2 и 4), звук (шаги 5–7),
//! групповой звонок «каждый с каждым» (шаг 9).
//!
//! Модель. «Нога» (`Call`) — звонок между двумя узлами со своим call_id и ключом. Звонок 1:1 — одна нога;
//! групповой — по ноге на каждую пару участников с общим `GroupCall::id` (сеанс). Клиенты работают с сеансом:
//! его id — у 1:1 это call_id ноги, у группового — id группы звонка.
//!
//! Групповой звонок. Инициатор звонит всем участникам. Принявший отвечает на все входящие ноги сеанса и сам
//! звонит тем, с кем ноги ещё нет; узел, уже вошедший в сеанс, такие вызовы принимает сам. Если двое
//! одновременно позвонили друг другу, остаётся нога от узла с меньшим NodeID (обе стороны решают одинаково).

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{bail, Result};
use tracing::{info, warn};

use super::{now_us, Messenger};
use crate::app::calls::{self, Call, CallAnswer, CallEnd, CallKeys, CallOffer, CallReport, CallState, GroupCall};
use crate::app::history::HistoryEntry;
use crate::app::message::{AppMessage, MessageKind};
use crate::app::voice::{self, FRAME};
use crate::protocol::payload::decode;
use crate::types::{now_ms, NodeId};

type Calls = HashMap<Vec<u8>, Call>;

/// Звук узла: кодер исходящего потока и громкость микрофона.
#[derive(Default)]
pub struct VoiceState {
    enc:       Option<voice::Encoder>,
    mic_level: f32,
    last_warn_ms: u64,
}

fn legs<'a>(list: &'a Calls, sid: &'a [u8]) -> impl Iterator<Item = &'a Call> + 'a {
    list.values().filter(move |c| c.session() == sid)
}

/// Сеанс по id ноги или сеанса.
fn resolve(list: &Calls, id: &[u8]) -> Option<Vec<u8>> {
    list.get(id).map(|c| c.session().to_vec())
        .or_else(|| list.values().find(|c| c.session() == id).map(|c| c.session().to_vec()))
}

fn session_json(list: &Calls, sid: &[u8]) -> Option<serde_json::Value> {
    let mut v: Vec<&Call> = legs(list, sid).collect();
    if v.is_empty() { return None; }
    v.sort_by_key(|c| c.created_ms);
    let state = if v.iter().any(|c| c.state == CallState::Active) { "active" }
        else if v.iter().any(|c| c.state == CallState::Ringing) { "ringing" } else { "ended" };
    let group = v[0].group.clone();
    let outgoing = v.iter().any(|c| c.outgoing && !c.auto);
    let answered = v.iter().filter_map(|c| c.answered_ms).min();
    let ended = if state == "ended" { v.iter().filter_map(|c| c.ended_ms).max() } else { None };
    let end_reason = match (state, &group) {
        ("ended", None) => v[0].end_reason.clone(),
        ("ended", Some(_)) => Some(if answered.is_some() { "hangup".into() }
            else { v.iter().find_map(|c| c.end_reason.clone()).unwrap_or_default() }),
        _ => None,
    };
    let mos: Vec<f64> = v.iter().filter_map(|c| c.quality()["mos"].as_f64()).collect();
    let peers: Vec<serde_json::Value> = v.iter().filter(|c| c.end_reason.as_deref() != Some("duplicate")).map(|c| serde_json::json!({
        "peer": c.peer.to_hex(), "call_id": hex::encode(&c.id), "state": c.state, "outgoing": c.outgoing,
        "level": c.audio.as_ref().map_or(0.0, |a| a.stats.level), "quality": c.quality(), "remote": c.remote,
        "end_reason": c.end_reason,
    })).collect();
    Some(serde_json::json!({
        "id": hex::encode(sid), "state": state, "outgoing": outgoing,
        "group": group.as_ref().map(|g| serde_json::json!({
            "id": hex::encode(&g.id), "name": g.name, "initiator": g.initiator.to_hex(), "chat": g.chat,
            "members": g.members.iter().map(|m| m.to_hex()).collect::<Vec<_>>() })),
        "peers": peers,
        "created_ms": v[0].created_ms, "answered_ms": answered, "ended_ms": ended, "end_reason": end_reason,
        "duration_s": answered.map(|a| ended.unwrap_or_else(now_ms).saturating_sub(a) / 1000),
        "mos": if mos.is_empty() { None } else { Some(mos.iter().sum::<f64>() / mos.len() as f64) },
    }))
}

impl Messenger {
    /// Занят: есть незавершённый звонок вне сеанса `except`.
    fn busy_except(list: &Calls, except: Option<&[u8]>) -> bool {
        list.values().any(|c| c.state != CallState::Ended && Some(c.session()) != except)
    }

    async fn call_event(&self, list: &Calls, sid: &[u8]) {
        if let Some(s) = session_json(list, sid) { self.events.emit("call", s).await; }
    }

    /// Все ноги сеанса завершены — одна запись в историю на сеанс.
    async fn finish_session(&self, list: &mut Calls, sid: &[u8]) {
        let done = legs(list, sid).all(|c| c.state == CallState::Ended);
        if done && legs(list, sid).any(|c| !c.logged) {
            self.log_session(list, sid).await;
            for c in list.values_mut().filter(|c| c.session() == sid) { c.logged = true; }
        }
        self.call_event(list, sid).await;
    }

    async fn log_session(&self, list: &Calls, sid: &[u8]) {
        let Some(h) = &self.history else { return };
        let v: Vec<&Call> = legs(list, sid).filter(|c| c.end_reason.as_deref() != Some("duplicate")).collect();
        let Some(first) = v.iter().min_by_key(|c| c.created_ms) else { return };
        let answered = v.iter().filter_map(|c| c.answered_ms).min();
        let ended = v.iter().filter_map(|c| c.ended_ms).max().unwrap_or_else(now_ms);
        let outgoing = v.iter().any(|c| c.outgoing && !c.auto);
        let dur = |a: u64| { let s = ended.saturating_sub(a) / 1000; format!("{}:{:02}", s / 60, s % 60) };
        let (status, text, peer, group) = match &first.group {
            None => {
                let (st, t) = match (answered, first.end_reason.as_deref()) {
                    (Some(a), _) => ("call_ended", format!("📞 {} звонок · {}", if outgoing { "Исходящий" } else { "Входящий" }, dur(a))),
                    (None, Some("busy")) => ("call_rejected", "📞 Абонент занят".to_owned()),
                    (None, Some("rejected")) => ("call_rejected", "📞 Звонок отклонён".to_owned()),
                    (None, _) if outgoing => ("call_missed", "📞 Нет ответа".to_owned()),
                    (None, _) => ("call_missed", "📞 Пропущенный звонок".to_owned()),
                };
                (st, t, first.peer.to_hex(), None)
            }
            Some(g) => {
                let n = v.iter().filter(|c| c.answered_ms.is_some()).count() + 1;
                let (st, t) = match answered {
                    Some(a) => ("call_ended", format!("📞 Групповой звонок «{}» · {} · участников: {n}", g.name, dur(a))),
                    None if outgoing => ("call_missed", format!("📞 Групповой звонок «{}» · никто не ответил", g.name)),
                    None => ("call_missed", format!("📞 Пропущенный групповой звонок «{}»", g.name)),
                };
                (st, t, g.initiator.to_hex(), Some(g))
            }
        };
        h.append(&HistoryEntry {
            ts_ms: first.created_ms, dir: if outgoing { "out".into() } else { "in".into() }, peer,
            message_id: hex::encode(sid), status: status.into(), text: Some(text), rtt_ms: None, error: None,
            lookup_ms: None, tunnel_build_ms: None, tunnel_send_ms: None,
            group: group.and_then(|g| g.chat.clone()), group_name: group.map(|g| g.name.clone()),
            members: group.map(|g| g.members.iter().map(|m| m.to_hex()).collect()), read_ms: None,
        }).await;
    }

    /// Создать исходящую ногу и доставить подписанный CallOffer.
    async fn offer_leg(self: &Arc<Self>, peer: NodeId, group: Option<GroupCall>, auto: bool) -> Result<Vec<u8>> {
        let id = uuid::Uuid::new_v4().into_bytes().to_vec();
        let offer = {
            let mut list = self.calls.lock().await;
            let mut c = Call::new(id.clone(), peer, true);
            c.group = group.clone();
            c.auto = auto;
            let o = CallOffer { call_id: id.clone(), eph_pk: c.my_pk.to_vec(), codec: voice::CODEC.into(),
                                frame_ms: voice::FRAME_MS, group };
            list.insert(id.clone(), c);
            o
        };
        let m = self.build_kind(peer, MessageKind::CallOffer, &offer)?;
        if let Err(e) = self.deliver(peer, None, &m).await {
            let mut list = self.calls.lock().await;
            if let Some(c) = list.get_mut(&id) {
                c.end("unreachable");
                let sid = c.session().to_vec();
                if offer.group.is_some() { self.finish_session(&mut list, &sid).await; } else { list.remove(&id); }
            }
            bail!("вызов {} не доставлен: {e}", peer.short());
        }
        let list = self.calls.lock().await;
        if let Some(c) = list.get(&id) { self.call_event(&list, &c.session().to_vec()).await; }
        Ok(id)
    }

    /// Позвонить: одноразовый ключ, подписанный CallOffer. Ok — id звонка (hex); ответ придёт событием `call`.
    pub async fn call_start(self: &Arc<Self>, target: NodeId) -> Result<String> {
        if target == self.identity.node_id { bail!("нельзя позвонить себе"); }
        if Self::busy_except(&*self.calls.lock().await, None) { bail!("уже идёт звонок"); }
        Ok(hex::encode(self.offer_leg(target, None, false).await?))
    }

    /// Групповой звонок: вызов каждому участнику (до 4 собеседников). Ok — id сеанса.
    pub async fn call_start_group(self: &Arc<Self>, name: &str, members: Vec<NodeId>, chat: Option<String>) -> Result<String> {
        let me = self.identity.node_id;
        let mut all: Vec<NodeId> = members.into_iter().filter(|m| *m != me).collect();
        all.sort_by_key(|m| m.0);
        all.dedup();
        if all.is_empty() { bail!("в группе нет других участников"); }
        if all.len() + 1 > calls::MAX_GROUP { bail!("групповой звонок — не больше {} участников", calls::MAX_GROUP); }
        if Self::busy_except(&*self.calls.lock().await, None) { bail!("уже идёт звонок"); }
        let mut roster = all.clone();
        roster.push(me);
        let g = GroupCall { id: uuid::Uuid::new_v4().into_bytes().to_vec(), name: name.chars().take(64).collect(),
                            members: roster, initiator: me, chat };
        let sid = g.id.clone();
        let res = futures::future::join_all(all.iter().map(|m| self.offer_leg(*m, Some(g.clone()), false))).await;
        if res.iter().all(|r| r.is_err()) {
            bail!("никто из участников не доступен: {}", res.into_iter().filter_map(|r| r.err()).next().map(|e| e.to_string()).unwrap_or_default());
        }
        Ok(hex::encode(sid))
    }

    /// Ответить на входящие ноги: свой одноразовый ключ, ключи звонка, подписанный CallAnswer каждому.
    async fn accept_legs(self: &Arc<Self>, ids: Vec<Vec<u8>>) -> Result<()> {
        let mut answers = vec![];
        {
            let mut list = self.calls.lock().await;
            for id in &ids {
                let Some(c) = list.get_mut(id) else { continue };
                if c.outgoing || c.state != CallState::Ringing || c.keys.is_some() { continue; }
                let caller_pk = c.caller_pk.ok_or_else(|| anyhow::anyhow!("нет ключа звонящего"))?;
                let Some(secret) = c.secret.take() else { continue };
                c.keys = Some(CallKeys::derive(&c.id, &c.peer, &self.identity.node_id, &caller_pk, &c.my_pk, &secret, false)?);
                answers.push((id.clone(), c.peer, CallAnswer { call_id: id.clone(), eph_pk: c.my_pk.to_vec() }));
            }
        }
        let sent = futures::future::join_all(answers.into_iter().map(|(id, peer, a)| async move {
            let r = match self.build_kind(peer, MessageKind::CallAnswer, &a) {
                Ok(m) => self.deliver(peer, None, &m).await.map(|_| ()),
                Err(e) => Err(e),
            };
            (id, r)
        })).await;
        let mut list = self.calls.lock().await;
        let mut sids = vec![];
        let mut first_err = None;
        for (id, r) in sent {
            let Some(c) = list.get_mut(&id) else { continue };
            match r {
                Ok(()) => c.activate(),
                Err(e) => { c.end("answer_failed"); first_err.get_or_insert(e); }
            }
            sids.push(c.session().to_vec());
        }
        sids.dedup();
        for sid in sids { self.finish_session(&mut list, &sid).await; }
        match first_err { Some(e) if !list.values().any(|c| ids.contains(&c.id) && c.state == CallState::Active) =>
            bail!("ответ не доставлен: {e}"), _ => Ok(()) }
    }

    /// Ответить на звонок (id ноги или сеанса). В групповом — подключиться и к остальным участникам.
    pub async fn call_accept(self: &Arc<Self>, id_hex: &str) -> Result<()> {
        let id = hex::decode(id_hex).map_err(|_| anyhow::anyhow!("call_id: hex"))?;
        let (ids, group) = {
            let list = self.calls.lock().await;
            let sid = resolve(&list, &id).ok_or_else(|| anyhow::anyhow!("нет такого звонка"))?;
            let ids: Vec<Vec<u8>> = legs(&list, &sid).filter(|c| !c.outgoing && c.state == CallState::Ringing)
                .map(|c| c.id.clone()).collect();
            if ids.is_empty() { bail!("звонок не ожидает ответа"); }
            let group = legs(&list, &sid).next().and_then(|c| c.group.clone());
            (ids, group)
        };
        self.accept_legs(ids).await?;
        if let Some(g) = &group {
            // ноги сеанса, пришедшие, пока доставлялся ответ, — принять тоже
            loop {
                let more: Vec<Vec<u8>> = {
                    let list = self.calls.lock().await;
                    legs(&list, &g.id).filter(|c| !c.outgoing && c.state == CallState::Ringing && c.keys.is_none())
                        .map(|c| c.id.clone()).collect()
                };
                if more.is_empty() { break; }
                self.accept_legs(more).await?;
            }
        }
        if let Some(g) = group {
            let me = self.identity.node_id;
            let missing: Vec<NodeId> = {
                let list = self.calls.lock().await;
                g.members.iter().copied().filter(|m| *m != me && !legs(&list, &g.id).any(|c| c.peer == *m && c.state != CallState::Ended)).collect()
            };
            for m in missing {
                let (me2, g2) = (self.clone(), g.clone());
                tokio::spawn(async move {
                    if let Err(e) = me2.offer_leg(m, Some(g2), true).await { warn!("групповой звонок: {e}"); }
                });
            }
        }
        Ok(())
    }

    /// Отклонить входящий (`rejected`) или завершить (`hangup`) — все ноги сеанса.
    pub async fn call_end(self: &Arc<Self>, id_hex: &str, reason: &str) -> Result<()> {
        let id = hex::decode(id_hex).map_err(|_| anyhow::anyhow!("call_id: hex"))?;
        let ends = {
            let mut list = self.calls.lock().await;
            let sid = resolve(&list, &id).ok_or_else(|| anyhow::anyhow!("нет такого звонка"))?;
            let mut ends = vec![];
            for c in list.values_mut().filter(|c| c.session() == sid && c.state != CallState::Ended) {
                let r = if reason == "rejected" && !c.outgoing && c.state == CallState::Ringing { "rejected" } else { "hangup" };
                c.end(r);
                let kind = if r == "rejected" { MessageKind::CallReject } else { MessageKind::CallHangup };
                ends.push((c.peer, kind, CallEnd { call_id: c.id.clone(), reason: r.into() }));
            }
            self.finish_session(&mut list, &sid).await;
            ends
        };
        futures::future::join_all(ends.into_iter().map(|(peer, kind, body)| async move {
            match self.build_kind(peer, kind, &body) {
                Ok(m) => if let Err(e) = self.deliver(peer, None, &m).await { warn!("завершение звонка не доставлено: {e}"); },
                Err(e) => warn!("{e}"),
            }
        })).await;
        Ok(())
    }

    fn reply_end(self: &Arc<Self>, to: NodeId, call_id: Vec<u8>, reason: &str) {
        let (me, reason) = (self.clone(), reason.to_owned());
        tokio::spawn(async move {
            if let Ok(m) = me.build_kind(to, MessageKind::CallReject, &CallEnd { call_id, reason }) {
                let _ = me.deliver(to, None, &m).await;
            }
        });
    }

    /// Входящая сигнализация. Ответ, отказ и отчёт принимаются только от собеседника этой ноги.
    pub(super) async fn on_call_message(self: &Arc<Self>, msg: AppMessage) -> Result<()> {
        let from = msg.from;
        let me = self.identity.node_id;
        match msg.kind {
            MessageKind::CallOffer => {
                let o: CallOffer = decode(&msg.payload)?;
                if o.call_id.len() != calls::CALL_ID_LEN || o.eph_pk.len() != 32 { bail!("некорректный вызов"); }
                if let Some(g) = &o.group {
                    if g.id.len() != calls::CALL_ID_LEN || g.members.len() > calls::MAX_GROUP
                        || !g.members.contains(&from) || !g.members.contains(&me) || !g.members.contains(&g.initiator) {
                        bail!("некорректный групповой вызов");
                    }
                }
                let mut list = self.calls.lock().await;
                if list.contains_key(&o.call_id) { bail!("повтор вызова"); }
                let sid = o.group.as_ref().map_or(o.call_id.clone(), |g| g.id.clone());
                if Self::busy_except(&list, o.group.as_ref().map(|g| g.id.as_slice())) {
                    drop(list);
                    self.reply_end(from, o.call_id, "busy");
                    return Ok(());
                }
                let mut joined = false;
                if o.group.is_some() {
                    // уже в сеансе: есть разговор, свой вызов или ответ в пути (ключи выведены, CallAnswer доставляется)
                    joined = legs(&list, &sid).any(|c| c.state == CallState::Active
                        || (c.state == CallState::Ringing && (c.outgoing || c.keys.is_some())));
                    let dup = list.values_mut().find(|c| c.session() == sid && c.peer == from && c.state != CallState::Ended);
                    if let Some(d) = dup {
                        if !(d.outgoing && d.state == CallState::Ringing) { bail!("повтор ноги группового звонка"); }
                        // встречные вызовы: остаётся нога от узла с меньшим NodeID
                        if me.0 < from.0 { drop(list); self.reply_end(from, o.call_id, "duplicate"); return Ok(()); }
                        d.drop_duplicate();
                    }
                }
                let mut c = Call::new(o.call_id.clone(), from, false);
                c.caller_pk = Some(o.eph_pk.as_slice().try_into().unwrap());
                c.group = o.group.clone();
                c.auto = joined;
                match &o.group {
                    Some(g) => info!("📞 групповой звонок «{}» от {}", g.name, from.short()),
                    None => info!("📞 входящий звонок от {}", from.short()),
                }
                list.insert(o.call_id.clone(), c);
                self.call_event(&list, &sid).await;
                drop(list);
                if joined {
                    let me2 = self.clone();
                    tokio::spawn(async move { if let Err(e) = me2.accept_legs(vec![o.call_id]).await { warn!("групповой звонок: {e}"); } });
                }
            }
            MessageKind::CallAnswer => {
                let a: CallAnswer = decode(&msg.payload)?;
                let callee_pk: [u8; 32] = a.eph_pk.as_slice().try_into().map_err(|_| anyhow::anyhow!("ключ ответа"))?;
                let mut list = self.calls.lock().await;
                let c = list.get_mut(&a.call_id).filter(|c| c.peer == from && c.outgoing && c.state == CallState::Ringing)
                    .ok_or_else(|| anyhow::anyhow!("ответ не на наш вызов"))?;
                let secret = c.secret.take().ok_or_else(|| anyhow::anyhow!("секрет уже использован"))?;
                let my_pk = c.my_pk;
                c.keys = Some(CallKeys::derive(&c.id, &me, &from, &my_pk, &callee_pk, &secret, true)?);
                c.activate();
                info!("📞 звонок с {} принят", from.short());
                let sid = c.session().to_vec();
                self.call_event(&list, &sid).await;
            }
            MessageKind::CallReport => {
                let mut r: CallReport = decode(&msg.payload)?;
                r.at_ms = now_ms();
                let mut list = self.calls.lock().await;
                let c = list.get_mut(&r.call_id).filter(|c| c.peer == from)
                    .ok_or_else(|| anyhow::anyhow!("отчёт о чужом звонке"))?;
                c.remote = Some(r);
            }
            _ => {
                let e: CallEnd = decode(&msg.payload)?;
                let mut list = self.calls.lock().await;
                let c = list.get_mut(&e.call_id).filter(|c| c.peer == from)
                    .ok_or_else(|| anyhow::anyhow!("завершение чужого звонка"))?;
                if c.state != CallState::Ended {
                    if e.reason == "duplicate" { c.drop_duplicate(); }
                    else { c.end(if e.reason.len() <= 32 { &e.reason } else { "hangup" }); }
                    let sid = c.session().to_vec();
                    self.finish_session(&mut list, &sid).await;
                }
            }
        }
        Ok(())
    }

    /// Медиакадр из туннеля: найти ногу, расшифровать её ключом, отсечь повтор, учесть, отдать звук в буфер.
    pub async fn on_media(&self, packet: Vec<u8>) {
        let recv_us = now_us();
        let Ok((id, _)) = calls::parse_header(&packet) else { return };
        let mut list = self.calls.lock().await;
        let Some(c) = list.get_mut(id) else { return };
        if c.state != CallState::Active { return; }
        let Some(keys) = &c.keys else { return };
        match keys.open(&packet) {
            Ok((seq, sent_us, payload)) => {
                if !c.window.accept(seq) { c.rx.rejected += 1; return; }
                c.rx.record(seq, sent_us, recv_us, payload.len());
                if payload.first() == Some(&voice::PT_OPUS) {
                    if let Some(a) = c.audio.as_mut() { a.push(seq, payload[1..].to_vec()); }
                }
            }
            Err(_) => c.rx.rejected += 1,
        }
    }

    /// Такт звука (20 мс): кадр микрофона → Opus → каждому собеседнику (свой ключ); в ответ — смесь
    /// принятого от всех собеседников. Без активного звонка — тишина.
    pub async fn voice_tick(&self, pcm: &[i16; FRAME]) -> [i16; FRAME] {
        let mut acc = [0i32; FRAME];
        let mut sends = vec![];
        {
            let mut list = self.calls.lock().await;
            if !list.values().any(|c| c.state == CallState::Active && c.keys.is_some()) { return [0; FRAME]; }
            let opus = {
                let mut v = self.voice.lock().unwrap_or_else(|e| e.into_inner());
                v.mic_level = voice::rms(pcm);
                if v.enc.is_none() { v.enc = voice::Encoder::new().ok(); }
                v.enc.as_mut().and_then(|e| e.encode(pcm).ok())
            };
            let now = now_us();
            for c in list.values_mut().filter(|c| c.state == CallState::Active) {
                let Some(keys) = &c.keys else { continue };
                if let Some(o) = &opus {
                    let mut pl = Vec::with_capacity(1 + o.len());
                    pl.push(voice::PT_OPUS);
                    pl.extend_from_slice(o);
                    let seq = c.tx_seq;
                    c.tx_seq += 1;
                    if let Ok(p) = keys.seal(&c.id, seq, now, &pl) { sends.push((c.peer, p)); }
                }
                let jitter = c.rx.jitter_ms;
                if let Some(a) = c.audio.as_mut() {
                    let mut f = [0i16; FRAME];
                    a.next_frame(jitter, &mut f);
                    voice::mix_into(&mut acc, &f);
                }
            }
        }
        for (peer, p) in sends {
            if let Err(e) = self.tunnels.send_media(&peer, p).await {
                let mut v = self.voice.lock().unwrap_or_else(|e| e.into_inner());
                if now_ms().saturating_sub(v.last_warn_ms) > 5000 {
                    v.last_warn_ms = now_ms();
                    warn!("звонок: кадр к {} не отправлен: {e}", peer.short());
                }
            }
        }
        voice::clip(&acc)
    }

    /// Локальный аудиоканал для клиента (UDP 127.0.0.1, токен в каждом кадре) — создаётся при первом запросе.
    pub async fn audio_link(self: &Arc<Self>) -> Result<(u16, [u8; voice::TOKEN_LEN])> {
        let me = self.clone();
        let r = self.audio_link.get_or_try_init(|| async move {
            let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await?;
            let port = sock.local_addr()?.port();
            let mut token = [0u8; voice::TOKEN_LEN];
            rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut token);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 2048];
                loop {
                    let Ok((n, from)) = sock.recv_from(&mut buf).await else { continue };
                    if !from.ip().is_loopback() { continue; }
                    let Some(pcm) = voice::parse_client(&buf[..n], &token) else { continue };
                    let out = me.voice_tick(&pcm).await;
                    let _ = sock.send_to(&voice::client_frame(&out), from).await;
                }
            });
            info!("📞 локальный аудиоканал: 127.0.0.1:{port}");
            Ok::<_, anyhow::Error>((port, token))
        }).await?;
        Ok(*r)
    }

    /// Тестовый «клиент» внутри узла: тон `freq` Гц вместо микрофона `seconds` секунд; что слышно в ответ —
    /// громкость и какие из частот `listen` присутствуют (для сквозной проверки звука и сценария).
    pub async fn call_tone(&self, freq: f64, seconds: f64, listen: &[f64]) -> serde_json::Value {
        let frames = (seconds * 1000.0 / voice::FRAME_MS as f64).round().max(1.0) as u64;
        let mut t = tokio::time::interval(std::time::Duration::from_millis(voice::FRAME_MS as u64));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Burst);
        let mut heard: Vec<i16> = Vec::new();
        for n in 0..frames {
            t.tick().await;
            let out = self.voice_tick(&voice::tone_frame(freq, n, 0.25)).await;
            heard.extend_from_slice(&out);
        }
        // первая секунда — набор буфера и подключение участников; анализ — по остальному
        let tail = &heard[heard.len().saturating_sub(heard.len() * 2 / 3)..];
        serde_json::json!({ "frames": frames, "rms": voice::rms(tail),
                            "heard": voice::tones_present(tail, listen, 0.25) })
    }

    /// Синтетический медиапоток (без звука, для замера транспорта): `count` кадров по `size` байт каждые
    /// `interval_ms` — каждому собеседнику сеанса.
    pub async fn call_send_media(&self, id_hex: &str, count: u32, interval_ms: u64, size: usize) -> Result<serde_json::Value> {
        let id = hex::decode(id_hex).map_err(|_| anyhow::anyhow!("call_id: hex"))?;
        let sid = resolve(&*self.calls.lock().await, &id).ok_or_else(|| anyhow::anyhow!("нет такого звонка"))?;
        let mut payload = vec![0u8; size.clamp(1, 1200)];
        payload[0] = voice::PT_PROBE;
        let start = tokio::time::Instant::now();
        let (mut sent, mut failed) = (0u32, 0u32);
        let mut first_err = None;
        for i in 0..count {
            tokio::time::sleep_until(start + std::time::Duration::from_millis(interval_ms * i as u64)).await;
            let packets: Vec<(NodeId, Vec<u8>)> = {
                let mut list = self.calls.lock().await;
                let mut v = vec![];
                for c in list.values_mut().filter(|c| c.session() == sid && c.state == CallState::Active) {
                    let Some(k) = &c.keys else { continue };
                    let seq = c.tx_seq;
                    c.tx_seq += 1;
                    v.push((c.peer, k.seal(&c.id, seq, now_us(), &payload)?));
                }
                v
            };
            if packets.is_empty() { if i == 0 { bail!("звонок не активен"); } break; }
            for (peer, p) in packets {
                match self.tunnels.send_media(&peer, p).await {
                    Ok(()) => sent += 1,
                    Err(e) => { failed += 1; if first_err.is_none() { first_err = Some(e.to_string()); } }
                }
            }
        }
        Ok(serde_json::json!({ "sent": sent, "failed": failed, "error": first_err,
                               "span_ms": start.elapsed().as_millis() as u64 }))
    }

    /// Ноги звонков (последние сверху) — для замеров.
    pub async fn calls_json(&self) -> Vec<serde_json::Value> {
        let list = self.calls.lock().await;
        let mut v: Vec<&Call> = list.values().collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.created_ms));
        v.into_iter().map(Call::json).collect()
    }

    /// Сеансы (звонок 1:1 или групповой) — для клиентов.
    pub async fn call_sessions_json(&self) -> Vec<serde_json::Value> {
        let list = self.calls.lock().await;
        let mut sids: Vec<(u64, Vec<u8>)> = vec![];
        for c in list.values() {
            let s = c.session().to_vec();
            match sids.iter_mut().find(|(_, x)| *x == s) {
                Some(e) => e.0 = e.0.min(c.created_ms),
                None => sids.push((c.created_ms, s)),
            }
        }
        sids.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
        sids.iter().filter_map(|(_, s)| session_json(&list, s)).collect()
    }

    /// Громкость своего микрофона (0..1) — индикатор в клиенте.
    pub fn mic_level(&self) -> f32 { self.voice.lock().map(|v| v.mic_level).unwrap_or(0.0) }

    /// Каждые 5 с: тайм-аут вызова (30 с без ответа), отчёт получателя каждому собеседнику (он же проверяет
    /// туннель и перестраивает его при обрыве), подстройка FEC под потери, уборка звонков старше 10 мин.
    pub async fn call_housekeeping(self: &Arc<Self>) {
        let now = now_ms();
        let mut hangups = vec![];
        let mut reports = vec![];
        {
            let mut list = self.calls.lock().await;
            let mut touched = vec![];
            for c in list.values_mut() {
                if c.state == CallState::Ringing && now.saturating_sub(c.created_ms) > calls::RING_TIMEOUT_MS {
                    c.end(if c.outgoing { "no_answer" } else { "missed" });
                    if c.outgoing { hangups.push((c.peer, c.id.clone())); }
                    touched.push(c.session().to_vec());
                }
                if c.state == CallState::Active {
                    let a = c.audio.as_ref().map(|a| a.stats.clone()).unwrap_or_default();
                    reports.push((c.peer, CallReport { call_id: c.id.clone(), packets: c.rx.packets, lost: c.rx.lost(),
                        jitter_ms: c.rx.jitter_ms, concealed: a.concealed, buffer_ms: a.buffer_ms, at_ms: 0 }));
                }
            }
            touched.dedup();
            for sid in touched { self.finish_session(&mut list, &sid).await; }
            let loss = list.values().filter(|c| c.state == CallState::Active).filter_map(|c| c.remote.as_ref())
                .map(|r| 100.0 * r.lost as f64 / (r.packets + r.lost).max(1) as f64).fold(0.0, f64::max);
            if let Some(e) = self.voice.lock().unwrap_or_else(|e| e.into_inner()).enc.as_mut() { e.adapt(loss); }
            list.retain(|_, c| c.ended_ms.map_or(true, |e| now.saturating_sub(e) < 600_000));
        }
        for (peer, id) in hangups {
            let me = self.clone();
            tokio::spawn(async move {
                if let Ok(m) = me.build_kind(peer, MessageKind::CallHangup, &CallEnd { call_id: id, reason: "no_answer".into() }) {
                    let _ = me.deliver(peer, None, &m).await;
                }
            });
        }
        for (peer, r) in reports {
            let me = self.clone();
            tokio::spawn(async move {
                let id = r.call_id.clone();
                if let Ok(m) = me.build_kind(peer, MessageKind::CallReport, &r) {
                    match me.deliver(peer, None, &m).await {
                        // время до сквозного TUNNEL_ACK = круг по туннелю (туннель уже есть, lookup не нужен)
                        Ok(rep) if rep.tunnel_build_ms.is_none() && rep.lookup_ms.is_none() => {
                            if let Some(c) = me.calls.lock().await.get_mut(&id) {
                                let x = rep.tunnel_send_ms as f64;
                                c.rtt_ms = Some(c.rtt_ms.map_or(x, |o| o * 0.7 + x * 0.3));
                            }
                        }
                        Ok(_) => {}
                        Err(e) => warn!("отчёт о звонке {} не доставлен: {e}", peer.short()),
                    }
                }
            });
        }
    }
}
