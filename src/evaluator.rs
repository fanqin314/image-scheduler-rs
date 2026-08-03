//! 价值评估模块
//!
//! 本模块负责根据特征提取器生成的7维特征向量，
//! 计算图片的"价值分数"，并做出算力分配决策。
//! 决策结果决定该图片应由本地处理、云端处理、还是直接丢弃。

use crate::types::{EvaluationResponse, FeatureMap};

/// 评估特征，返回价值分和决策动作
///
/// # 参数
/// - `features`: 特征图，包含7个键值对：
///   - `entropy`: 香农熵 (0~8)，衡量灰度分布随机性
///   - `edge_ratio`: 全局边缘占比 (0~1)，衡量画面内容丰富度
///   - `brightness`: 平均亮度 (0~1)，用于判断过曝/欠曝
///   - `local_peak`: 局部峰值边缘 (0~1)，捕捉画面中最密集的细节区域
///   - `local_variance`: 局部离散度，衡量内容分布的均匀程度
///   - `lower_advantage`: 下半区优势比，车行场景专用（路面/车辆 vs 天空）
///   - `motion`: 帧间运动幅度，衡量动态风险
///
/// # 返回值
/// 返回 `EvaluationResponse`，包含：
/// - `score`: 0~1 之间的价值分数，越高表示越值得投入更多算力
/// - `action`: 决策动作，取值为 `"CLOUD"`、`"LOCAL"`、`"DROP"`
///
/// # 决策阈值
/// - `score >= 0.7` → `CLOUD`（送云端GPU）
/// - `0.4 <= score < 0.7` → `LOCAL`（本地处理）
/// - `score < 0.4` → `DROP`（直接丢弃）
pub fn evaluate(features: &FeatureMap) -> EvaluationResponse {
    // ========== 第一步：定义各特征的权重 ==========
    // 权重总和 = 1.0，反映各特征对最终决策的贡献度
    // 这些权重可根据业务场景调优，例如：
    // - 车行场景可适当提高 local_peak 和 lower_advantage 的权重
    // - 通用场景可均衡分配

    let w_edge = 0.25;       // 全局边缘占比权重
    let w_peak = 0.30;       // 局部峰值边缘权重（最高）
    let w_var = 0.15;        // 局部离散度权重
    let w_lower = 0.20;      // 下半区优势比权重
    let w_motion = 0.10;     // 运动幅度权重

    // ========== 第二步：读取特征值 ==========
    // 从特征图中提取各维度的具体数值
    let edge_ratio = features["edge_ratio"];
    let local_peak = features["local_peak"];
    let local_variance = features["local_variance"];
    let lower_advantage = features["lower_advantage"];
    let motion = features["motion"];

    // ========== 第三步：计算基础分数 ==========
    // 加权求和公式：
    // score = w_edge * edge_ratio
    //       + w_peak * local_peak
    //       + w_var * (local_variance / (local_variance + 1))   ← 归一化到 0~1
    //       + w_lower * (lower_advantage / (lower_advantage + 1)) ← 归一化到 0~1
    //       + w_motion * (motion / 64)                          ← 归一化到 0~1
    //
    // 为什么要归一化？
    // - local_variance 理论上可无限大，用 x/(x+1) 映射到 (0,1)
    // - lower_advantage 同理，避免极端值主导分数
    // - motion 除以 64 做归一化（假设最大运动幅度为64像素/帧）
    let mut score = w_edge * edge_ratio
        + w_peak * local_peak
        + w_var * (local_variance / (local_variance + 1.0))
        + w_lower * (lower_advantage / (lower_advantage + 1.0))
        + w_motion * (motion / 64.0);

    // ========== 第四步：应用亮度惩罚 ==========
    // 如果图片过暗（亮度 < 0.15）或过曝（亮度 > 0.92），
    // 说明图像质量差，无法提取有效信息，价值应大幅降低
    // 这里直接乘以 0.3 的惩罚系数
    let brightness = features["brightness"];
    if brightness < 0.15 || brightness > 0.92 {
        score *= 0.3;  // 亮度惩罚：价值打三折
    }

    // ========== 第五步：裁剪分数到 [0, 1] 区间 ==========
    // 防止浮点误差导致分数超出范围
    let score = score.min(1.0).max(0.0);

    // ========== 第六步：根据分数做出决策 ==========
    // 决策阈值定义了"高价值"和"低价值"的分界线
    // - score >= 0.7：画面内容复杂或有风险，需要云端最强算力
    // - 0.4 <= score < 0.7：内容适中，本地算力可应对
    // - score < 0.4：低价值画面（纯色背景、模糊、噪点），直接丢弃以节省资源
    let action = if score >= 0.7 {
        "CLOUD".to_string()
    } else if score >= 0.4 {
        "LOCAL".to_string()
    } else {
        "DROP".to_string()
    };

    // ========== 第七步：返回评估结果 ==========
    EvaluationResponse { score, action }
}