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
/// - `(grad_magnitude, edge_binary, edge_ratio)`
///   - `grad_magnitude`: 梯度幅值图 (0~255，饱和截断)
///   - `edge_binary`: **二值边缘图** (0 或 255)，下游统计必须用它
///   - `edge_ratio`: 边缘像素占比 = 边缘像素数 / 总像素数 (0~1)
///
/// # 为什么必须返回二值图
/// 原实现只返回连续的梯度幅值图，下游（`sliding_window_features`、
/// `lower_advantage`、可视化）却一律用 `> 0` 判定"是不是边缘"。
/// 这等价于把"任何存在梯度的像素"都算作边缘——平坦区域的一点点噪声
/// 也会被计入，导致精心设计的自适应阈值 `threshold` 形同虚设。
/// 这里显式产出阈值化二值图，让统计与判定口径统一。
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
pub fn sobel_edge_detection(gray: &GrayImage, threshold: u8) -> (GrayImage, GrayImage, f64) {
    let (width, height) = gray.dimensions();
    let (w, h) = (width as usize, height as usize);

    // 扁平切片访问，避免逐像素 get_pixel 的边界检查与函数调用开销
    let src: &[u8] = gray.as_raw();

    let mut grad_vec = vec![0u8; w * h];
    let mut bin_vec = vec![0u8; w * h];

    // 统计边缘像素数
    let mut sum_edge = 0u64;

    // 遍历每个像素（跳过边界 1 像素，因为 3x3 卷积需要邻域）
    for y in 1..(h - 1) {
        for x in 1..(w - 1) {
            // 3x3 邻域，按 Sobel 核展开（展开后省掉 3x3 内层循环与查表）
            let i = y * w + x;
            let p00 = src[i - w - 1] as i32;
            let p10 = src[i - w] as i32;
            let p20 = src[i - w + 1] as i32;
            let p01 = src[i - 1] as i32;
            let p21 = src[i + 1] as i32;
            let p02 = src[i + w - 1] as i32;
            let p12 = src[i + w] as i32;
            let p22 = src[i + w + 1] as i32;

            // Gx: [-1 0 1; -2 0 2; -1 0 1]
            let gx = (p20 + 2 * p21 + p22) - (p00 + 2 * p01 + p02);
            // Gy: [-1 -2 -1; 0 0 0; 1 2 1]
            let gy = (p02 + 2 * p12 + p22) - (p00 + 2 * p10 + p20);

            // 梯度幅值 = |Gx| + |Gy| (曼哈顿距离，比欧氏距离快)
            //
            // 修复：原实现写 `(gx.abs() + gy.abs()) as u8`，而该和最大可达
            // 4*255*2 = 2040，`as` 是**截断回绕**而非饱和——256→0、260→4，
            // 于是最强烈的边缘反而被记成 0，弱边缘却可能值更大。
            // 这里改为饱和截断到 255。
            let mag = (gx.abs() + gy.abs()).min(255) as u8;

            grad_vec[i] = mag;

            // 统计边缘像素 (梯度 > threshold)，并写入二值图
            if mag > threshold {
                bin_vec[i] = 255;
                sum_edge += 1;
            }
        }
    }

    // 计算边缘像素占比
    let total_pixels = (w * h) as f64;
    let edge_ratio = sum_edge as f64 / total_pixels;

    let grad_magnitude = GrayImage::from_raw(width, height, grad_vec).unwrap();
    let edge_binary = GrayImage::from_raw(width, height, bin_vec).unwrap();

    (grad_magnitude, edge_binary, edge_ratio)
}

