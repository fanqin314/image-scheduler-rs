//! 基于 tokio UDP 的 V2V 广播通信层。
//!
//! [`V2vComm`] 绑定一个 UDP 端口，向一组 peer 地址广播消息，并在后台循环接收：
//! - seq 缺口检测 → 丢包计数（lost）
//! - seq 小于期望 → 区分乱序（首次见到）/ 重复（已见过）
//! - `ts_ms` → 单程延迟 min/max/avg
//! - `last_seen` → 离线检测（[`offline_peers`](V2vComm::offline_peers)）
//! - 畸形消息 → 计数 + 日志，不崩溃

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::net::UdpSocket;

use crate::types::{MessagePayload, MsgType, V2vMessage};

/// 收发统计。
#[derive(Default, Clone, Debug)]
pub struct Stats {
    pub sent: u64,
    pub received: u64,
    /// 检测到的丢包数（seq 缺口累计）
    pub lost: u64,
    pub out_of_order: u64,
    pub duplicated: u64,
    /// 畸形消息数
    pub malformed: u64,
    // 单程延迟（毫秒）
    latency_sum_ms: f64,
    latency_min_ms: f64,
    latency_max_ms: f64,
    latency_count: u64,
}

impl Stats {
    pub fn avg_latency_ms(&self) -> f64 {
        if self.latency_count == 0 {
            0.0
        } else {
            self.latency_sum_ms / self.latency_count as f64
        }
    }
    pub fn min_latency_ms(&self) -> f64 {
        self.latency_min_ms
    }
    pub fn max_latency_ms(&self) -> f64 {
        self.latency_max_ms
    }
    fn record_latency(&mut self, ms: f64) {
        if self.latency_count == 0 {
            self.latency_min_ms = ms;
            self.latency_max_ms = ms;
        } else {
            self.latency_min_ms = self.latency_min_ms.min(ms);
            self.latency_max_ms = self.latency_max_ms.max(ms);
        }
        self.latency_sum_ms += ms;
        self.latency_count += 1;
    }
}

type MsgHandler = Arc<dyn Fn(&V2vMessage) + Send + Sync>;

/// V2V 通信节点。
pub struct V2vComm {
    pub id: crate::types::VehicleId,
    pub listen_port: u16,
    peers: Vec<SocketAddr>,
    socket: Arc<UdpSocket>,
    started: Instant,
    seq: Mutex<u64>,
    /// 已知编队成员的 id（离线检测据此遍历）
    known_peers: Mutex<Vec<crate::types::VehicleId>>,
    /// 每个发送方「期望的下一个 seq」
    expected_seq: Mutex<HashMap<crate::types::VehicleId, u64>>,
    /// 每个发送方最近收到的窗口（用于判断乱序/重复）
    seen: Mutex<HashMap<crate::types::VehicleId, HashSet<u64>>>,
    /// 每个发送方最后一次收到消息的时间
    last_seen: Mutex<HashMap<crate::types::VehicleId, Instant>>,
    /// 每个发送方累计收到的条数
    incoming: Mutex<HashMap<crate::types::VehicleId, u64>>,
    stats: Mutex<Stats>,
    handler: Mutex<Option<MsgHandler>>,
}

impl V2vComm {
    /// 绑定 `0.0.0.0:listen_port`，直接广播到各 `peers`。
    pub async fn bind(
        id: impl Into<crate::types::VehicleId>,
        listen_port: u16,
        peers: Vec<SocketAddr>,
    ) -> anyhow::Result<Arc<Self>> {
        let id = id.into();
        let socket = Arc::new(UdpSocket::bind(("0.0.0.0", listen_port)).await?);
        socket.set_broadcast(true)?;
        let comm = Arc::new(V2vComm {
            id,
            listen_port,
            peers,
            socket,
            started: Instant::now(),
            seq: Mutex::new(0),
            known_peers: Mutex::new(Vec::new()),
            expected_seq: Mutex::new(HashMap::new()),
            seen: Mutex::new(HashMap::new()),
            last_seen: Mutex::new(HashMap::new()),
            incoming: Mutex::new(HashMap::new()),
            stats: Mutex::new(Stats::default()),
            handler: Mutex::new(None),
        });
        let rx = Arc::clone(&comm);
        tokio::spawn(async move {
            rx.recv_loop().await;
        });
        Ok(comm)
    }

    /// 注册收到消息时的回调（用于用户侧展示）。
    pub fn set_handler(&self, f: impl Fn(&V2vMessage) + Send + Sync + 'static) {
        *self.handler.lock().unwrap() = Some(Arc::new(f));
    }

