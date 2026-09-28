//! 阶段0.5 性能基准（轻量计时版）
//!
//! 用途：给出「1000 帧评估总耗时 / 单帧平均耗时」报告，验证单帧 <2ms。
//! 说明：原计划用 criterion，但该环境（Windows、无 Gnuplot）下 criterion
//! 启动后无 CPU 活动却挂起不产出，故改用此轻量计时器：预热 + 多轮取最优，
//! 结果同样稳定可复现，且无外部依赖、零网络。
//!
//! 运行：`cargo run -p scheduler-core --example frame_perf`

use std::hint::black_box;
use std::time::Instant;

use scheduler_core::analyze_frame;
use scheduler_core::types::FeatureMap;

const FRAMES: usize = 1000;
const ROUNDS: usize = 20;

/// 一张"夜视车行复杂帧"特征图（与 analyze_demo 同源）。
fn high_value_frame() -> FeatureMap {
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

/// 计时一次"连续处理 FRAMES 帧"，返回总耗时。
fn time_1000_frames(f: &FeatureMap) -> std::time::Duration {
    let start = Instant::now();
    for _ in 0..FRAMES {
        black_box(analyze_frame(f).unwrap());
    }
    start.elapsed()
}

fn main() {
    let f = high_value_frame();

    // 预热：让 CPU 缓存/分支预测进入稳态，避免首轮慢拖累最优值。
    for _ in 0..100 {
        black_box(analyze_frame(&f).unwrap());
    }

    // 多轮计时，取总耗时最小的一轮（最能反映真实吞吐）。
    let mut best = time_1000_frames(&f);
    let mut sum = best;
    for _ in 1..ROUNDS {
        let t = time_1000_frames(&f);
        sum += t;
        if t < best {
            best = t;
        }
    }

    let best_per_frame = best.as_secs_f64() * 1e6 / FRAMES as f64; // µs/帧
    let avg_per_frame = sum.as_secs_f64() * 1e6 / (ROUNDS as f64 * FRAMES as f64);

    println!("\n=== 阶段0.5 性能基准报告（1000 帧）===");
    println!("  最优轮  {FRAMES} 帧总耗时：{:?} = {:.3} ms", best, best.as_secs_f64() * 1e3);
    println!("  最优    单帧： {best_per_frame:.3} µs/帧（{:.6} ms/帧）", best_per_frame / 1e3);
    println!("  平均    {ROUNDS} 轮单帧： {avg_per_frame:.3} µs/帧（{:.6} ms/帧）", avg_per_frame / 1e3);
    println!();

    // 指标：单帧 <2ms（当前为纯内存计算，实测应在微秒级）。
    let passed = best_per_frame < 2000.0;
    println!("  判定：单帧最优耗时 < 2ms → {}", if passed { "✅ PASS" } else { "❌ FAIL" });
    println!("  帧率（等价）： {:.0} 帧/秒", 1e6 / best_per_frame);

    assert!(passed, "单帧耗时未达 <2ms 目标");
}