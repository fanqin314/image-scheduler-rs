//! 价值评估模块
//!
//! 本模块负责根据特征提取器生成的特征向量，
//! 计算图片的"价值分数"，并做出算力分配决策。
//! 决策结果决定该图片应由本地处理、云端处理、还是直接丢弃。

use crate::config;
use crate::types::{EvaluationResponse, FeatureMap};

// ============================================================
// 归一化层（评分与诊断共用）
// ============================================================

/// 归一化后的 9 项评分分量，取值均在 0~1，可直接加权求和。
///
/// # 为什么要单独抽出这一层
/// 诊断模块需要观察"每个分量实际取到了什么值"，才能判断某个特征是否
/// 已经饱和/失效（例如 `contour_area_variance` 曾长期恒等于 1.0，
/// 占 10% 权重却毫无区分度）。若诊断另写一遍归一化，就会出现
/// "诊断看到的"与"评分真正用的"两张皮，失效特征便永远无法被自动发现。
/// 这里提供唯一的归一化实现，两边共用。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScoreBreakdown {
    pub saliency: f64,
    pub local_peak: f64,
    pub contour_count: f64,
    pub color_richness: f64,
    pub edge_ratio: f64,
    pub lower_advantage: f64,
    pub contour_area_var: f64,
    pub entropy: f64,
    pub motion: f64,
    /// 亮度惩罚系数（0.2~1.0）。不参与加权求和，而是在最后整体相乘。
    pub brightness_penalty: f64,
}

