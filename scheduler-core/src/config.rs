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
/// Sobel 基础阈值（暗画面用）。实际阈值 = base + 亮度×0.3，范围 20~80。
pub const SOBEL_THRESHOLD_BASE: u8 = 15;

/// 滑动窗口边长（像素）。32 保证任何物体至少被 2~3 个窗口覆盖。
pub const WINDOW_SIZE: u32 = 32;

/// 滑动窗口步长（像素）。16 意味着窗口重叠一半，避免物体被网格切开。
pub const WINDOW_STEP: u32 = 16;

/// 下半区分界线（y >= 该值视为下半区）。车行场景：路面/车辆 vs 天空。
pub const LOWER_HALF_OFFSET: u32 = 64;

/// 关注的半区方向。可在运行时通过 /set-region 端点切换。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HalfRegion { Lower, Upper, Left, Right }
pub const DEFAULT_REGION: HalfRegion = HalfRegion::Lower;

/// motion 归一化除数：motion / 该值 后裁剪到 0~1。
///
/// motion 是两帧 128x128 灰度图的逐像素平均绝对差，**理论**上限 255，
/// 但真实视频的相邻帧差值分布远低于此：
///   静止/缓慢 ≈ 1~5，正常行车 ≈ 10~30，急刹/场景切换 ≈ 40~80。
/// 原来取理论上限 64.0，导致正常运动归一化后只有 0.15~0.5，
/// 再乘以 0.04 的权重，对总分贡献不到 0.02 —— 特征形同失效。
/// 这里改为贴合实测分布的 24.0，使典型运动区间落在 0.4~1.0。
///
/// 若希望 motion 真正起到"风险触发器"作用（画面突变时强制抬分），
/// 除了调这个值，还应考虑提高 W_MOTION 或改用超阈值非线性加分。
pub const MOTION_DIVISOR: f64 = 24.0;

/// 轮廓数量归一化上限。128x128 缩略图下 12 个轮廓已非常密集。
pub const CONTOUR_COUNT_MAX: f64 = 12.0;

/// 连通域最小面积（像素）。小于该面积的连通块视为噪点，不计入物体统计。
/// 注意：这是**面积**（像素数），不是轮廓周长。
pub const MIN_OBJECT_AREA: f64 = 5.0;

/// 物体大小差异（变异系数 CV = 标准差 / 均值）的饱和上限。
///
/// 为什么改用 CV 而不是面积方差：面积方差的量级随画面尺度飙升，
/// 128×128 图下可达 10^7 量级，任何 `x/(x+1)` 式的压缩在它面前
/// 都会饱和到 1.0 —— 特征退化成一个常数偏置，丧失全部区分度。
/// CV 是尺度无关的：物体等大时趋近 0，大小悬殊时趋近 1+。
/// 1.5 及以上视为"大小差异极大"，不必再细分。
pub const CONTOUR_CV_MAX: f64 = 1.5;

/// 色相分箱数量。将 0~360° 色相环均分。
/// 12=每30°（多数帧满分，无区分度）→ 已改为 24=每15°（约13%满分，区分度正常）
pub const HUE_BINS: usize = 24;

/// 决策滞后宽度：上一帧为 CLOUD 时，本次需低于 THRESHOLD_CLOUD - HYSTERESIS 才降级。
/// 增大 → 更稳定但更迟钝；减小 → 更灵敏但可能恢复抖动。0.03 是实测最佳值。
pub const HYSTERESIS: f64 = 0.03;

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
/// 价值分 >= 该值 → 送云端 CLOUD；否则 → 本地处理 LOCAL。
/// 0.72→0.65：配合 local_peak diff 评分整体下降约 0.05-0.10 后的调整。
///
/// 当前为**二分类**，不再有 LOCAL/DROP 的第二道阈值：过暗、过亮等低价值帧
/// 由亮度惩罚拉低分数后自然落到 LOCAL 兜底，不会丢失数据。
/// 若日后要恢复三档决策（重新引入 DROP），在此补充 `THRESHOLD_LOCAL`
/// 并在 `evaluator::evaluate` 中加一道分支即可。
pub const THRESHOLD_CLOUD: f64 = 0.65;

/// 是否启用**运行时自适应阈值**。
///
/// 关闭（默认）：一律使用 `THRESHOLD_CLOUD`，行为完全确定、可复现。
/// 开启：`THRESHOLD_CLOUD` 仅作为冷启动兜底，实际阈值由最近若干帧的
/// 分数分布按 `TARGET_CLOUD_RATIO` 实时求解。
///
/// 这是 Reducto (SIGCOMM'20) 思路的简化版——端侧过滤的阈值必须随场景
/// （昼夜/隧道/天气）漂移而调整，固定常数在长时间运行下必然失准。
///
/// 开启前请先用 `bench` 子命令跑一批真实样本，确认目标比例与阈值
/// 的对应关系符合预期。
pub const ADAPTIVE_THRESHOLD: bool = false;

/// 启用自适应阈值时的**目标送云端比例**（0~1）。
///
/// 语义：希望长期有大约该比例的帧被判为 CLOUD。这是**成本旋钮**——
/// 云端调用预算决定它，而不是反过来。
pub const TARGET_CLOUD_RATIO: f64 = 0.20;

/// 自适应阈值维护的分数滑动窗口容量。
/// 窗口越大越稳定但越迟钝；需覆盖至少一个完整的场景周期。
pub const ADAPTIVE_WINDOW: usize = 1000;

/// 自适应阈值冷启动所需的最少样本数。
/// 在此之前回退到 `THRESHOLD_CLOUD`，避免前几帧就剧烈抖动。
pub const ADAPTIVE_MIN_SAMPLES: usize = 50;

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
