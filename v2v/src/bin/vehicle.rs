//! 单进程模拟一辆车。
//!
//! 用法：
//! ```text
//! vehicle --id car-a --listen-port 9000 \
//!         --peers 127.0.0.1:9001,127.0.0.1:9002 \
//!         --known car-b,car-c \
//!         [--rate 100] [--run-ms 60000]
//! ```
//!
//! - `--rate`：状态/心跳广播间隔（毫秒），默认 100 → 每秒 10 条
//! - `--run-ms`：运行毫秒数，到点后打印 `RESULT <json>` 到 stdout 并退出（供 simulator 聚合）
//! - 每满 1 秒向 stderr 打印一行实时状态（sent/recv/lost/延迟/离线）

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use v2v::types::{MessagePayload, MsgType, PerceptionSummary, VehicleState};
use v2v::V2vComm;

fn arg(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone())
}

fn parse_peers(s: &str) -> Vec<SocketAddr> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|x| x.parse().expect("非法 peer 地址"))
        .collect()
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let id = arg(&args, "--id").unwrap_or_else(|| "car-0".into());
    let port: u16 = arg(&args, "--listen-port")
        .unwrap_or_else(|| "9000".into())
        .parse()?;
    let peers = arg(&args, "--peers").map(|s| parse_peers(&s)).unwrap_or_default();
    let known: Vec<String> = arg(&args, "--known")
        .map(|s| s.split(',').map(|x| x.to_string()).collect())
        .unwrap_or_default();
    let rate_ms: u64 = arg(&args, "--rate").unwrap_or_else(|| "100".into()).parse()?;
    let run_ms: Option<u64> = arg(&args, "--run-ms").map(|x| x.parse::<u64>().expect("run-ms"));

    let rate = Duration::from_millis(rate_ms);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    rt.block_on(run(id, port, peers, known, rate, run_ms))
}

async fn run(
    id: String,
    port: u16,
    peers: Vec<SocketAddr>,
    known: Vec<String>,
    rate: Duration,
    run_ms: Option<u64>,
) -> anyhow::Result<()> {
    let comm = V2vComm::bind(id.clone(), port, peers).await?;
    if !known.is_empty() {
        comm.set_known_peers(known.clone());
    }
    // 收到消息时打日志（回复一条模拟决策，让车队产生业务流）
    comm.set_handler(|m| match &m.payload {
        MessagePayload::Perception(p) => {
            eprintln!("[{}] 收到 {} 的感知摘要 目标={} 熵={:.2} 复杂比={:.2}",
                "", m.sender, p.objects, p.entropy, p.complex_ratio);
        }
        MessagePayload::Decision(d) => {
            eprintln!("[{}] 收到 {} 的决策 → {:?} score={:.2} 理由={}",
                "", m.sender, d.action, d.score, d.reason);
        }
        _ => {}
    });

    let started = Instant::now();
    let mut tick = 0u64;
    let mut x = 0.0f64;
    loop {
        x += 1.0;
        let st = VehicleState {
            x: (started.elapsed().as_secs_f64() * 0.5).sin() * 20.0 + x % 3.0,
            y: (started.elapsed().as_secs_f64() * 0.3).cos() * 20.0,
            speed: 10.0 + (tick % 30) as f64,
            heading: (started.elapsed().as_secs_f64() * 20.0) % 360.0,
            load: 0.3 + ((tick as f64 * 0.13) % 0.5),
        };
        comm.broadcast(MsgType::State, MessagePayload::State(st)).await;
        tick += 1;

        // 每 1 秒附带一条感知摘要，制造业务流量
        if tick % 10 == 0 {
            comm.broadcast(
                MsgType::Perception,
                MessagePayload::Perception(PerceptionSummary {
                    objects: (tick % 5) as u32 + 1,
                    entropy: 1.0 + (tick as f64 % 7.0),
                    complex_ratio: 0.1 + (tick as f64 % 0.6),
                }),
            )
            .await;
        }

        // 每秒打一行状态
        let step = (1000 / rate.as_millis().max(1)) as u64;
        if tick % step == 0 || tick % 10 == 0 {
            let s = comm.stats();
            let offline: Vec<String> = comm
                .offline_peers(Duration::from_millis(500))
                .into_iter()
                .map(|(i, _)| i)
                .collect();
            eprintln!(
                "[{}] 已运行 {:>5.1}s  sent={:<5} recv={:<5} lost={:<4} 延迟avg={:>7.2}ms 离线=[{}]",
                id,
                started.elapsed().as_secs_f64(),
                s.sent,
                s.received,
                s.lost,
                s.avg_latency_ms(),
                offline.join(",")
            );
        }

        if let Some(ms) = run_ms {
            if started.elapsed().as_millis() as u64 >= ms {
                break;
            }
        }
        tokio::time::sleep(rate).await;
    }

    // 收尾：打印 RESULT 供 simulator 聚合（丢包率报告）
    let s = comm.stats();
    let recv_by = known
        .iter()
        .map(|p| (p, comm.incoming_count(p)))
        .collect::<Vec<_>>();
    let result = serde_json::json!({
        "id": id,
        "up_ms": started.elapsed().as_millis() as u64,
        "sent": s.sent,
        "received": s.received,
        "lost": s.lost,
        "ooo": s.out_of_order,
        "dup": s.duplicated,
        "malformed": s.malformed,
        "avg_latency_ms": s.avg_latency_ms(),
        "max_latency_ms": s.max_latency_ms(),
        "recv_by_peer": recv_by,
    });
    println!("RESULT {}", serde_json::to_string(&result)?);
    Ok(())
}