// ============================================================
// 模块：特征提取 (features)
// 职责：将图像像素数据转换为 10 维数值特征向量
// 核心算法：香农熵、Sobel 边缘检测、滑动窗口统计、垂直分区分析、轮廓统计、HSV 色相统计
// 可调参数均集中在 config.rs，调参请改那里。
// ============================================================

use image::{DynamicImage, GrayImage, Luma};
use crate::config;
use crate::types::FeatureMap;

// ============================================================
// 香农熵 (Shannon Entropy)
// ============================================================
/// 计算灰度图的香农熵，衡量灰度分布的"随机性"或"信息量"
///
/// # 参数
/// - `gray`: 灰度图 (8位，0~255)
///
/// # 返回值
/// - 熵值，范围 0~8 (对于8位图像)
///   - 0: 所有像素灰度值相同 (纯色)
///   - 8: 所有灰度值均匀分布 (理想随机)
///
/// # 算法原理
/// H = -Σ p(i) * log2(p(i))
/// 其中 p(i) = 灰度值 i 出现的概率
pub fn shannon_entropy(gray: &GrayImage) -> f64 {
    // 1. 初始化 256 个桶，统计每个灰度值出现的次数
    let mut hist = [0u64; 256];
    for pixel in gray.pixels() {
        // pixel[0] 是灰度值 (0~255)
        hist[pixel[0] as usize] += 1;
    }

    // 2. 总像素数 (转为 f64 以便除法)
    let total = gray.pixels().len() as f64;
    let mut entropy = 0.0;

    // 3. 遍历每个灰度值，计算概率并累加熵
    for &count in hist.iter() {
        if count > 0 {
            let p = count as f64 / total;   // 概率 p(i)
            entropy -= p * p.log2();        // -p * log2(p)
        }
    }

    entropy
}

// ============================================================
// Sobel 边缘检测
// ============================================================
/// 对灰度图进行 Sobel 边缘检测，返回边缘图和边缘像素占比
///
/// # 参数
/// - `gray`: 灰度图
/// - `threshold`: 边缘判定阈值 (梯度 > threshold 视为边缘)
///   推荐值: config::SOBEL_THRESHOLD (15)，可过滤掉传感器噪点
///
/// # 返回值
/// - `(edge_map, edge_ratio)`
///   - `edge_map`: 边缘强度图 (每个像素值代表梯度幅值)
///   - `edge_ratio`: 边缘像素占比 = 边缘像素数 / 总像素数 (0~1)
///
/// # 算法原理
/// 1. 用 Sobel 算子计算 X 和 Y 方向的梯度
/// 2. 梯度幅值 ≈ |Gx| + |Gy| (近似值，比开平方快)
/// 3. 幅值 > threshold 的像素标记为"边缘"
///
/// # Sobel 算子 (3x3 卷积核)
/// Gx (水平方向):      Gy (垂直方向):
/// [-1, 0, 1]          [-1, -2, -1]
/// [-2, 0, 2]          [ 0,  0,  0]
/// [-1, 0, 1]          [ 1,  2,  1]
pub fn sobel_edge_detection(gray: &GrayImage, threshold: u8) -> (GrayImage, f64) {
    // 定义 Sobel 卷积核 (3x3)
    let sobel_x: [[i32; 3]; 3] = [
        [-1, 0, 1],
        [-2, 0, 2],
        [-1, 0, 1]
    ];
    let sobel_y: [[i32; 3]; 3] = [
        [-1, -2, -1],
        [ 0,  0,  0],
        [ 1,  2,  1]
    ];

    let (width, height) = gray.dimensions();

    // 边缘图：存储每个像素的梯度幅值 (0~255)
    let mut grad_magnitude = GrayImage::new(width, height);

    // 统计边缘像素数
    let mut sum_edge = 0u64;
    let total_pixels = (width as u64) * (height as u64);

    // 遍历每个像素（跳过边界 1 像素，因为 3x3 卷积需要邻域）
    for y in 1..(height - 1) {
        for x in 1..(width - 1) {
            let mut gx = 0i32;
            let mut gy = 0i32;

            // 提取 3x3 邻域，与 Sobel 核做卷积
            for dy in -1..=1 {
                for dx in -1..=1 {
                    // 获取邻域像素的灰度值 (转为 i32 以便做乘法)
                    let px = gray
                        .get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0]
                        as i32;

                    // 累加 X 方向梯度
                    gx += px * sobel_x[(dy + 1) as usize][(dx + 1) as usize];
                    // 累加 Y 方向梯度
                    gy += px * sobel_y[(dy + 1) as usize][(dx + 1) as usize];
                }
            }

            // 梯度幅值 = |Gx| + |Gy| (曼哈顿距离，比欧氏距离快)
            // 也可以用 sqrt(Gx² + Gy²)，但更慢
            let mag = (gx.abs() + gy.abs()) as u8;

            // 存入边缘图
            grad_magnitude.put_pixel(x, y, Luma([mag]));

            // 统计边缘像素 (梯度 > threshold)
            if mag > threshold {
                sum_edge += 1;
            }
        }
    }

    // 计算边缘像素占比
    let edge_ratio = sum_edge as f64 / total_pixels as f64;

    (grad_magnitude, edge_ratio)
}

