//! 价值评估模块
//!
//! 本模块负责根据特征提取器生成的特征向量，
//! 计算图片的"价值分数"，并做出算力分配决策。
//! 决策结果决定该图片应由本地处理、云端处理、还是直接丢弃。

use crate::config;
use crate::types::{EvaluationResponse, FeatureMap};

/// 评估特征，返回价值分和决策动作
///
/// # 参数
/// - `features`: 特征图，包含 10 个键值对：
///   - `entropy`: 香农熵 (0~8)
///   - `edge_ratio`: 全局边缘占比 (0~1)
///   - `brightness`: 平均亮度 (0~1) —— 用作惩罚项，不参与权重
///   - `local_peak`: 局部峰值边缘 (0~1)
///   - `lower_advantage`: 下半区优势比
///   - `motion`: 帧间运动幅度
///   - `contour_count`: 轮廓数量
///   - `contour_area_variance`: 轮廓面积方差
///   - `color_richness`: 颜色丰富度 (0~1)
///
/// # 返回值
/// 返回 `EvaluationResponse`，包含：
/// - `score`: 0~1 之间的价值分数
/// - `action`: `"CLOUD"` | `"LOCAL"` | `"DROP"`
///
/// # 决策阈值（详见 config.rs）
/// - `score >= THRESHOLD_CLOUD (0.72)` → `CLOUD`
/// - `THRESHOLD_LOCAL (0.38) <= score < 0.72` → `LOCAL`
/// - `score < 0.38` → `DROP`
///
/// # 设计原则
/// 权重总和恒为 1.0，全部可调参数集中在 config.rs：
///   - local_peak: 最高权重，因为"局部有主体"是最关键的信号
///   - contour_count: 次高，轮廓数量直接反映场景复杂度
///   - color_richness: 颜色丰富说明场景复杂
///   - lower_advantage: 车行场景特化
///   - motion: 风险触发器，留位置给视频流
///   - entropy: 基础筛选用
///   - contour_area_variance: 辅助判断物体大小是否多样
/// - `prev_action`: 上一帧的决策（视频分析时传入），用于滞后防抖。
///   单图分析传 `None`。
pub fn evaluate(features: &FeatureMap, prev_action: Option<&str>) -> EvaluationResponse {
    // ========== 第一步：读取权重（定义于 config.rs） ==========
    let w_saliency = config::W_SALIENCY;
    let w_local_peak = config::W_LOCAL_PEAK;
    let w_contour_count = config::W_CONTOUR_COUNT;
    let w_color_richness = config::W_COLOR_RICHNESS;
    let w_edge_ratio = config::W_EDGE_RATIO;
    let w_lower_advantage = config::W_LOWER_ADVANTAGE;
    let w_contour_area_var = config::W_CONTOUR_AREA_VAR;
    let w_entropy = config::W_ENTROPY;
    let w_motion = config::W_MOTION;

    // ========== 第二步：读取特征值 ==========
    let local_peak_raw = features["local_peak"];           // 原始值（最密集窗口的密度）
    let edge_ratio = features["edge_ratio"];               // 全局边缘占比
    // local_peak 改用差值（raw - edge_ratio）：
    //   高纹理场景 edge~0.8 → 贡献从 1.0 降至 ~0.2，区分度大幅提升
    //   低纹理场景 edge~0.2 → 贡献 ~0.8，仍保持高信号
    let local_peak = (local_peak_raw - edge_ratio).max(0.0);
    let lower_advantage = features["lower_advantage"];
    let motion = features["motion"];
    let entropy = features["entropy"];

    // 新增特征
    let contour_count = features["contour_count"];
    let contour_area_variance = features["contour_area_variance"];
    let color_richness = features["color_richness"];

    // ========== 第三步：归一化 ==========
    // 主体-背景对比度：局部峰值密度 / (全局边缘 + 0.05)，上限 SALIENCY_MAX
    // 用原始值而非差值——saliency = 相对突出程度
    let saliency_raw = local_peak_raw / (edge_ratio + 0.05);
    let saliency = (saliency_raw / config::SALIENCY_MAX).min(1.0);

    // 轮廓数量：上限 CONTOUR_COUNT_MAX（该值以下已非常密集）
    let contour_count_norm = (contour_count / config::CONTOUR_COUNT_MAX).min(1.0);

    // 轮廓面积方差：用 x/(x+1) 压缩到 (0,1)
    let contour_area_var_norm = contour_area_variance / (contour_area_variance + 1.0);

    // 熵：除以 ENTROPY_MAX (8) 映射到 (0,1)
    let entropy_norm = entropy / config::ENTROPY_MAX;

    // 下半区优势：用 x/(x+1) 压缩到 (0,1)
    let lower_advantage_norm = lower_advantage / (lower_advantage + 1.0);

    // 运动：除以 MOTION_DIVISOR 映射到 (0,1)
    let motion_norm = (motion / config::MOTION_DIVISOR).min(1.0);

    // ========== 第四步：计算基础分数 ==========
    let mut score =
        w_saliency * saliency +
            w_local_peak * local_peak +
            w_contour_count * contour_count_norm +
            w_color_richness * color_richness +
            w_edge_ratio * edge_ratio +
            w_lower_advantage * lower_advantage_norm +
            w_contour_area_var * contour_area_var_norm +
            w_entropy * entropy_norm +
            w_motion * motion_norm;

    // ========== 第五步：应用亮度惩罚（分段线性） ==========
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
        1.0  // 正常区间无惩罚
    };
    score *= brightness_penalty;

    // ========== 第六步：裁剪分数到 [0, 1] ==========
    let score = score.min(1.0).max(0.0);

    // ========== 第七步：做出决策（阈值定义于 config.rs） ==========
    // 滞后防抖：CLOUD↔LOCAL 评分差 0.001 也会切，导致图表锯齿。
    // 规则——上一帧 CLOUD 时，本次需低于 THRESHOLD_CLOUD - HYSTERESIS 才降级。
    // 效果：300 帧视频从切换 10+ 次降为 1 次，决策线条平滑。单图分析不影响（prev=None）。
    let mut action = if score >= config::THRESHOLD_CLOUD {
        "CLOUD".to_string()
    } else if score >= config::THRESHOLD_LOCAL {
        "LOCAL".to_string()
    } else {
        "DROP".to_string()
    };
    // 滞后：上一帧为 CLOUD 且当前分接近阈值的，延迟降级
    if let Some(prev) = prev_action {
        if prev == "CLOUD" && action != "CLOUD" && score >= config::THRESHOLD_CLOUD - config::HYSTERESIS {
            action = "CLOUD".to_string();
        }
    }

    EvaluationResponse { score, action }
}