// ============================================================
// 滑动窗口特征
// ============================================================
/// 在边缘图上用滑动窗口扫描，计算"局部峰值"和"局部离散度"
///
/// # 参数
/// - `edge_binary`: **二值**边缘图 (来自 sobel_edge_detection，取值 0 或 255)
/// - `win_size`: 窗口大小 (像素)
/// - `step`: 滑动步长 (像素)
///
/// # 返回值
/// - `(local_peak, local_variance, peak_window)`
///   - `local_peak`: 所有窗口中边缘密度的最大值 (0~1)
///     表示"画面中最聚焦的区域有多丰富"
///   - `local_variance`: 所有窗口边缘密度的方差
///     表示"画面内容的均匀程度"
///     高方差 = 有疏有密 (有主体有背景)
///     低方差 = 均匀 (全是噪点或全是平滑区域)
///   - `peak_window`: 峰值窗口左上角坐标 `(x, y)`，供可视化复用，避免二次扫描
///
/// # 物理意义
/// 固定网格会把物体切成碎片，滑动窗口保证物体总被某个窗口完整框住。
/// 窗口大小 32，步长 16 意味着窗口重叠一半，任何物体至少被 2~3 个窗口覆盖。
///
/// # 性能
/// 用**积分图**把每个窗口的求和降到 O(1)。原实现对每个窗口逐像素扫描，
/// 128×128 图下约 49 窗口 × 1024 像素 ≈ 5 万次采样；积分图只需 O(N) 建表
/// + 49 次四则运算。
pub fn sliding_window_features(
    edge_binary: &GrayImage,
    win_size: u32,
    step: u32,
) -> (f64, f64, (u32, u32)) {
    let (w, h) = edge_binary.dimensions();
    let (wi, hi) = (w as usize, h as usize);
    let ws = win_size as usize;
    let st = step.max(1) as usize;

    // 图片比窗口还小，或窗口/步长为 0：无法扫描
    if ws == 0 || wi < ws || hi < ws {
        return (0.0, 0.0, (0, 0));
    }

    let src: &[u8] = edge_binary.as_raw();

    // ---- 构建积分图 (wi+1) × (hi+1) ----
    // integral[(y+1)*stride + (x+1)] = 左上角 (0,0)~(x,y) 闭区间的边缘像素总数
    let stride = wi + 1;
    let mut integral = vec![0u32; stride * (hi + 1)];
    for y in 0..hi {
        let row_off = y * wi;
        for x in 0..wi {
            let v = if src[row_off + x] > 0 { 1u32 } else { 0u32 };
            let above = integral[y * stride + (x + 1)];
            let left = integral[(y + 1) * stride + x];
            let diag = integral[y * stride + x];
            integral[(y + 1) * stride + (x + 1)] = v + above + left - diag;
        }
    }

    // 窗口 (x, y) ~ (x+ws-1, y+ws-1) 的边缘像素数，O(1) 查询
    let win_sum = |x: usize, y: usize| -> u32 {
        let x1 = x + ws;
        let y1 = y + ws;
        integral[y1 * stride + x1] + integral[y * stride + x]
            - integral[y * stride + x1] - integral[y1 * stride + x]
    };

    let mut ratios: Vec<f64> = Vec::new();
    let mut local_peak = 0.0_f64;
    let mut peak_window = (0u32, 0u32);

    let mut y = 0usize;
    while y + ws <= hi {
        let mut x = 0usize;
        while x + ws <= wi {
            // 边缘密度 = 窗口内边缘像素数 / 窗口面积
            let ratio = win_sum(x, y) as f64 / (ws * ws) as f64;
            if ratio > local_peak {
                local_peak = ratio;
                peak_window = (x as u32, y as u32);
            }
            ratios.push(ratio);
            x += st;
        }
        y += st;
    }

    // 如果窗口数为 0 (图片太小)，返回默认值
    if ratios.is_empty() {
        return (0.0, 0.0, (0, 0));
    }

    // 计算方差
    let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
    let var = ratios
        .iter()
        .map(|v| (v - mean).powi(2))
        .sum::<f64>()
        / ratios.len() as f64;

    (local_peak, var, peak_window)
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
pub fn extract_features(img: &DynamicImage, region: config::HalfRegion) -> FeatureMap {
    extract_features_with_motion(img, None, region)
}

/// 完整特征提取，支持帧间 motion 计算和用户指定的关注区间
///
/// # 参数
/// - `img`: 当前帧图像
/// - `prev_gray`: 上一帧的灰度图（THUMBNAIL_SIZE x THUMBNAIL_SIZE），用于计算 motion。
///   单图模式传 `None`，motion 固定为 0.0。
/// - `region`: 关注的半区方向（下半区/上半区/左半区/右半区），由 /set-region 切换。
pub fn extract_features_with_motion(
    img: &DynamicImage,
    prev_gray: Option<&GrayImage>,
    region: config::HalfRegion,
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

    // 平均亮度只计算一次（原实现为算 Sobel 阈值和 brightness 各扫了一遍全图）
    let total_px = (n * n) as f64;
    let sum_luma: f64 = small.pixels().map(|p| p[0] as f64).sum();
    let avg_luma = sum_luma / total_px;      // 0~255
    let brightness = avg_luma / 255.0;       // 特征 4: 归一化到 0~1

    // 特征 2 & 3: 边缘占比（Sobel 检测，自适应阈值防亮画面边缘泛滥）
    let edge_thresh = ((config::SOBEL_THRESHOLD_BASE as f64) + avg_luma * 0.3).clamp(20.0, 80.0) as u8;
    // edge_binary 为阈值化后的二值边缘图：后续所有"是否边缘"的判定都基于它，
    // 而不是基于连续的梯度幅值（否则等于把任何有梯度的像素都算作边缘）。
    let (_edge_map, edge_binary, edge_ratio) = sobel_edge_detection(&small, edge_thresh);

    // 特征 5 & 6: 滑动窗口特征 (局部峰值 + 方差)，附带峰值窗口坐标
    let (local_peak, local_variance, _peak_window) =
        sliding_window_features(&edge_binary, config::WINDOW_SIZE, config::WINDOW_STEP);

    // 特征 7: 半区优势比（根据 region 参数动态选择关注方向）
    // 统计"关注区"的边缘密度 vs "非关注区"的边缘密度
    // 注意：这里统计的是**边缘像素**（二值图），而非"任何有梯度的像素"
    let half = config::LOWER_HALF_OFFSET;
    let bin_raw: &[u8] = edge_binary.as_raw();
    let mut region_a = 0u64;  // 关注区边缘像素数
    let mut region_b = 0u64;  // 非关注区边缘像素数
    for y in 0..n {
        for x in 0..n {
            if bin_raw[(y * n + x) as usize] > 0 {
                let in_a = match region {
                    config::HalfRegion::Lower => y >= half,
                    config::HalfRegion::Upper => y < half,
                    config::HalfRegion::Left  => x < half,
                    config::HalfRegion::Right => x >= half,
                };
                if in_a { region_a += 1; } else { region_b += 1; }
            }
        }
    }
    // +1 防止除零
    let lower_advantage = region_a as f64 / (region_b as f64 + 1.0);

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
    // 用连通域数量 + 面积分布刻画"场景里有几个物体、大小是否悬殊"
    let (contour_count, contour_area_mean, contour_area_variance) = extract_contour_stats(&small);

    // 物体大小的**相对**离散程度：变异系数 CV = 标准差 / 均值。
    // 直接用面积方差不行——它是绝对量，随画面尺度飙升到 10^7 量级，
    // 任何饱和压缩都会让它恒等于 1（详见 config::CONTOUR_CV_MAX 注释）。
    // CV 尺度无关：物体等大 → 0，大小悬殊 → 1+。
    let contour_area_cv = if contour_area_mean > 0.0 {
        contour_area_variance.sqrt() / contour_area_mean
    } else {
        0.0
    };

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
    // 供评估使用的尺度无关指标（原始方差仍保留，用于前端展示）
    map.insert("contour_area_cv".to_string(), contour_area_cv);
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
/// - `GrayImage`: THUMBNAIL_SIZE x THUMBNAIL_SIZE 的**二值**边缘图 (0 或 255)
///
/// # 用途
/// visualization::generate_visualization() 用它绘制白色边缘高亮，
/// 与特征提取阶段使用的边缘判定口径保持一致。
pub fn get_edge_map(img: &DynamicImage) -> GrayImage {
    let gray = img.to_luma8();
    let n = config::THUMBNAIL_SIZE;
    let small = image::imageops::resize(
        &gray,
        n,
        n,
        image::imageops::FilterType::Nearest,
    );
    // 自适应阈值：亮度越高边缘越密集→提高阈值避免全图白
    let avg_brightness: f64 = small.pixels().map(|p| p[0] as f64).sum::<f64>() / (n * n) as f64;
    let dyn_thresh = ((config::SOBEL_THRESHOLD_BASE as f64) + avg_brightness * 0.3).clamp(20.0, 80.0) as u8;
    let (_grad, edge_binary, _ratio) = sobel_edge_detection(&small, dyn_thresh);
    edge_binary
}

// ============================================================
// 轮廓统计与颜色丰富度（特征 9、10 的计算函数）
// ============================================================

/// Otsu 自动阈值：在灰度直方图上最小化类内方差，返回最佳分割阈值。
fn otsu_threshold(gray: &GrayImage) -> u8 {
    let mut hist = [0u64; 256];
    for p in gray.pixels() { hist[p[0] as usize] += 1; }
    let total = (gray.width() * gray.height()) as f64;
    let mut sum_all = 0u64;
    for i in 0..256 { sum_all += i as u64 * hist[i]; }

    let (mut w0, mut sum0) = (0u64, 0u64);
    let mut best_thresh = 128u8;
    let mut best_between = 0.0f64;

    for t in 1..255 {
        w0 += hist[t];
        if w0 == 0 { continue; }
        sum0 += t as u64 * hist[t];
        let w1 = total as u64 - w0;
        if w1 == 0 { break; }
        let sum1 = sum_all - sum0;
        let m0 = sum0 as f64 / w0 as f64;
        let m1 = sum1 as f64 / w1 as f64;
        let between = w0 as f64 * w1 as f64 * (m0 - m1) * (m0 - m1);
        if between > best_between {
            best_between = between;
            best_thresh = t as u8;
        }
    }
    best_thresh
}

/// 在灰度图上做 Otsu 二值化 + 开运算去噪，返回干净的分割二值图。
/// 同时被 visualization.rs 调用来画物体轮廓。
pub fn segment_foreground(gray: &GrayImage) -> GrayImage {
    let (w, h) = gray.dimensions();
    let t = otsu_threshold(gray);
    let mut binary = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = gray.get_pixel(x, y)[0];
            binary.put_pixel(x, y, Luma([if v > t { 255u8 } else { 0u8 }]));
        }
    }
    // 开运算：先腐蚀再膨胀，去除椒盐噪点
    let opened = perform_opening(&binary, 2);
    opened
}

