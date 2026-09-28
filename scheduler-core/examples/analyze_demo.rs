//! analyze_frame 独立 demo
//!
//! 验证 `scheduler-core` 对外 API 可脱离 HTTP 服务独立运行：
//! 构造一张特征图 → `analyze_frame` → 打印分数、动作与各分量明细。
//!
//! 运行：`cargo run --example analyze_demo --manifest-path scheduler-core/Cargo.toml`

use scheduler_core::types::FeatureMap;

/// 一张典型的"夜视车行复杂帧"特征图（局部有主体、轮廓多、运动明显）。
fn sample_frame() -> FeatureMap {
    let mut m = FeatureMap::new();
    m.insert("entropy".to_string(), 7.1);
    m.insert("edge_ratio".to_string(), 0.52);
    m.insert("brightness".to_string(), 0.31);
    m.insert("local_peak".to_string(), 0.94);
    m.insert("local_variance".to_string(), 0.0);
    m.insert("lower_advantage".to_string(), 1.4);
    m.insert("motion".to_string(), 18.0);
    m.insert("contour_count".to_string(), 12.5);
    m.insert("contour_area_variance".to_string(), 0.0);
    m.insert("contour_area_cv".to_string(), 1.2);
    m.insert("color_richness".to_string(), 0.86);
    m
}

fn main() {
    let frame = sample_frame();

    match scheduler_core::analyze_frame(&frame) {
        Ok(decision) => {
            println!("=== analyze_frame 结果 ===");
            println!("score  = {:.4}", decision.score);
            println!("action = {}", decision.action.as_str());
            println!("--- 9 项评分分量 ---");
            for (name, value, weight, contribution) in decision.breakdown.terms() {
                println!("  {name:<18} 值={value:.4}  权重={weight:.2}  贡献={contribution:.4}");
            }
            println!("--- 亮度惩罚 ---");
            println!("  brightness_penalty = {:.4}", decision.breakdown.brightness_penalty);
        }
        Err(e) => {
            eprintln!("analyze_frame 失败: {e}");
            std::process::exit(1);
        }
    }
}