impl ScoreBreakdown {
    /// 返回 9 个评分项：`(名称, 归一化值, 权重, 加权贡献)`
    ///
    /// 顺序与 config.rs 中的权重定义一致，保证加权求和的浮点累加次序
    /// 与重构前完全相同（避免引入数值差异）。
    pub fn terms(&self) -> [(&'static str, f64, f64, f64); 9] {
        [
            (
                "saliency",
                self.saliency,
                config::W_SALIENCY,
                config::W_SALIENCY * self.saliency,
            ),
            (
                "local_peak",
                self.local_peak,
                config::W_LOCAL_PEAK,
                config::W_LOCAL_PEAK * self.local_peak,
            ),
            (
                "contour_count",
                self.contour_count,
                config::W_CONTOUR_COUNT,
                config::W_CONTOUR_COUNT * self.contour_count,
            ),
            (
                "color_richness",
                self.color_richness,
                config::W_COLOR_RICHNESS,
                config::W_COLOR_RICHNESS * self.color_richness,
            ),
            (
                "edge_ratio",
                self.edge_ratio,
                config::W_EDGE_RATIO,
                config::W_EDGE_RATIO * self.edge_ratio,
            ),
            (
                "lower_advantage",
                self.lower_advantage,
                config::W_LOWER_ADVANTAGE,
                config::W_LOWER_ADVANTAGE * self.lower_advantage,
            ),
            (
                "contour_area_var",
                self.contour_area_var,
                config::W_CONTOUR_AREA_VAR,
                config::W_CONTOUR_AREA_VAR * self.contour_area_var,
            ),
            (
                "entropy",
                self.entropy,
                config::W_ENTROPY,
                config::W_ENTROPY * self.entropy,
            ),
            (
                "motion",
                self.motion,
                config::W_MOTION,
                config::W_MOTION * self.motion,
            ),
        ]
    }

    /// 加权求和（**尚未**乘亮度惩罚）
    pub fn weighted_sum(&self) -> f64 {
        let mut sum = 0.0;
        for (_name, _value, _weight, contribution) in self.terms() {
            sum += contribution;
        }
        sum
    }
}

/// 只做归一化，不做决策。`evaluate` 与诊断工具共用此函数。
///
/// 各分量的归一化方式见下方实现，上限类参数集中在 config.rs。
pub fn normalize(features: &FeatureMap) -> ScoreBreakdown {
    // ========== 读取特征值 ==========
    let local_peak_raw = features["local_peak"]; // 原始值（最密集窗口的密度）
    let edge_ratio = features["edge_ratio"]; // 全局边缘占比
    // local_peak 改用差值（raw - edge_ratio）：
    //   高纹理场景 edge~0.8 → 贡献从 1.0 降至 ~0.2，区分度大幅提升
    //   低纹理场景 edge~0.2 → 贡献 ~0.8，仍保持高信号
    let local_peak = (local_peak_raw - edge_ratio).max(0.0);
    let lower_advantage = features["lower_advantage"];
    let motion = features["motion"];
    let entropy = features["entropy"];

    let contour_count = features["contour_count"];
    // 取尺度无关的变异系数，而不是 contour_area_variance（绝对面积方差）。
    // 后者量级可达 10^7，经任何饱和压缩后都恒为 1.0，等于没有这个特征。
    let contour_area_cv = features["contour_area_cv"];
    let color_richness = features["color_richness"];

    // ========== 归一化 ==========
    // 主体-背景对比度：局部峰值密度 / (全局边缘 + 0.05)，上限 SALIENCY_MAX
    // 用原始值而非差值——saliency = 相对突出程度
    let saliency_raw = local_peak_raw / (edge_ratio + 0.05);
    let saliency = (saliency_raw / config::SALIENCY_MAX).min(1.0);

    // 轮廓数量：上限 CONTOUR_COUNT_MAX（该值以下已非常密集）
    let contour_count_norm = (contour_count / config::CONTOUR_COUNT_MAX).min(1.0);

    // 物体大小差异：变异系数（尺度无关）除以饱和上限后裁剪到 (0,1)
    let contour_area_var_norm = (contour_area_cv / config::CONTOUR_CV_MAX).min(1.0);

    // 熵：除以 ENTROPY_MAX (8) 映射到 (0,1)
    let entropy_norm = entropy / config::ENTROPY_MAX;

    // 关注半区优势：用 x/(x+1) 压缩到 (0,1)
    let lower_advantage_norm = lower_advantage / (lower_advantage + 1.0);

    // 运动：除以 MOTION_DIVISOR 映射到 (0,1)
    let motion_norm = (motion / config::MOTION_DIVISOR).min(1.0);

    // ========== 亮度惩罚系数（分段线性）==========
    // brightness 不参与权重，只做惩罚。用分段线性替代一刀切，
    // 保留过渡区间，避免夜间车灯/隧道出口等临界场景被误杀。
    let brightness = features["brightness"];
    let brightness_penalty = if brightness < config::BRIGHTNESS_DARK {
        // 极暗：亮度 0→0.2 线性升至 BRIGHTNESS_DARK(0.10)→0.7
        (brightness / config::BRIGHTNESS_DARK) * 0.5 + 0.2
    } else if brightness > config::BRIGHTNESS_OVER {
        // 过曝：亮度 BRIGHTNESS_OVER(0.95)→0.7 线性降至 1.0→0.2
        ((1.0 - brightness) / (1.0 - config::BRIGHTNESS_OVER)) * 0.5 + 0.2
    } else {
        1.0 // 正常区间无惩罚
    };

    ScoreBreakdown {
        saliency,
        local_peak,
        contour_count: contour_count_norm,
        color_richness,
        edge_ratio,
        lower_advantage: lower_advantage_norm,
        contour_area_var: contour_area_var_norm,
        entropy: entropy_norm,
        motion: motion_norm,
        brightness_penalty,
    }
}

/// 评估特征，返回价值分和决策动作
///
/// # 参数
/// - `features`: 特征图，包含 10 个键值对：
///   - `entropy`: 香农熵 (0~8)
///   - `edge_ratio`: 全局边缘占比 (0~1)
///   - `brightness`: 平均亮度 (0~1) —— 只作惩罚项，不参与加权
///   - `local_peak`: 局部峰值边缘 (0~1)
///   - `local_variance`: 窗口边缘密度方差 —— **目前不参与加权**，仅前端展示
///   - `lower_advantage`: 关注半区优势比
///   - `motion`: 帧间运动幅度
///   - `contour_count`: 连通域（物体）数量
///   - `contour_area_variance`: 连通域面积方差
///   - `color_richness`: 颜色丰富度 (0~1)
///
/// # 返回值
/// 返回 `EvaluationResponse`，包含：
/// - `score`: 0~1 之间的价值分数
/// - `action`: `"CLOUD"` | `"LOCAL"`
///
/// # 决策阈值（详见 config.rs，以代码常量为准）
/// - `score >= THRESHOLD_CLOUD (0.65)` → `CLOUD`
/// - `score <  0.65` → `LOCAL`
///
/// 说明：当前为**二分类**。过暗/过亮等低价值帧由亮度惩罚拉低分数后
/// 自然落到 LOCAL 兜底，因此不再单独产生 DROP（`THRESHOLD_LOCAL` 与
/// `VideoSummary::drop_count` 仅为兼容保留，恒不触发）。
///
/// # 设计原则
/// 权重总和恒为 1.0，全部可调参数集中在 config.rs：
///   - local_peak / saliency: 最高，"局部有主体/主体突出"是最关键的信号
///   - contour_count: 次高，物体数量直接反映场景复杂度
///   - color_richness: 颜色丰富说明场景复杂
///   - lower_advantage: 车行场景特化
///   - motion: 风险触发器，留给视频流
///   - entropy: 基础筛选用
///   - contour_area_variance: 辅助判断物体大小是否多样
///
/// 注：`saliency` 是 `local_peak / (edge_ratio + 0.05)` 派生的相对突出度，
/// 并非独立特征维度。
///
/// # 滞后防抖不在本函数内
/// 视频流的决策滞后由 `handlers::upload_video` 在串行汇总阶段施加
/// （见 config::HYSTERESIS），因为关键帧是 rayon 并行评估的，
/// 并行阶段拿不到"上一帧决策"。本函数是无状态的纯计算。
pub fn evaluate(features: &FeatureMap) -> EvaluationResponse {
    evaluate_with_threshold(features, config::THRESHOLD_CLOUD)
}

/// 用**指定阈值**评估。`evaluate` 是它取 `THRESHOLD_CLOUD` 的特例。
///
/// 把阈值抽成参数的原因：它是调度策略（成本）旋钮，不该硬编码在
/// 评分流程里。自适应阈值（`diagnostics::AdaptiveThreshold`）
/// 与未来按场景分桶的阈值都依赖这个入口。
pub fn evaluate_with_threshold(
    features: &FeatureMap,
    threshold: f64,
) -> EvaluationResponse {
    let breakdown = normalize(features);

    // 加权求和 → 乘亮度惩罚 → 裁剪到 [0, 1]
    let score = (breakdown.weighted_sum() * breakdown.brightness_penalty).clamp(0.0, 1.0);

    // 作出决策（二分类：CLOUD / LOCAL）
    // 去掉 DROP——过暗/过亮帧由亮度惩罚拉低分数，自然归 LOCAL 兜底，
    // 不会丢失数据。滞后防抖仍生效。
    let action = if score >= threshold {
        "CLOUD".to_string()
    } else {
        "LOCAL".to_string()
    };

    EvaluationResponse { score, action }
}