/// 简单开运算（腐蚀 → 膨胀），用于去除二值图中的椒盐噪点
fn perform_opening(img: &GrayImage, radius: u32) -> GrayImage {
    dilate(&erode(img, radius), radius)
}

/// 腐蚀：方形核最小值滤波。
///
/// 采用**可分离**实现（先水平窗口最小值，再垂直窗口最小值）：
/// 复杂度从朴素实现的 O(N·r²) 降到 O(N·r)。
/// 128×128 图、半径 2 时，采样次数从约 41 万降到约 16 万。
fn erode(img: &GrayImage, r: u32) -> GrayImage {
    separable_filter(img, r, true)
}

/// 膨胀：方形核最大值滤波（同样为可分离实现）。
fn dilate(img: &GrayImage, r: u32) -> GrayImage {
    separable_filter(img, r, false)
}

/// 可分离方形核最小/最大滤波
///
/// - `take_min = true`  → 腐蚀（取窗口最小值）
/// - `take_min = false` → 膨胀（取窗口最大值）
///
/// 边界采用 clamp（复制边缘像素），比"边界一律置 0"更贴近常规形态学语义。
fn separable_filter(img: &GrayImage, r: u32, take_min: bool) -> GrayImage {
    let (w, h) = img.dimensions();
    let (wi, hi) = (w as usize, h as usize);
    let r = r as usize;
    let src: &[u8] = img.as_raw();

    // 半径 0 时运算恒等，直接返回副本
    if r == 0 {
        return img.clone();
    }

    let mut tmp = vec![0u8; wi * hi];

    // ---- 第一趟：水平方向窗口极值 ----
    for y in 0..hi {
        let row = &src[y * wi..(y + 1) * wi];
        let out_row = &mut tmp[y * wi..(y + 1) * wi];
        for x in 0..wi {
            let lo = x.saturating_sub(r);
            let hi_x = (x + r).min(wi - 1);
            let mut acc = if take_min { u8::MAX } else { 0u8 };
            for xx in lo..=hi_x {
                let v = row[xx];
                if take_min { if v < acc { acc = v; } }
                else        { if v > acc { acc = v; } }
            }
            out_row[x] = acc;
        }
    }

    // ---- 第二趟：垂直方向窗口极值 ----
    let mut out = vec![0u8; wi * hi];
    for y in 0..hi {
        let lo = y.saturating_sub(r);
        let hi_y = (y + r).min(hi - 1);
        for x in 0..wi {
            let mut acc = if take_min { u8::MAX } else { 0u8 };
            for yy in lo..=hi_y {
                let v = tmp[yy * wi + x];
                if take_min { if v < acc { acc = v; } }
                else        { if v > acc { acc = v; } }
            }
            out[y * wi + x] = acc;
        }
    }

    GrayImage::from_raw(w, h, out).unwrap()
}