// ============================================================
// 滑动窗口特征
// ============================================================
/// 在边缘图上用滑动窗口扫描，计算"局部峰值"和"局部离散度"
///
/// # 参数
/// - `edge_map`: 边缘图 (来自 sobel_edge_detection)
/// - `win_size`: 窗口大小 (像素)
/// - `step`: 滑动步长 (像素)
///
/// # 返回值
/// - `(local_peak, local_variance)`
///   - `local_peak`: 所有窗口中边缘密度的最大值 (0~1)
///     表示"画面中最聚焦的区域有多丰富"
///   - `local_variance`: 所有窗口边缘密度的方差
///     表示"画面内容的均匀程度"
///     高方差 = 有疏有密 (有主体有背景)
///     低方差 = 均匀 (全是噪点或全是平滑区域)
///
/// # 物理意义
/// 固定网格会把物体切成碎片，滑动窗口保证物体总被某个窗口完整框住。
/// 窗口大小 32，步长 16 意味着窗口重叠一半，任何物体至少被 2~3 个窗口覆盖。
pub fn sliding_window_features(edge_map: &GrayImage, win_size: u32, step: u32) -> (f64, f64) {
    let (w, h) = edge_map.dimensions();
    let mut ratios = Vec::new();

    // 用 while 循环，步长可配置
    let mut y = 0;
    while y + win_size <= h {
        let mut x = 0;
        while x + win_size <= w {
            // 统计当前窗口内的边缘像素数
            let mut count = 0u64;
            for i in 0..win_size {
                for j in 0..win_size {
                    // edge_map 中 > 0 表示该像素是边缘
                    if edge_map.get_pixel(x + i, y + j)[0] > 0 {
                        count += 1;
                    }
                }
            }
            // 计算边缘密度 = 边缘像素数 / 窗口面积
            ratios.push(count as f64 / (win_size * win_size) as f64);
            x += step;
        }
        y += step;
    }

    // 如果窗口数为 0 (图片太小)，返回默认值
    if ratios.is_empty() {
        return (0.0, 0.0);
    }

    // 局部峰值 = 所有窗口边缘密度的最大值
    // 用 fold 代替 max() 避免浮点数类型歧义
    let local_peak = ratios.iter().fold(0.0_f64, |a, &b| a.max(b));

    // 计算方差
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let var = ratios
        .iter()
        .map(|v| (v - mean).powi(2))
        .sum::<f64>()
        / ratios.len() as f64;

    (local_peak, var)
}

// ============================================================
// 完整特征提取（主入口）
// ============================================================
/// 从原始图像中提取完整的 10 维特征向量
///
/// # 参数
/// - `img`: 动态图像 (支持任何格式，RGB/RGBA/灰度等)
///
/// # 返回值
/// - `FeatureMap`: 特征名 → 特征值的 HashMap
///
/// # 10 维特征定义
/// | 特征名 | 取值范围 | 含义 |
/// |--------|---------|------|
/// | entropy | 0~8 | 灰度分布随机性 (纯色→0, 随机→8) |
/// | edge_ratio | 0~1 | 全局边缘像素占比 (画面内容丰富度) |
/// | brightness | 0~1 | 平均亮度 (0=纯黑, 1=纯白) |
/// | local_peak | 0~1 | 最密集窗口的边缘密度 (捕捉局部主体) |
/// | local_variance | 0~∞ | 窗口边缘密度的方差 (疏密对比度) |
/// | lower_advantage | 0~∞ | 下半区边缘 / 上半区边缘 (车行场景: 路面/车辆 vs 天空) |
/// | motion | 0~∞ | 帧间运动幅度 (单图为0，视频流时填充) |
/// | contour_count | 0~∞ | 边缘轮廓数量 (场景复杂度) |
/// | contour_area_variance | 0~∞ | 轮廓面积方差 (物体大小是否多样) |
/// | color_richness | 0~1 | HSV 色相覆盖广度 (颜色多样性) |
///
/// 单图特征提取入口（向后兼容）：motion 固定为 0.0。
pub fn extract_features(img: &DynamicImage) -> FeatureMap {
    extract_features_with_motion(img, None)
}

