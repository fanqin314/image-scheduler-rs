// ============================================================
// config.rs - 全局可调参数中心
//
// 本模块集中管理所有"调参型"魔法数字（特征提取参数、评估权重、
// 决策阈值、视频采样参数）。调参时只需修改本文件，无需深入各算法模块。
//
// 命名规范：全部为大写蛇形 (SCREAMING_SNAKE_CASE) 常量，
//          按所属模块分组，每组配一行说明。
// ============================================================

// ---------- 特征提取参数 (features.rs) ----------
/// 灰度缩略图边长（像素）。整条算法流水线都在此分辨率上运行，
/// 从几百万像素降到 1.6 万像素是"快"的关键。
pub const THUMBNAIL_SIZE: u32 = 128;

/// 颜色特征采样尺寸（像素）。色相统计不需要高分辨率，64x64 足够。
pub const COLOR_SAMPLE_SIZE: u32 = 64;

/// Sobel 边缘判定阈值。梯度幅值 > 该值视为边缘，15 可过滤传感器噪点。
pub const SOBEL_THRESHOLD: u8 = 15;

/// 滑动窗口边长（像素）。32 保证任何物体至少被 2~3 个窗口覆盖。
pub const WINDOW_SIZE: u32 = 32;

/// 滑动窗口步长（像素）。16 意味着窗口重叠一半，避免物体被网格切开。
pub const WINDOW_STEP: u32 = 16;

/// 下半区分界线（y >= 该值视为下半区）。车行场景：路面/车辆 vs 天空。
pub const LOWER_HALF_OFFSET: u32 = 64;

/// motion 归一化除数。两帧 128x128 灰度图逐像素平均绝对差的理论上限。
pub const MOTION_DIVISOR: f64 = 64.0;

/// 轮廓数量归一化上限。128x128 缩略图下 12 个轮廓已非常密集。
pub const CONTOUR_COUNT_MAX: f64 = 12.0;

/// 色相分箱数量。将 0~360° 色相环均分为 12 个 bin。
pub const HUE_BINS: usize = 12;

/// 8 位灰度图的香农熵理论上限（log2(256)）。
pub const ENTROPY_MAX: f64 = 8.0;

/// 亮度惩罚：极暗判定阈值（brightness < 该值视为极暗）。
pub const BRIGHTNESS_DARK: f64 = 0.10;

/// 亮度惩罚：过曝判定阈值（brightness > 该值视为过曝）。
pub const BRIGHTNESS_OVER: f64 = 0.95;

/// 主体-背景对比度（saliency）归一化上限。
pub const SALIENCY_MAX: f64 = 4.0;

// ---------- 价值评估权重 (evaluator.rs) ----------
/// 权重总和恒为 1.0。设计原则：
///   - local_peak 权重最高："局部有主体"是最关键的信号
///   - contour_count / color_richness 反映场景复杂度
///   - lower_advantage 为车行场景特化
///   - motion 为风险触发器，留给视频流
///   - entropy 作基础筛选，权重最低
pub const W_SALIENCY: f64 = 0.15;            // 主体-背景对比度
pub const W_LOCAL_PEAK: f64 = 0.18;          // 局部峰值边缘
pub const W_CONTOUR_COUNT: f64 = 0.15;       // 轮廓数量
pub const W_COLOR_RICHNESS: f64 = 0.12;      // 颜色丰富度
pub const W_EDGE_RATIO: f64 = 0.10;          // 全局边缘占比
pub const W_LOWER_ADVANTAGE: f64 = 0.10;     // 下半区优势比
pub const W_CONTOUR_AREA_VAR: f64 = 0.10;    // 轮廓面积方差
pub const W_ENTROPY: f64 = 0.06;             // 香农熵
pub const W_MOTION: f64 = 0.04;              // 帧间运动幅度

// ---------- 决策阈值 (evaluator.rs) ----------
/// 价值分 >= 该值 → 送云端 CLOUD。
pub const THRESHOLD_CLOUD: f64 = 0.72;

/// 价值分 >= 该值 → 本地处理 LOCAL；低于 LOCAL 阈值 → 丢弃 DROP。
pub const THRESHOLD_LOCAL: f64 = 0.38;

// ---------- 视频分析参数 (handlers.rs / video.rs) ----------
/// 轻量检测分辨率（像素）。先用 64x64 快速筛关键帧，避免全分辨率串行分析。
pub const LIGHT_RES: u32 = 64;

/// 关键帧判定：帧间运动幅度超过该值 → 关键帧。
pub const MOTION_THRESHOLD: f64 = 5.0;

/// 关键帧判定：帧间亮度变化超过该值 → 关键帧（捕捉场景切换/光照突变）。
pub const BRIGHTNESS_CHANGE: f64 = 0.05;

/// 强制关键帧间隔（帧数）。即使画面静止，每隔该帧数也强制分析一次。
pub const FORCE_INTERVAL: usize = 30;

/// 视频最大采样帧数。超过则自动拉大采样间隔，控制解码与分析耗时。
pub const VIDEO_MAX_FRAMES: f64 = 60.0;
