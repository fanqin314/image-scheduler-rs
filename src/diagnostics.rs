// ============================================================
// diagnostics.rs — 特征诊断与批量评测
//
// 存在的理由：本系统 9 项权重是手工设定的常数。没有这套工具时，
// 某个特征是否已经饱和/失效只能靠人肉排查——`contour_area_variance`
// 就曾长期恒等于 1.0，占 10% 权重却毫无区分度，一直没被发现。
//
// Reducto (SIGCOMM'20) 的核心结论正是：端侧过滤必须持续校准
// "廉价特征 ↔ 真实价值"的映射关系，而这个关系是时变的。
// 本模块就是做这件事所需的量化地基。
// ============================================================

use crate::{config, evaluator, features, types::FeatureMap};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};

// ============================================================
// 判定阈值
// ============================================================

/// 标准差低于此值视为"恒定"（在样本上取值几乎不变）
const CONSTANT_STD: f64 = 1e-6;

/// 归一化后极差低于此值视为"区分度低"
const SPREAD_FLOOR: f64 = 0.05;

/// 均值高于此值视为顶到归一化上限
const SATURATE_HIGH: f64 = 0.98;

/// 均值低于此值视为贴住归一化下限
const SATURATE_LOW: f64 = 0.02;

/// 给出可信诊断结论所需的最少样本数
const MIN_SAMPLES: usize = 20;

/// 参与评分的 9 个分量（与 config.rs 的权重一一对应）
pub const COMPONENT_NAMES: [&str; 9] = [
    "saliency",
    "local_peak",
    "contour_count",
    "color_richness",
    "edge_ratio",
    "lower_advantage",
    "contour_area_var",
    "entropy",
    "motion",
];

/// 全部原始特征（含仅用于展示的项）
pub const FEATURE_NAMES: [&str; 11] = [
    "entropy",
    "edge_ratio",
    "brightness",
    "local_peak",
    "local_variance",
    "lower_advantage",
    "motion",
    "contour_count",
    "contour_area_variance",
    "contour_area_cv",
    "color_richness",
];

/// 支持的图片扩展名
const IMAGE_EXTS: [&str; 6] = ["jpg", "jpeg", "png", "bmp", "webp", "gif"];

// ============================================================
// 基础统计
// ============================================================

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    /// 总体标准差（描述这批样本自身的离散程度）
    pub std: f64,
    /// 极差 = max - min
    pub spread: f64,
}

/// 计算一组样本的基本统计量；空切片返回 None
pub fn summarize(values: &[f64]) -> Option<Stats> {
    if values.is_empty() {
        return None;
    }
    let n = values.len();
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut sum = 0.0;
    for &v in values {
        if v < min {
            min = v;
        }
        if v > max {
            max = v;
        }
        sum += v;
    }
    let mean = sum / n as f64;
    let var = values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / n as f64;
    Some(Stats {
        n,
        min,
        max,
        mean,
        std: var.sqrt(),
        spread: max - min,
    })
}

/// 线性插值分位数，`q` 取 0~1。空切片返回 0.0。
pub fn quantile(values: &[f64], q: f64) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    quantile_sorted(&sorted, q)
}

fn quantile_sorted(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    if sorted.len() == 1 {
        return sorted[0];
    }
    let q = q.clamp(0.0, 1.0);
    let pos = q * (sorted.len() - 1) as f64;
    let lo = pos.floor() as usize;
    let hi = pos.ceil() as usize;
    if lo == hi {
        return sorted[lo];
    }
    let frac = pos - lo as f64;
    sorted[lo] * (1.0 - frac) + sorted[hi] * frac
}

/// 给定"目标送云端比例"，从分数样本反推阈值。
///
/// 用途：把 `config::THRESHOLD_CLOUD` 从"拍脑袋的常数"变成
/// "由成本目标直接决定"——想让 5% 的帧上云，就取 95 分位。
/// 这是 Reducto 式在线校准的离线版本。
pub fn suggest_threshold(scores: &[f64], target_cloud_ratio: f64) -> f64 {
    if scores.is_empty() {
        return config::THRESHOLD_CLOUD;
    }
    // 希望 P(score >= threshold) = target，即 threshold = 第 (1-target) 分位数
    quantile(scores, 1.0 - target_cloud_ratio)
}

// ============================================================
// 分量健康度
// ============================================================

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    /// 区分度正常
    Ok,
    /// 样本量不足，不做判定
    Insufficient,
    /// 取值恒定，完全没有信息量
    Constant,
    /// 几乎总是顶到归一化上限
    SaturatedHigh,
    /// 几乎总是贴住下限
    SaturatedLow,
    /// 极差过小，加权后对总分影响微弱
    LowSpread,
}