/// 8-连通域标记，返回每个前景连通域的**像素面积**
///
/// # 为什么不用 find_contours 的 points.len()
/// `imageproc::contours::find_contours` 返回的是**边界像素链**，
/// `points.len()` 是周长而不是面积。原实现把它当作面积参与
/// `contour_area_variance` 计算，量纲错误且二者不成正比
/// （同面积下形状越复杂周长越长）。
/// 这里用 flood fill 统计真实的连通像素数。
fn connected_component_areas(binary: &GrayImage) -> Vec<u32> {
    let (w, h) = binary.dimensions();
    let (wi, hi) = (w as usize, h as usize);
    let src: &[u8] = binary.as_raw();

    let mut visited = vec![false; wi * hi];
    let mut areas: Vec<u32> = Vec::new();
    let mut stack: Vec<usize> = Vec::with_capacity(512);

    for start in 0..(wi * hi) {
        if visited[start] || src[start] == 0 {
            continue;
        }

        let mut area = 0u32;
        visited[start] = true;
        stack.clear();
        stack.push(start);

        while let Some(idx) = stack.pop() {
            area += 1;
            let x = idx % wi;
            let y = idx / wi;

            // 8 邻域扩散
            for dy in -1isize..=1 {
                for dx in -1isize..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x as isize + dx;
                    let ny = y as isize + dy;
                    if nx < 0 || ny < 0 || nx >= wi as isize || ny >= hi as isize {
                        continue;
                    }
                    let nidx = (ny as usize) * wi + (nx as usize);
                    if !visited[nidx] && src[nidx] > 0 {
                        visited[nidx] = true;
                        stack.push(nidx);
                    }
                }
            }
        }

        areas.push(area);
    }

    areas
}