/// 完整特征提取，支持帧间 motion 计算
///
/// # 参数
/// - `img`: 当前帧图像
/// - `prev_gray`: 上一帧的灰度图（THUMBNAIL_SIZE x THUMBNAIL_SIZE），用于计算 motion。
///   单图模式传 `None`，motion 固定为 0.0。
pub fn extract_features_with_motion(
    img: &DynamicImage,
    prev_gray: Option<&GrayImage>,
) -> FeatureMap {
    // ===== 第 1 步：预处理 =====
    // 转灰度 (丢弃颜色信息，只保留亮度)
    let gray = img.to_luma8();

    // 缩放到 THUMBNAIL_SIZE x THUMBNAIL_SIZE (最近邻插值，最快)
    // 这是整个算法"快"的关键：从几百万像素降到约 1.6 万像素
    let n = config::THUMBNAIL_SIZE;
    let small = image::imageops::resize(
        &gray,
        n,
        n,
        image::imageops::FilterType::Nearest,
    );

    // ===== 第 2 步：计算各特征 =====

    // 特征 1: 香农熵
    let entropy = shannon_entropy(&small);

    // 特征 2 & 3: 边缘占比 (Sobel 检测)
    let (edge_map, edge_ratio) = sobel_edge_detection(&small, config::SOBEL_THRESHOLD);

    // 特征 4: 平均亮度 (归一化到 0~1)
    let total_px = (n * n) as f64;
    let brightness = small
        .pixels()
        .map(|p| p[0] as f64)
        .sum::<f64>()
        / total_px
        / 255.0;

    // 特征 5 & 6: 滑动窗口特征 (局部峰值 + 方差)
    let (local_peak, local_variance) =
        sliding_window_features(&edge_map, config::WINDOW_SIZE, config::WINDOW_STEP);

    // 特征 7: 下半区优势比 (车行场景专用)
    // 下半区 (y >= LOWER_HALF_OFFSET) vs 上半区 (y < LOWER_HALF_OFFSET)
    let half = config::LOWER_HALF_OFFSET;
    let mut upper = 0u64;
    let mut lower = 0u64;
    for y in 0..n {
        for x in 0..n {
            if edge_map.get_pixel(x, y)[0] > 0 {
                if y < half {
                    upper += 1;
                } else {
                    lower += 1;
                }
            }
        }
    }
    // +1 防止除零 (如果上半区完全没有边缘)
    let lower_advantage = lower as f64 / (upper as f64 + 1.0);

    // 特征 8: 帧间运动幅度
    let motion = match prev_gray {
        Some(prev) => {
            // 计算两帧缩略图的逐像素平均绝对差
            let mut sum_diff = 0u64;
            for (a, b) in prev.pixels().zip(small.pixels()) {
                let diff = (a[0] as i32 - b[0] as i32).unsigned_abs() as u64;
                sum_diff += diff;
            }
            sum_diff as f64 / total_px
        }
        None => 0.0,
    };

    // ===== 特征 9: 轮廓统计 =====
    // 用轮廓数量 + 面积方差刻画"场景里有几个物体、大小是否悬殊"
    let (contour_count, _contour_area_mean, contour_area_variance) = extract_contour_stats(&edge_map);

    // ===== 特征 10: 颜色丰富度 =====
    // 注意：这里传入的是原始图像 img，不是缩略图 small（颜色特征需要 RGB 信息）
    let color_richness = compute_color_richness(img);

    // ===== 第 3 步：组装成 HashMap =====
    let mut map = FeatureMap::new();
    map.insert("entropy".to_string(), entropy);
    map.insert("edge_ratio".to_string(), edge_ratio);
    map.insert("brightness".to_string(), brightness);
    map.insert("local_peak".to_string(), local_peak);
    map.insert("local_variance".to_string(), local_variance);
    map.insert("lower_advantage".to_string(), lower_advantage);
    map.insert("motion".to_string(), motion);
    map.insert("contour_count".to_string(), contour_count as f64);
    map.insert("contour_area_variance".to_string(), contour_area_variance);
    map.insert("color_richness".to_string(), color_richness);

    map
}

// ============================================================
// 获取边缘图 (供可视化模块使用)
// ============================================================
/// 从原始图像中提取边缘图，用于生成可视化标注
///
/// # 参数
/// - `img`: 动态图像
///
/// # 返回值
/// - `GrayImage`: THUMBNAIL_SIZE x THUMBNAIL_SIZE 的边缘强度图
///
/// # 用途
/// visualization::generate_visualization() 需要边缘图来绘制网格和高亮框
pub fn get_edge_map(img: &DynamicImage) -> GrayImage {
    let gray = img.to_luma8();
    let n = config::THUMBNAIL_SIZE;
    let small = image::imageops::resize(
        &gray,
        n,
        n,
        image::imageops::FilterType::Nearest,
    );
    let (edge_map, _) = sobel_edge_detection(&small, config::SOBEL_THRESHOLD);
    edge_map
}

