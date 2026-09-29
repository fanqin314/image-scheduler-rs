//! 多车联调编排器：派生 N 个 `vehicle` 子进程，跑一段时长后汇总丢包率报告。
//!
//! 用法：
//! ```text
//! simulator --count 3 --base-port 9000 --rate 100 --duration-ms 30000 [--hub-port 9100] [--report out.md]
//! ```
//!
//! 说明：
//! - N 辆车互为 peer（全连通环），每车每秒广播 `1000/rate` 条状态。
//! - 若指定 `--hub-port`，每辆车还会把广播打到该端口（可视化面板监听处，见主 crate `/v2v`）。
//! - 每个 vehicle 进程运行 `duration-ms` 后在 stdout 输出 `RESULT <json>`，
//!   simulator 聚合所有车辆的 lost / received，计算并打印丢包率。

use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone())
}

fn sibling_bin(name: &str) -> std::path::PathBuf {
    let mut p = std::env::current_exe().expect("获取当前可执行路径失败");
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.into() };
    p.set_file_name(exe);
    p
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let count: usize = arg(&args, "--count").unwrap_or_else(|| "3".into()).parse()?;
    let base: u16 = arg(&args, "--base-port").unwrap_or_else(|| "9000".into()).parse()?;
    let rate_ms: u64 = arg(&args, "--rate").unwrap_or_else(|| "100".into()).parse()?;
    let dur_ms: u64 = arg(&args, "--duration-ms").unwrap_or_else(|| "60000".into()).parse()?;
    let hub_port: u16 = arg(&args, "--hub-port").unwrap_or_else(|| "9100".into()).parse()?;
    let report: Option<String> = arg(&args, "--report");

    println!(">> 启动 {count} 车联调：base={base}, 广播间隔={rate_ms}ms, 时长={dur_ms}ms");
    let vehicle_bin = sibling_bin("vehicle");
    if !vehicle_bin.exists() {
        return Err(anyhow::anyhow!(
            "找不到 vehicle 可执行文件 {}（请先 cargo build -p v2v --bins）",
            vehicle_bin.display()
        ));
    }

    let ids: Vec<String> = (0..count).map(|i| format!("car-{i}")).collect();
    let mut children: Vec<Child> = Vec::new();

    for i in 0..count {
        let port = base + i as u16;
        let mut peers: Vec<String> = (0..count)
            .filter(|&j| j != i)
            .map(|j| format!("127.0.0.1:{}", base + j as u16))
            .collect();
        if hub_port > 0 {
            peers.push(format!("127.0.0.1:{hub_port}")); // 广播到可视化面板
        }
        let known: Vec<String> = (0..count).filter(|&j| j != i).map(|j| format!("car-{j}")).collect();

        let mut cmd = Command::new(&vehicle_bin);
        cmd.args([
            "--id", &ids[i],
            "--listen-port", &port.to_string(),
            "--peers", &peers.join(","),
            "--known", &known.join(","),
            "--rate", &rate_ms.to_string(),
            "--run-ms", &dur_ms.to_string(),
        ]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
        let child = cmd.spawn().expect("启动 vehicle 子进程失败");
        println!("    spawn {:>6}:{:<5} peers=[{}]", ids[i], port, peers.join(" , "));
        children.push(child);
    }

    // 等待全部子进程退出（统一时长）
    let started = std::time::Instant::now();
    let mut reserved: Vec<Option<String>> = Vec::new();
    for ch in children.iter_mut() {
        if let Some(mut so) = ch.stdout.take() {
            let mut lines = String::new();
            use std::io::Read;
            let _ = so.read_to_string(&mut lines);
            reserved.push(Some(lines));
        } else {
            reserved.push(None);
        }
        let _ = ch.wait();
    }
    println!(">> 联调结束，耗时 {:.1}s", started.elapsed().as_secs_f64());

    // 解析 RESULT
    let mut results = Vec::new();
    for lines in reserved.into_iter().flatten() {
        for line in lines.lines() {
            if let Some(json) = line.strip_prefix("RESULT ") {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(json) {
                    results.push(v);
                }
            }
        }
    }

    // 聚合丢包率
    let mut tot_received = 0u64;
    let mut tot_lost = 0u64;
    let mut tot_lat = 0.0f64;
    let mut rows = String::new();
    for r in &results {
        let recv = r["received"].as_u64().unwrap_or(0);
        let lost = r["lost"].as_u64().unwrap_or(0);
        let lat = r["avg_latency_ms"].as_f64().unwrap_or(0.0);
        tot_received += recv;
        tot_lost += lost;
        tot_lat += lat;
        let loss = if recv + lost > 0 {
            100.0 * lost as f64 / (recv + lost) as f64
        } else {
            0.0
        };
        rows.push_str(&format!(
            "| {:<16} | {:>6} | {:>8} | {:>6} | {:>8.2} | {:>7.3}% |\n",
            r["id"].as_str().unwrap_or("?"),
            recv,
            lost,
            r["ooo"].as_u64().unwrap_or(0),
            r["max_latency_ms"].as_f64().unwrap_or(0.0),
            loss
        ));
    }
    let n = results.len().max(1);
    let overall_loss = if tot_received + tot_lost > 0 {
        100.0 * tot_lost as f64 / (tot_received + tot_lost) as f64
    } else {
        0.0
    };
    let avg_lat = tot_lat / n as f64;

    println!();
    println!("== 丢包率报告（阶段1 验收：丢包率<1%，延迟<10ms）==");
    println!("| 车辆 | 收到 | 丢失 | 乱序 | 最大延迟ms | 丢包率 |");
    println!("|-----|------|------|------|-----------|--------|");
    println!("{rows}| **合计** | **{tot_received}** | **{tot_lost}** | | **{avg_lat:.2}avg** | **{overall_loss:.3}%** |");
    println!();
    println!("→ 总丢包率 {overall_loss:.3}%（目标 <1%）| 平均最大延迟 {avg_lat:.2}ms（目标 <10ms）");

    if let Some(path) = report {
        let md = format!(
            "# 阶段1 V2V 通信丢包率报告\n\n\
             - 车辆数：{count}，广播间隔：{rate_ms}ms，时长：{dur_ms}ms\n\
             - 日期：{}\n\n\
             | 车辆 | 收到 | 丢失 | 乱序 | 最大延迟ms | 丢包率 |\n\
             |-----|------|------|------|-----------|--------|\n\
             {rows}\
             | **合计** | **{tot_received}** | **{tot_lost}** | | **{avg_lat:.2}avg** | **{overall_loss:.3}%** |\n\n\
             ## 结论\n\
             - 总丢包率 **{overall_loss:.3}%**（目标 <1%）：{}\n\
             - 延迟 **{avg_lat:.2}ms**（目标 <10ms）：{}\n",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            if overall_loss < 1.0 { "✅ 达标" } else { "⚠️ 未达标" },
            if avg_lat < 10.0 { "✅ 达标" } else { "⚠️ 未达标" },
        );
        std::fs::write(&path, md)?;
        println!("报告已写入: {path}");
    }

    let _ = Duration::from_millis(0);
    Ok(())
}