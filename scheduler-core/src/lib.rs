//! scheduler-core —— 图像调度核心库
//!
//! 把 `image-scheduler-rs` 里**与 HTTP 服务无关**的部分提炼成独立 crate：
//! 特征提取（`features`）、价值评估（`evaluator`）、可调参数（`config`）、
//! 公共数据结构（`types`）。这样调度服务（axum Web 层）、边缘端、基准工具
//! 都能复用同一套打分逻辑，避免"多份代码各写一遍"。
//!
//! 对外最常用的入口是 [`analyze_frame`]：输入一帧的已提取特征图，返回
//! 分数、决策动作与 9 项评分分量明细（`Decision`）。

pub mod config;
pub mod evaluator;
pub mod features;
pub mod types;

pub use evaluator::{EvalError, ScoreBreakdown};
pub use types::FeatureMap;

// ============================================================
// 决策结果类型
// ============================================================

/// 算力分配动作（二分类：送云端 / 本地处理）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// 送云端做完整分析
    Cloud,
    /// 本地轻量处理
    Local,
}

impl Action {
    /// 序列化/展示用字符串（与既有 `EvaluationResponse.action` 保持一致）。
    pub fn as_str(&self) -> &'static str {
        match self {
            Action::Cloud => "CLOUD",
            Action::Local => "LOCAL",
        }
    }
}

/// 一帧的完整决策结果。
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    /// 0~1 价值分数
    pub score: f64,
    /// 决策动作（CLOUD/LOCAL）
    pub action: Action,
    /// 9 项评分分量明细，供诊断/展示观察"每个分量实际取到什么值"。
    pub breakdown: ScoreBreakdown,
}

// ============================================================
// 对外公开 API
// ============================================================

/// 对一帧**已提取**的特征做完整决策。
///
/// - `features`: 由 `features::extract_features`（或视频流的
///   `extract_features_with_motion`）产出的 10 维特征图。
/// - 返回：`Ok(Decision)`，或特征图缺键时的 `Err(EvalError::MissingFeature)`。
///
/// 这是阶段0「单车调度标准化」里约定的 `analyze_frame` 公开接口——
/// 下游（Web 层 / bench 工具）统一走这里拿决策，而不是各自调用
/// `evaluator::evaluate` 再拼装。
pub fn analyze_frame(features: &FeatureMap) -> Result<Decision, EvalError> {
    // evaluate 内部已做一遍 normalize（权威的打分/决策来源）。
    // 这里再 normalize 一次只为取分量明细；normalize 是纯读图运算，开销可忽略。
    // 刻意复用 evaluate 而非手动重算阈值/动作，避免"决策逻辑"两份实现。
    let response = evaluator::evaluate(features)?;
    let breakdown = evaluator::normalize(features)?;
    let action = if response.action == "CLOUD" {
        Action::Cloud
    } else {
        Action::Local
    };
    Ok(Decision {
        score: response.score,
        action,
        breakdown,
    })
}