// ============================================================
// 轮廓统计与颜色丰富度（特征 9、10 的计算函数）
// ============================================================

/// 提取轮廓统计（浅层：连通域分析）
///
/// 在 Sobel 边缘图上做二值化 → 连通域标记，返回三个指标：
/// - `轮廓数量`：(usize) 独立连通域个数，反映画面中"物体"的多少
/// - `面积均值`：(f64) 轮廓的平均像素数，反映物体大小的平均水平
/// - `面积方差`：(f64) 轮廓面积的方差，大=物体大小差异大（前景/背景分明），小=均匀纹理
pub fn extract_contour_stats(edge_map: &GrayImage) -> (usize, f64, f64) {
    // 1. 将边缘图转为二值图（0 或 255）
    let (w, h) = edge_map.dimensions();
    let mut binary = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = edge_map.get_pixel(x, y)[0];
            binary.put_pixel(x, y, Luma([if v > 0 { 255u8 } else { 0u8 }]));
        }
    }

    // 2. 提取轮廓
    use imageproc::contours::find_contours;

    let contours = find_contours(&binary);
    let count = contours.len();

    if count == 0 {
        return (0, 0.0, 0.0);
    }

    // 3. 计算每个轮廓的面积（用边界框面积近似）
    let total_pixels = (w * h) as f64;
    let mut areas: Vec<f64> = Vec::new();

    for contour in &contours {
        // 修正点1：使用 contour.points.is_empty()
        if contour.points.is_empty() {
            continue;
        }
        // 修正点2：使用 contour.points.iter()，Point 有 x 和 y 字段
        let min_x = contour.points.iter().map(|p| p.x).min().unwrap_or(0);
        let max_x = contour.points.iter().map(|p| p.x).max().unwrap_or(0);
        let min_y = contour.points.iter().map(|p| p.y).min().unwrap_or(0);
        let max_y = contour.points.iter().map(|p| p.y).max().unwrap_or(0);
        let bbox_area = ((max_x - min_x + 1) * (max_y - min_y + 1)) as f64;
        areas.push(bbox_area / total_pixels);
    }

    // 如果所有轮廓都被跳过了（理论上不会发生），返回 0
    if areas.is_empty() {
        return (0, 0.0, 0.0);
    }

    let area_mean = areas.iter().sum::<f64>() / areas.len() as f64;
    let area_var = areas.iter()
        .map(|a| (a - area_mean).powi(2))
        .sum::<f64>() / areas.len() as f64;

    (count, area_mean, area_var)
}

/// 计算颜色丰富度（HSV 色相覆盖广度）
/// 将色相 0~360° 分成 config::HUE_BINS 个 bin，统计覆盖了多少个 bin
/// 返回: 0.0 ~ 1.0
pub fn compute_color_richness(img: &DynamicImage) -> f64 {
    // 缩放到 COLOR_SAMPLE_SIZE 就够了（颜色特征不需要高分辨率）
    let c = config::COLOR_SAMPLE_SIZE;
    let small = img.resize(c, c, image::imageops::FilterType::Nearest);
    let rgb = small.to_rgb8();

    // 每个 bin 覆盖的色相角度 = 360° / bin 数量
    // 注意：hue 在下方 RGB→HSV 转换中为 f32，此处保持 f32 类型一致
    let bin_width = 360.0_f32 / config::HUE_BINS as f32;
    let mut hue_bins = [false; config::HUE_BINS];

    for y in 0..rgb.height() {
        for x in 0..rgb.width() {
            let px = rgb.get_pixel(x, y);
            let r = px[0] as f32 / 255.0;
            let g = px[1] as f32 / 255.0;
            let b = px[2] as f32 / 255.0;

            // RGB → HSV 转换（只提取色相 H）
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            let delta = max - min;

            let hue_deg = if delta < 0.001 {
                0.0 // 灰色/黑白，无色相
            } else if max == r {
                60.0 * (((g - b) / delta) % 6.0)
            } else if max == g {
                60.0 * (((b - r) / delta) + 2.0)
            } else {
                60.0 * (((r - g) / delta) + 4.0)
            };

            // 归一化到 0~360
            let hue = (hue_deg % 360.0 + 360.0) % 360.0;

            // 落入哪个 bin？
            let bin = (hue / bin_width) as usize;
            if bin < config::HUE_BINS {
                hue_bins[bin] = true;
            }
        }
    }

    // 统计覆盖的 bin 数量
    let covered = hue_bins.iter().filter(|&&b| b).count();
    covered as f64 / config::HUE_BINS as f64
}