    /// 设置已知编队成员 id（供离线检测遍历）。
    pub fn set_known_peers(&self, ids: Vec<crate::types::VehicleId>) {
        *self.known_peers.lock().unwrap() = ids;
    }

    fn now_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }

    /// 向所有 peer 广播一条消息。
    ///
    /// `n_msg` 用于统计：多条不同业务消息时，`lost`/`received` 按「接收侧期望序号」
    /// 折算。为简单，这里把「发送给每个 peer 的每一条」各计 1 次发送。
    pub async fn broadcast(&self, msg_type: MsgType, payload: MessagePayload) {
        let seq = {
            let mut s = self.seq.lock().unwrap();
            *s += 1;
            *s
        };
        let msg = V2vMessage {
            msg_type,
            sender: self.id.clone(),
            seq,
            ts_ms: Self::now_ms(),
            payload,
        };
        let raw = match serde_json::to_vec(&msg) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("[{}] 序列化失败: {}", self.id, e);
                return;
            }
        };
        let n = self.peers.len();
        for p in &self.peers {
            if let Err(e) = self.socket.send_to(&raw, p).await {
                eprintln!("[{}] 发送到 {p} 失败: {e}", self.id);
            }
        }
        self.stats.lock().unwrap().sent += n as u64;
    }

    /// 便捷：周期发送心跳。
    pub fn start_heartbeat(self: &Arc<Self>, period: Duration) {
        let me = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(period).await;
                me.broadcast(MsgType::Heartbeat, MessagePayload::Heartbeat { nonce: 0 }).await;
            }
        });
    }

    /// 返回编队中超过 `timeout` 未收到消息/心跳的车辆（即离线）。
    pub fn offline_peers(&self, timeout: Duration) -> Vec<(crate::types::VehicleId, Duration)> {
        let now = Instant::now();
        let last_seen = self.last_seen.lock().unwrap();
        let known = self.known_peers.lock().unwrap();
        let mut out = Vec::new();
        for id in known.iter() {
            let age = match last_seen.get(id) {
                Some(t) => now.duration_since(*t),
                // 从未见过：把启动至今视为离线时长（超过超时即判离线）
                None => now.duration_since(self.started),
            };
            if age > timeout {
                out.push((id.clone(), age));
            }
        }
        out
    }

    pub fn last_seen(&self) -> HashMap<crate::types::VehicleId, Instant> {
        self.last_seen.lock().unwrap().clone()
    }

    pub fn incoming_count(&self, sender: &str) -> u64 {
        self.incoming.lock().unwrap().get(sender).copied().unwrap_or(0)
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().unwrap().clone()
    }

    async fn recv_loop(&self) {
        let mut buf = vec![0u8; 65536];
        loop {
            let n = match self.socket.recv(&mut buf).await {
                Ok(n) => n,
                Err(_) => continue,
            };
            let raw = &buf[..n];
            match serde_json::from_slice::<V2vMessage>(raw) {
                Ok(msg) => {
                    self.handle_msg(&msg);
                    if let Some(h) = self.handler.lock().unwrap().clone() {
                        h(&msg);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[{}] 畸形消息丢弃 ({} 字节): {}",
                        self.id,
                        raw.len(),
                        e
                    );
                    self.stats.lock().unwrap().malformed += 1;
                }
            }
        }
    }

    fn handle_msg(&self, msg: &V2vMessage) {
        self.last_seen
            .lock()
            .unwrap()
            .insert(msg.sender.clone(), Instant::now());
        {
            let mut inc = self.incoming.lock().unwrap();
            *inc.entry(msg.sender.clone()).or_default() += 1;
        }
        // 延迟
        {
            let now = Self::now_ms();
            if msg.ts_ms <= now {
                self.stats
                    .lock()
                    .unwrap()
                    .record_latency((now - msg.ts_ms) as f64);
            }
        }
        self.stats.lock().unwrap().received += 1;
        // seq 检测
        self.track_seq(&msg.sender, msg.seq);
    }

    /// 纯函数化的 seq 追踪：暴露给测试直接喂序列使用。
    ///
    /// 规则：`seq == 期望` 正常；`seq > 期望` 记 missing 缺口为 lost；
    /// `seq < 期望` 时若首次见记乱序、否则记重复。
    fn track_seq(&self, sender: &str, seq: u64) {
        let mut exp = self.expected_seq.lock().unwrap();
        let entry = exp.entry(sender.to_string()).or_insert(1);
        let mut stats = self.stats.lock().unwrap();
        if seq == *entry {
            *entry = seq + 1;
        } else if seq > *entry {
            stats.lost += seq - *entry;
            *entry = seq + 1;
        } else {
            let mut seen = self.seen.lock().unwrap();
            let h = seen.entry(sender.to_string()).or_default();
            if h.insert(seq) {
                stats.out_of_order += 1;
            } else {
                stats.duplicated += 1;
            }
            // 裁剪窗口，避免无限增长
            h.retain(|&x| x + 512 > seq);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PerceptionSummary, VehicleState};

    fn v(port: u16) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], port))
    }

    #[tokio::test]
    async fn unicast_latency_below_10ms() {
        let a = V2vComm::bind("car-a", 19010, vec![v(19011)]).await.unwrap();
        let b = V2vComm::bind("car-b", 19011, vec![v(19010)]).await.unwrap();

        // A → B 广播一条状态
        for _ in 0..5 {
            a.broadcast(
                MsgType::State,
                MessagePayload::State(VehicleState {
                    x: 1.0,
                    y: 2.0,
                    speed: 3.0,
                    heading: 90.0,
                    load: 0.5,
                }),
            )
            .await;
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // 轮询等待 B 收到 ≥5 条（上限 2s）
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let s = b.stats();
            if s.received >= 5 || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let s = b.stats();
        assert!(s.received >= 5, "B 应收到 ≥5 条，实际 {}", s.received);
        assert_eq!(b.incoming_count("car-a") >= 5, true);
        assert!(
            s.avg_latency_ms() < 10.0,
            "单程延迟应 <10ms，实际 avg={:.3} max={:.3}",
            s.avg_latency_ms(),
            s.max_latency_ms()
        );
    }

    #[tokio::test]
    async fn three_car_broadcast() {
        // 3 车互为 peer，每车广播 3 条 → 每车应收到来自另外两车各 ≥3 条
        let base = 19100;
        let mut comms = Vec::new();
        for i in 0..3 {
            let peers = (0..3)
                .filter(|&j| j != i)
                .map(|j| v(base + j))
                .collect();
            let c = V2vComm::bind(format!("car-{i}"), base + i, peers)
                .await
                .unwrap();
            comms.push(c);
        }
        for i in 0..3 {
            for _ in 0..3 {
                comms[i]
                    .broadcast(
                        MsgType::Perception,
                        MessagePayload::Perception(PerceptionSummary {
                            objects: (i + 1) as u32,
                            entropy: 3.0,
                            complex_ratio: 0.2,
                        }),
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(120)).await;

        for i in 0..3 {
            let others: Vec<String> = (0..3).filter(|&j| j != i).map(|j| format!("car-{j}")).collect();
            for o in &others {
                assert!(
                    comms[i].incoming_count(o) >= 3,
                    "car-{i} 从 {o} 应收到 ≥3 条，实际 {}",
                    comms[i].incoming_count(o)
                );
            }
        }
    }

    #[tokio::test]
    async fn malformed_message_does_not_crash() {
        let a = V2vComm::bind("car-a", 19200, vec![v(19201)]).await.unwrap();
        let src = Arc::new(UdpSocket::bind(v(0)).await.unwrap());
        // 发一串非法字节
        src.send_to(b"\xff\xfe\x00not-json{{{", v(19200)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let s = a.stats();
        assert!(s.malformed >= 1, "应记录畸形消息，实际 {}", s.malformed);
    }

    #[test]
    fn seq_tracking_detects_loss_ooo_dup() {
        // 用合成序列验证 track_seq 的分类：不依赖网络
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let c = V2vComm::bind("car-x", 19300, vec![]).await.unwrap();
            // 正常 1,2,3
            c.track_seq("car-y", 1);
            c.track_seq("car-y", 2);
            c.track_seq("car-y", 3);
            // 跳到 6 → 丢 4,5
            c.track_seq("car-y", 6);
            // 乱序回 4（首次见）
            c.track_seq("car-y", 4);
            // 重复 4
            c.track_seq("car-y", 4);
            let s = c.stats();
            assert_eq!(s.lost, 2, "应检测丢失 4,5");
            assert_eq!(s.out_of_order, 1, "4 应记乱序");
            assert_eq!(s.duplicated, 1, "重复 4");
        });
    }

    #[tokio::test]
    async fn offline_detection_after_timeout() {
        // A 知道编队里有 car-b，但 car-b 从不向 A 发消息 → 超时后 A 判 car-b 离线
        let a = V2vComm::bind("car-a", 19400, vec![v(19401)]).await.unwrap();
        let _b = V2vComm::bind("car-b", 19401, vec![v(19400)]).await.unwrap();
        a.set_known_peers(vec!["car-b".into()]);
        tokio::time::sleep(Duration::from_millis(120)).await;
        let offline = a.offline_peers(Duration::from_millis(50));
        assert!(
            offline.iter().any(|(id, _)| id == "car-b"),
            "应检测到 car-b 离线，实际 {offline:?}"
        );
    }
}