impl Health {
    pub fn label(&self) -> &'static str {
        match self {
            Health::Ok => "正常",
            Health::Insufficient => "样本不足",
            Health::Constant => "恒定(无信息)",
            Health::SaturatedHigh => "饱和(顶上限)",
            Health::SaturatedLow => "饱和(贴下限)",
            Health::LowSpread => "区分度低",
        }
    }

    /// 是否需要引起注意
    pub fn is_problem(&self) -> bool {
        !matches!(self, Health::Ok | Health::Insufficient)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ComponentDiag {
    pub name: &'static str,
    pub weight: f64,
    pub stats: Stats,
    pub health: Health,
    /// 该分量最多能影响总分多少 = 权重 × 归一化极差
    pub contribution_range: f64,
}

fn classify(stats: Stats, n: usize) -> Health {
    if n < MIN_SAMPLES {
        return Health::Insufficient;
    }
    if stats.std < CONSTANT_STD {
        return Health::Constant;
    }
    if stats.mean >= SATURATE_HIGH {
        return Health::SaturatedHigh;
    }
    if stats.mean <= SATURATE_LOW {
        return Health::SaturatedLow;
    }
    if stats.spread < SPREAD_FLOOR {
        return Health::LowSpread;
    }
    Health::Ok
}

// ============================================================
// 批量评测
// ============================================================

pub struct BenchOptions {
    /// 需要计算推荐阈值的目标 CLOUD 比例列表
    pub target_ratios: Vec<f64>,
}

impl Default for BenchOptions {
    fn default() -> Self {
        Self {
            target_ratios: vec![0.05, 0.10, 0.20, 0.30, 0.50],
        }
    }
}

pub struct FileResult {
    pub path: PathBuf,
    pub score: f64,
    pub action: String,
    pub breakdown: evaluator::ScoreBreakdown,
    pub features: FeatureMap,
}

pub struct BenchReport {
    pub files: Vec<FileResult>,
    pub cloud_count: usize,
    pub local_count: usize,
    pub score_stats: Option<Stats>,
    /// 9 个评分分量的诊断，按"对总分影响力"升序（最可疑的在前）
    pub component_diags: Vec<ComponentDiag>,
    /// 11 个原始特征的分布
    pub feature_stats: Vec<(&'static str, Stats)>,
    /// (目标 CLOUD 比例, 推荐阈值)
    pub suggested: Vec<(f64, f64)>,
}

impl BenchReport {
    pub fn sample_count(&self) -> usize {
        self.files.len()
    }

    /// 样本量是否足以支撑可信结论
    pub fn has_enough_samples(&self) -> bool {
        self.sample_count() >= MIN_SAMPLES
    }
}

/// 递归收集图片文件路径（结果已排序，保证输出可复现）
pub fn collect_image_paths(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut out = Vec::new();

    if root.is_file() {
        if is_image(root) {
            out.push(root.to_path_buf());
        }
        return Ok(out);
    }
    if !root.is_dir() {
        anyhow::bail!("路径不存在: {}", root.display());
    }

    collect_recursive(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_recursive(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_recursive(&path, out)?;
        } else if is_image(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_image(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| IMAGE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 对一批图片跑完整流水线（特征提取 → 归一化 → 评分 → 诊断）
pub fn run_bench(paths: &[PathBuf], opts: &BenchOptions) -> anyhow::Result<BenchReport> {
    let mut files: Vec<FileResult> = Vec::with_capacity(paths.len());

    for path in paths {
        let img = match image::open(path) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("跳过 {}: 解码失败 ({})", path.display(), e);
                continue;
            }
        };

        let feature_map = features::extract_features(&img, config::DEFAULT_REGION);
        let breakdown = evaluator::normalize(&feature_map);
        let evaluation = evaluator::evaluate(&feature_map);

        files.push(FileResult {
            path: path.clone(),
            score: evaluation.score,
            action: evaluation.action,
            breakdown,
            features: feature_map,
        });
    }

    if files.is_empty() {
        anyhow::bail!("没有成功处理任何图片");
    }

    // ---- 决策分布与分数分布 ----
    let scores: Vec<f64> = files.iter().map(|f| f.score).collect();
    let cloud_count = files.iter().filter(|f| f.action == "CLOUD").count();
    let local_count = files.len() - cloud_count;
    let score_stats = summarize(&scores);

    // ---- 9 个评分分量的诊断 ----
    let n = files.len();
    let mut component_diags: Vec<ComponentDiag> = COMPONENT_NAMES
        .iter()
        .enumerate()
        .filter_map(|(idx, &name)| {
            let values: Vec<f64> = files
                .iter()
                .map(|f| f.breakdown.terms()[idx].1) // (名称, 归一化值, 权重, 贡献)
                .collect();
            let stats = summarize(&values)?;
            let weight = files[0].breakdown.terms()[idx].2;
            Some(ComponentDiag {
                name,
                weight,
                stats,
                health: classify(stats, n),
                contribution_range: weight * stats.spread,
            })
        })
        .collect();

    // 最没影响力的排前面
    component_diags.sort_by(|a, b| {
        a.contribution_range
            .partial_cmp(&b.contribution_range)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // ---- 原始特征分布 ----
    let feature_stats: Vec<(&'static str, Stats)> = FEATURE_NAMES
        .iter()
        .filter_map(|&name| {
            let values: Vec<f64> = files
                .iter()
                .map(|f| *f.features.get(name).unwrap_or(&0.0))
                .collect();
            let stats = summarize(&values)?;
            Some((name, stats))
        })
        .collect();

    // ---- 阈值推荐 ----
    let suggested: Vec<(f64, f64)> = opts
        .target_ratios
        .iter()
        .map(|&ratio| (ratio, suggest_threshold(&scores, ratio)))
        .collect();

    Ok(BenchReport {
        files,
        cloud_count,
        local_count,
        score_stats,
        component_diags,
        feature_stats,
        suggested,
    })
}

// ============================================================
// 运行时自适应阈值（Reducto 式在线校准的简化实现）
// ============================================================

/// 基于分数滑动窗口的自适应阈值
///
/// 维护最近若干帧的分数，按目标 CLOUD 比例实时反解阈值：
/// 想让约 20% 的帧上云，阈值就取分数分布的 80 分位。
///
/// 为什么需要它：固定阈值在长时间运行下必然失准。傍晚光线变暗时
/// 分数整体下滑，固定 0.65 会让 CLOUD 比例骤降到接近 0；
/// 进入复杂路段时又可能飙升到几乎全上云。自适应阈值锁定的不是
/// "分数"，而是**成本**（上云比例），这正是调度真正关心的量。
///
/// 复杂度：每次取阈值需排序一次窗口，1000 帧约 10^4 次比较，
/// 在毫秒级特征提取面前可忽略。
pub struct AdaptiveThreshold {
    scores: VecDeque<f64>,
    capacity: usize,
    target_cloud_ratio: f64,
}

impl AdaptiveThreshold {
    pub fn new(capacity: usize, target_cloud_ratio: f64) -> Self {
        Self {
            scores: VecDeque::with_capacity(capacity.min(4096)),
            capacity: capacity.max(1),
            target_cloud_ratio: target_cloud_ratio.clamp(0.0, 1.0),
        }
    }

    /// 按 config.rs 中的默认参数构造
    pub fn from_config() -> Self {
        Self::new(config::ADAPTIVE_WINDOW, config::TARGET_CLOUD_RATIO)
    }

    /// 记录一帧的分数
    pub fn push(&mut self, score: f64) {
        if self.scores.len() == self.capacity {
            self.scores.pop_front();
        }
        self.scores.push_back(score);
    }

    /// 当前生效的阈值。
    ///
    /// 窗口内样本不足 `ADAPTIVE_MIN_SAMPLES` 时回退到 `THRESHOLD_CLOUD`，
    /// 避免冷启动阶段阈值剧烈抖动。
    pub fn threshold(&self) -> f64 {
        if self.scores.len() < config::ADAPTIVE_MIN_SAMPLES {
            return config::THRESHOLD_CLOUD;
        }
        let scores: Vec<f64> = self.scores.iter().copied().collect();
        suggest_threshold(&scores, self.target_cloud_ratio)
    }

    /// 窗口内已积累的样本数
    pub fn len(&self) -> usize {
        self.scores.len()
    }

    pub fn is_empty(&self) -> bool {
        self.scores.is_empty()
    }
}

// ============================================================
// 报告输出
// ============================================================

const WIDTH: usize = 74;

pub fn print_report(report: &BenchReport) {
    let line = "=".repeat(WIDTH);
    let thin = "-".repeat(WIDTH);

    println!("\n{}", line);
    println!("  批量评测报告");
    println!("{}\n", line);

    let n = report.sample_count();
    println!("样本数: {}", n);
    if !report.has_enough_samples() {
        println!(
            "  [!] 样本量少于 {} 张，以下统计仅供参考，不宜据此下结论。",
            MIN_SAMPLES
        );
    }

    // ---- 决策分布 ----
    println!("\n{}", thin);
    println!("决策分布 (当前阈值 THRESHOLD_CLOUD = {})\n", config::THRESHOLD_CLOUD);
    let total = n.max(1) as f64;
    println!(
        "  CLOUD {:>6}  {:>6.1}%",
        report.cloud_count,
        report.cloud_count as f64 / total * 100.0
    );
    println!(
        "  LOCAL {:>6}  {:>6.1}%",
        report.local_count,
        report.local_count as f64 / total * 100.0
    );

    // ---- 分数分布 ----
    if let Some(s) = report.score_stats {
        println!("\n{}", thin);
        println!("分数分布\n");
        let scores: Vec<f64> = report.files.iter().map(|f| f.score).collect();
        println!(
            "  min {:>7.4}   p25 {:>7.4}   median {:>7.4}   p75 {:>7.4}   max {:>7.4}",
            s.min,
            quantile(&scores, 0.25),
            quantile(&scores, 0.50),
            quantile(&scores, 0.75),
            s.max
        );
        println!("  mean {:>7.4}   std {:>7.4}", s.mean, s.std);
    }

    // ---- 分量诊断 ----
    println!("\n{}", thin);
    println!("评分分量诊断（按对总分的影响力升序，最可疑的排最前）\n");
    println!(
        "  {:<20} {:>6} {:>8} {:>8} {:>10}  {}",
        "分量", "权重", "均值", "极差", "贡献幅度", "状态"
    );
    for d in &report.component_diags {
        let flag = if d.health.is_problem() { "[!]" } else { "   " };
        println!(
            "{} {:<20} {:>6.2} {:>8.4} {:>8.4} {:>10.5}  {}",
            flag,
            d.name,
            d.weight,
            d.stats.mean,
            d.stats.spread,
            d.contribution_range,
            d.health.label()
        );
    }
    println!(
        "\n  贡献幅度 = 权重 x 归一化极差，表示该分量最多能拉动总分多少。"
    );

    let problems: Vec<&&str> = report
        .component_diags
        .iter()
        .filter(|d| d.health.is_problem())
        .map(|d| &d.name)
        .collect();
    if !problems.is_empty() {
        println!("\n  存在问题、建议优先复核的分量: {:?}", problems);
    }

    // ---- 原始特征分布 ----
    println!("\n{}", thin);
    println!("原始特征分布（用于对照调参）\n");
    println!(
        "  {:<24} {:>12} {:>12} {:>12} {:>10}",
        "特征", "min", "max", "mean", "std"
    );
    for (name, s) in &report.feature_stats {
        println!(
            "  {:<24} {:>12.4} {:>12.4} {:>12.4} {:>10.4}",
            name, s.min, s.max, s.mean, s.std
        );
    }
    println!(
        "\n  注: motion 是视频特征，纯图片集上恒为 0 属预期，不是缺陷。"
    );

    // ---- 阈值推荐 ----
    println!("\n{}", thin);
    println!("阈值推荐：若希望 X% 的帧送云端，阈值应设为\n");
    for (ratio, threshold) in &report.suggested {
        println!(
            "  目标 CLOUD {:>4.0}%   ->   THRESHOLD_CLOUD = {:.4}",
            ratio * 100.0,
            threshold
        );
    }
    println!("\n  用法: 先按业务成本定下「能接受多少帧上云」，再取对应阈值写回");
    println!("  config::THRESHOLD_CLOUD，而不是反过来先拍一个数字。");

    // ---- 逐文件明细 ----
    println!("\n{}", thin);
    println!("逐文件明细\n");
    for f in &report.files {
        println!(
            "  {:>7.4}  {:<6}  {}",
            f.score,
            f.action,
            f.path.display()
        );
    }

    println!("\n{}\n", line);
}

// ============================================================
// 单元测试：核心诊断逻辑（失效特征判定 / 分位数阈值 / 自适应窗口）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config;

    #[test]
    fn summarize_basic() {
        let s = summarize(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert_eq!(s.n, 5);
        assert!((s.min - 1.0).abs() < 1e-12);
        assert!((s.max - 5.0).abs() < 1e-12);
        assert!((s.mean - 3.0).abs() < 1e-12);
        assert!((s.spread - 4.0).abs() < 1e-12);
        // 总体标准差：var = ((1-3)^2+...+(5-3)^2)/5 = 2 → std = √2
        assert!((s.std - 2.0_f64.sqrt()).abs() < 1e-9);
    }

    #[test]
    fn summarize_empty_is_none() {
        assert!(summarize(&[]).is_none());
    }

    #[test]
    fn quantile_linear_interp() {
        let v = [0.0, 10.0, 20.0, 30.0, 40.0];
        assert!((quantile(&v, 0.5) - 20.0).abs() < 1e-9);
        // 0.8 分位：pos = 0.8*4 = 3.2 → 30 + 0.2*(40-30) = 32
        assert!((quantile(&v, 0.8) - 32.0).abs() < 1e-9);
    }

    #[test]
    fn suggest_threshold_target_ratio() {
        // 10 个分数 0.0,0.1,...,0.9
        let scores: Vec<f64> = (0..10).map(|i| i as f64 / 10.0).collect();
        // 目标 20% CLOUD → 80 分位 = 0.72；分数 >= 0.72 的恰有 {0.8,0.9} 共 2 个 = 20%
        let t = suggest_threshold(&scores, 0.2);
        assert!((t - 0.72).abs() < 1e-9, "got {}", t);
        // 目标 50% CLOUD → 50 分位 = 0.45
        let t2 = suggest_threshold(&scores, 0.5);
        assert!((t2 - 0.45).abs() < 1e-9, "got {}", t2);
    }

    #[test]
    fn classify_constant() {
        let s = Stats { n: 100, min: 0.5, max: 0.5, mean: 0.5, std: 0.0, spread: 0.0 };
        assert_eq!(classify(s, 100), Health::Constant);
    }

    #[test]
    fn classify_saturated_high() {
        let s = Stats { n: 100, min: 0.99, max: 1.0, mean: 0.999, std: 0.003, spread: 0.01 };
        assert_eq!(classify(s, 100), Health::SaturatedHigh);
    }

    #[test]
    fn classify_low_spread() {
        let s = Stats { n: 100, min: 0.40, max: 0.43, mean: 0.415, std: 0.01, spread: 0.03 };
        assert_eq!(classify(s, 100), Health::LowSpread);
    }

    #[test]
    fn classify_ok() {
        let s = Stats { n: 100, min: 0.1, max: 0.9, mean: 0.5, std: 0.2, spread: 0.8 };
        assert_eq!(classify(s, 100), Health::Ok);
    }

    #[test]
    fn classify_insufficient_samples() {
        // 即使取值恒定，样本量不足也只报 Insufficient，不误判为失效
        let s = Stats { n: 5, min: 0.5, max: 0.5, mean: 0.5, std: 0.0, spread: 0.0 };
        assert_eq!(classify(s, 5), Health::Insufficient);
    }

    #[test]
    fn adaptive_threshold_fallback_then_solve() {
        let mut at = AdaptiveThreshold::new(1000, 0.2);
        // 冷启动：样本不足，应回退到固定阈值
        at.push(0.9);
        assert!((at.threshold() - config::THRESHOLD_CLOUD).abs() < 1e-12);
        // 灌满到 MIN_SAMPLES 个分数（0.5~1.0 区间）
        for i in 0..config::ADAPTIVE_MIN_SAMPLES {
            let v = 1.0 - (i as f64 / config::ADAPTIVE_MIN_SAMPLES as f64) * 0.5;
            at.push(v);
        }
        let t = at.threshold();
        assert!(t > 0.0 && t < 1.0, "窗口满后阈值应求解，实际 {}", t);
    }

    #[test]
    fn adaptive_threshold_window_rolls_and_adapts() {
        // 容量 1000、目标 25% CLOUD。先灌入低分循环值，再涌入高分，
        // 验证窗口滚动（上限封顶）且阈值随之自适应抬高。
        //
        // 自适应阈值的语义是「锁定上云比例」而非「锁定分数」：高分变多后，
        // 为维持 25% 上云比例，阈值必须抬高，否则会有超过 25% 的帧越线。
        let mut at = AdaptiveThreshold::new(1000, 0.25);
        for i in 0..config::ADAPTIVE_MIN_SAMPLES {
            at.push((i % 4) as f64); // 0,1,2,3 循环 → 75 分位约 2
        }
        let t_low = at.threshold();
        for _ in 0..300 {
            at.push(10.0); // 高分涌入，挤占窗口
        }
        let t_high = at.threshold();
        assert!(t_high > t_low, "高分涌入应抬高阈值: {} vs {}", t_low, t_high);
        assert!(at.len() <= 1000, "窗口应封顶在容量内");
    }
}