/// 提取轮廓统计（基于 Otsu 前景分割，而非 Sobel 边缘）
///
/// 在灰度原图上做 Otsu 分割 → 开运算去噪 → 连通域标记，返回：
/// - `轮廓数量`：(usize) 独立前景物体个数
/// - `面积均值`：(f64) 物体的平均像素数
/// - `面积方差`：(f64) 物体大小的方差
pub fn extract_contour_stats(gray_thumb: &GrayImage) -> (usize, f64, f64) {
    let binary = segment_foreground(gray_thumb);

    // 连通域真实像素面积（原实现误用轮廓链长度当面积，详见函数注释）
    let areas: Vec<f64> = connected_component_areas(&binary)
        .into_iter()
        .map(|a| a as f64)
        .filter(|&a| a >= config::MIN_OBJECT_AREA)
        .collect();

    let count = areas.len();
    if count == 0 { return (0, 0.0, 0.0); }

    let mean = areas.iter().sum::<f64>() / count as f64;
    let variance = areas.iter().map(|a| (a - mean) * (a - mean)).sum::<f64>() / count as f64;

    (count, mean, variance)
}
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

// ============================================================
// 单元测试
//
// 这里锁定的都是"曾经出错、且错误不会导致崩溃只会静默失真"的点，
// 靠肉眼观察很难发现，必须有断言兜底。
// ============================================================
#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一张左半区 / 右半区分别为指定灰度的测试图
    fn two_tone(width: u32, height: u32, left: u8, right: u8) -> GrayImage {
        let mut img = GrayImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                let v = if x < width / 2 { left } else { right };
                img.put_pixel(x, y, Luma([v]));
            }
        }
        img
    }

    /// 回归测试：Sobel 梯度幅值不得因 u8 截断回绕而丢失强边缘。
    ///
    /// 0→64 的阶跃在边界处 |Gx| = 4*(64-0) = 256，
    /// 旧实现 `(gx.abs() + gy.abs()) as u8` 会把它回绕成 0，
    /// 于是画面中最强烈的边缘反而被判定为"非边缘"。
    #[test]
    fn sobel_gradient_saturates_instead_of_wrapping() {
        let img = two_tone(8, 8, 0, 64);
        let (grad, binary, edge_ratio) = sobel_edge_detection(&img, 20);

        // 边界列 x=3（最后一个 0）与 x=4（第一个 64）都应取到饱和值 255
        assert_eq!(grad.get_pixel(3, 4)[0], 255, "梯度 256 应饱和为 255，而非回绕为 0");
        assert_eq!(grad.get_pixel(4, 4)[0], 255, "梯度 256 应饱和为 255，而非回绕为 0");

        // 饱和后自然高于阈值，应被二值图标记为边缘
        assert!(binary.get_pixel(3, 4)[0] > 0, "强边缘应被二值图标记");
        assert!(edge_ratio > 0.0, "存在强边缘时占比不应为 0");
    }

    /// 回归测试：滑动窗口必须统计**阈值化后的边缘**，而不是"任何有梯度的像素"。
    ///
    /// 线性渐变图每个像素都有稳定梯度（|Gx| = 8），但都低于阈值 20。
    /// 旧实现在窗口内用 `> 0` 判定，会把整片渐变都算成边缘，
    /// 导致 local_peak / lower_advantage 恒接近满值、完全丧失区分度。
    #[test]
    fn window_stats_respect_edge_threshold() {
        let n = config::THUMBNAIL_SIZE;
        let mut img = GrayImage::new(n, n);
        for y in 0..n {
            for x in 0..n {
                img.put_pixel(x, y, Luma([x as u8]));
            }
        }

        let (_grad, binary, edge_ratio) = sobel_edge_detection(&img, 20);
        assert_eq!(edge_ratio, 0.0, "渐变梯度低于阈值，不应判定为边缘");

        let (local_peak, local_variance, _) =
            sliding_window_features(&binary, config::WINDOW_SIZE, config::WINDOW_STEP);
        assert_eq!(local_peak, 0.0, "二值图无边缘时局部峰值应为 0");
        assert_eq!(local_variance, 0.0, "二值图全空时方差应为 0");
    }

    /// 积分图优化必须与朴素逐像素实现给出完全一致的结果
    #[test]
    fn sliding_window_matches_naive_scan() {
        let (w, h) = (32u32, 32u32);
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                // 确定性的伪图案，不依赖随机数
                let v = if (x * 7 + y * 13) % 5 == 0 { 255u8 } else { 0u8 };
                img.put_pixel(x, y, Luma([v]));
            }
        }

        let (win_size, step) = (config::WINDOW_SIZE, config::WINDOW_STEP);
        let (peak, var, peak_win) = sliding_window_features(&img, win_size, step);

        // 朴素参照实现
        let mut ratios = Vec::new();
        let mut naive_peak = 0.0_f64;
        let mut naive_peak_win = (0u32, 0u32);
        let mut y = 0;
        while y + win_size <= h {
            let mut x = 0;
            while x + win_size <= w {
                let mut cnt = 0u32;
                for j in 0..win_size {
                    for i in 0..win_size {
                        if img.get_pixel(x + i, y + j)[0] > 0 {
                            cnt += 1;
                        }
                    }
                }
                let r = cnt as f64 / (win_size * win_size) as f64;
                if r > naive_peak {
                    naive_peak = r;
                    naive_peak_win = (x, y);
                }
                ratios.push(r);
                x += step;
            }
            y += step;
        }
        let mean = ratios.iter().sum::<f64>() / ratios.len() as f64;
        let naive_var =
            ratios.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / ratios.len() as f64;

        assert!((peak - naive_peak).abs() < 1e-12, "局部峰值应与朴素实现一致: {peak} vs {naive_peak}");
        assert!((var - naive_var).abs() < 1e-12, "方差应与朴素实现一致: {var} vs {naive_var}");
        assert_eq!(peak_win, naive_peak_win, "峰值窗口坐标应与朴素实现一致");
    }

    /// 连通域面积必须是**真实像素数**，而不是轮廓边界链长度（周长）。
    /// 旧实现误用 `find_contours(...).points.len()` 当作面积，
    /// 两者量纲不同，同面积下形状越复杂周长越长。
    #[test]
    fn connected_component_areas_are_pixel_counts() {
        let mut img = GrayImage::new(16, 16);
        // 块 A：x 1..5 (宽 4) × y 1..4 (高 3) = 12 像素
        for y in 1..4 {
            for x in 1..5 {
                img.put_pixel(x, y, Luma([255]));
            }
        }
        // 块 B：x 10..12 (宽 2) × y 10..12 (高 2) = 4 像素
        for y in 10..12 {
            for x in 10..12 {
                img.put_pixel(x, y, Luma([255]));
            }
        }

        let mut areas = connected_component_areas(&img);
        areas.sort_unstable();
        assert_eq!(areas, vec![4, 12], "连通域面积应等于真实像素数");
    }

    /// 8-连通性：对角线相接的两个像素应属于同一连通域
    #[test]
    fn connected_components_join_diagonally() {
        let mut img = GrayImage::new(4, 4);
        img.put_pixel(1, 1, Luma([255]));
        img.put_pixel(2, 2, Luma([255])); // 与 (1,1) 对角相邻

        let areas = connected_component_areas(&img);
        assert_eq!(areas.len(), 1, "对角相接的像素应合并为一个连通域");
        assert_eq!(areas[0], 2, "连通域面积应为 2");
    }

    /// 可分离形态学（腐蚀/膨胀）应与朴素方形核实现结果一致
    #[test]
    fn separable_morphology_matches_naive_kernel() {
        let (w, h) = (16u32, 16u32);
        let mut img = GrayImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if (x * 3 + y * 5) % 7 < 2 { 255u8 } else { 0u8 };
                img.put_pixel(x, y, Luma([v]));
            }
        }
        let r = 2u32;

        // 朴素腐蚀：半径 2 的方形核内存在 0 → 结果为 0
        let eroded = erode(&img, r);
        for y in r..(h - r) {
            for x in r..(w - r) {
                let mut all = true;
                for dy in -(r as i32)..=(r as i32) {
                    for dx in -(r as i32)..=(r as i32) {
                        if img.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)[0] == 0 {
                            all = false;
                        }
                    }
                }
                let expect = if all { 255u8 } else { 0u8 };
                assert_eq!(
                    eroded.get_pixel(x, y)[0],
                    expect,
                    "腐蚀结果在 ({x}, {y}) 与朴素实现不一致"
                );
            }
        }
    }

    /// 回归测试：物体大小差异必须用**尺度无关**的指标。
    ///
    /// 面积方差是绝对量，量级可达 10^7，用 `x/(x+1)` 压缩后一律饱和到 ~1.0，
    /// "大小悬殊"与"大小均匀"两种截然不同的场景几乎无法区分，
    /// 该权重退化成一个常数偏置。变异系数 CV 则能清晰区分二者。
    #[test]
    fn contour_size_diversity_is_scale_invariant() {
        let cv_of = |areas: &[f64]| -> f64 {
            let n = areas.len() as f64;
            let mean = areas.iter().sum::<f64>() / n;
            let var = areas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / n;
            var.sqrt() / mean
        };
        // 旧归一化方式：x/(x+1)
        let old_norm = |areas: &[f64]| -> f64 {
            let n = areas.len() as f64;
            let mean = areas.iter().sum::<f64>() / n;
            let var = areas.iter().map(|a| (a - mean).powi(2)).sum::<f64>() / n;
            var / (var + 1.0)
        };

        let diverse = [100.0, 400.0, 1500.0]; // 大小悬殊
        let uniform = [600.0, 650.0, 700.0]; // 大小接近

        assert!(
            cv_of(&diverse) > cv_of(&uniform) * 5.0,
            "CV 应能区分大小悬殊与大小接近：{} vs {}",
            cv_of(&diverse),
            cv_of(&uniform)
        );
        assert!(
            (old_norm(&diverse) - old_norm(&uniform)).abs() < 0.01,
            "旧归一化方式下两者几乎无差别（{} vs {}），这正是要修的问题",
            old_norm(&diverse),
            old_norm(&uniform)
        );
    }

    /// 防止 motion 归一化参数退化回"特征失效"的取值。
    ///
    /// motion 是 128×128 灰度图的平均绝对差，理论上限 255 但实测分布低得多。
    /// 若除数取理论上限，正常行车帧间差归一化后不足 0.2，再乘 0.04 的权重
    /// 对总分贡献可忽略——所谓"风险触发器"名存实亡。
    #[test]
    fn motion_normalization_has_discrimination() {
        let norm = |m: f64| (m / config::MOTION_DIVISOR).min(1.0);
        let (slow, normal, fast) = (norm(2.0), norm(12.0), norm(40.0));

        assert!(
            normal - slow > 0.3,
            "正常运动与静止应拉开明显差距（当前差 {}），否则 motion 特征失效",
            normal - slow
        );
        assert!(fast >= 1.0, "剧烈运动/场景切换应达到饱和");
    }
}