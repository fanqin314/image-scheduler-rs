//! 车组 V2V 消息格式。
//!
//! 全部消息用 serde JSON 序列化（便于日志与抓包对照）。每个变体字段互不重叠，
//! 解码用 `#[serde(untagged)]` 由形状自动区分，不需显式 tag。

use serde::{Deserialize, Serialize};

/// 车辆标识（小写字母/数字/'-'），便于命令行输入。
pub type VehicleId = String;

/// 消息类别。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgType {
    /// 心跳：维持在线状态，不承载业务数据
    Heartbeat,
    /// 车辆状态：位置/速度/航向/算力负载
    State,
    /// 感知摘要：检测目标数、平均熵、复杂帧占比
    Perception,
    /// 调度决策：得分 / 动作 / 理由
    Decision,
}

impl MsgType {
    pub fn label(&self) -> &'static str {
        match self {
            MsgType::Heartbeat => "心跳",
            MsgType::State => "状态",
            MsgType::Perception => "感知",
            MsgType::Decision => "决策",
        }
    }
}

/// 承载的业务负载（未打 tag，按字段形状自动识别）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessagePayload {
    Heartbeat { nonce: u64 },
    State(VehicleState),
    Perception(PerceptionSummary),
    Decision(SchedulingDecision),
}

/// 车辆状态。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct VehicleState {
    /// 横向位置（模拟坐标，单位省略）
    pub x: f64,
    /// 纵向位置
    pub y: f64,
    /// 速度
    pub speed: f64,
    /// 航向（度，0-360）
    pub heading: f64,
    /// 瞬时算力负载 0..1
    pub load: f64,
}

/// 感知摘要。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PerceptionSummary {
    /// 检测到目标数量
    pub objects: u32,
    /// 平均信息熵（0-8），反映画面复杂度
    pub entropy: f64,
    /// 复杂帧占比 0..1
    pub complex_ratio: f64,
}

/// 调度决策。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulingDecision {
    /// 决策目标车辆
    pub target: VehicleId,
    /// 处理评分 0..1，越高越倾向云端
    pub score: f64,
    /// 动作，如 `local` / `cloud` / `delegate`
    pub action: String,
    /// 理由
    pub reason: String,
}

/// 单条 V2V 消息。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct V2vMessage {
    pub msg_type: MsgType,
    pub sender: VehicleId,
    /// 发送方单调递增序号，用于丢包/乱序/重复检测
    pub seq: u64,
    /// 毫秒级时间戳（epoch millis），用于单程延迟测量
    pub ts_ms: u64,
    pub payload: MessagePayload,
}

impl V2vMessage {
    /// 构造心跳。
    pub fn heartbeat(sender: &str, seq: u64, ts_ms: u64) -> Self {
        V2vMessage {
            msg_type: MsgType::Heartbeat,
            sender: sender.into(),
            seq,
            ts_ms,
            payload: MessagePayload::Heartbeat { nonce: seq ^ ts_ms },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_roundtrip() {
        let msg = V2vMessage {
            msg_type: MsgType::State,
            sender: "car-a".into(),
            seq: 3,
            ts_ms: 123456,
            payload: MessagePayload::State(VehicleState {
                x: 1.0,
                y: 2.0,
                speed: 3.5,
                heading: 90.0,
                load: 0.4,
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let back: V2vMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.sender, "car-a");
        assert_eq!(back.seq, 3);
        match back.payload {
            MessagePayload::State(s) => {
                assert!((s.x - 1.0).abs() < 1e-9);
                assert!((s.load - 0.4).abs() < 1e-9);
            }
            _ => panic!("应解析为 State"),
        }
    }

    #[test]
    fn heartbeat_reuse() {
        let msg = V2vMessage::heartbeat("car-b", 1, 99);
        let json = serde_json::to_string(&msg).unwrap();
        let back: V2vMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.msg_type, MsgType::Heartbeat);
        assert!(matches!(back.payload, MessagePayload::Heartbeat { nonce: _ }));
    }
}