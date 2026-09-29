//! # v2v — 车组 V2V 通信层模拟
//!
//! 阶段1 交付物：
//! - [`types`]：消息格式（车辆状态、感知摘要、调度决策、心跳）
//! - [`comm`]：基于 tokio UDP 的广播通信层（seq 丢包/乱序/重复检测、心跳、
//!   离线检测、收发统计）
//! - 二进制 `vehicle`：单进程模拟一辆车（`--id --listen-port --peers`）
//! - 二进制 `simulator`：编排器，派生 N 个 vehicle 进程做联调并输出丢包率报告
//!
//! 设计上与 `scheduler-core` 解耦，只依赖 tokio + serde，便于独立复用。

pub mod comm;
pub mod types;

pub use comm::{Stats, V2vComm};
pub use types::{MessagePayload, MsgType, PerceptionSummary, SchedulingDecision, VehicleState};