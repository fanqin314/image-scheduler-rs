//! 智能图像调度系统 — 服务入口
//!
//! 启动 axum HTTP 服务（127.0.0.1:5000），注册路由并自动打开浏览器。
//! 各模块职责见下方 mod 声明。
//!
//! 除默认的服务模式外，还提供 `bench` 子命令：批量评测一批图片，
//! 输出特征诊断报告与自适应阈值推荐（见 `diagnostics` 模块）。

// ============================================================
// 模块声明（核心算法已抽成独立 crate scheduler-core）
// ============================================================
mod visualization;  // 可视化标注图生成
mod handlers;       // Web 路由处理器（首页 / 上传 / 实时分析）
mod video;          // 视频解码（ffmpeg 逐帧提取）
mod diagnostics;    // 特征诊断与批量评测（bench 子命令）
mod fleet;          // 车组 V2V 可视化面板（阶段1）

use scheduler_core::config; // 特征/权重/阈值参数（含 HalfRegion、DEFAULT_REGION）

// ============================================================
// 外部依赖导入
// ============================================================
use axum::{
    extract::DefaultBodyLimit, // 请求体大小限制配置
    routing::{get, post},      // get: 处理 GET 请求, post: 处理 POST 请求
    Router,                    // 路由构建器，把 URL 路径和处理器函数绑定
};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tower_http::trace::TraceLayer; // 日志中间件，自动打印请求日志

// ============================================================
// 运行时共享状态
// ============================================================

/// 全服务共享状态（axum `State` 要求 `Clone`）。
///
/// - `region`：关注的半区方向，`/set-region` 端点可运行时切换。
/// - `threshold`：自适应阈值器，仅当 `config::ADAPTIVE_THRESHOLD` 开启时
///   参与决策；关闭时恒回退到 `config::THRESHOLD_CLOUD`。
#[derive(Clone)]
pub struct AppState {
    pub region: Arc<Mutex<config::HalfRegion>>,
    pub threshold: Arc<Mutex<diagnostics::AdaptiveThreshold>>,
    pub fleet: Option<Arc<fleet::Fleet>>,
}

// ============================================================
// 入口
// ============================================================

fn main() {
    let args: Vec<String> = std::env::args().collect();

    // ---- 子命令分发 ----
    if args.len() >= 2 && args[1] == "bench" {
        bench_cli(&args[2..]);
        return;
    }
    if args.len() >= 2 {
        // help 或未知子命令
        if matches!(args[1].as_str(), "help" | "--help" | "-h") {
            print_usage();
        } else {
            eprintln!("未知子命令: {}", args[1]);
            print_usage();
        }
        return;
    }

    // ---- 无参数：默认启动 Web 服务 ----
    println!("🐀 启动 Rust 图像调度服务...");
    if let Err(e) = crate::video::ensure_ffmpeg() {
        eprintln!("ffmpeg 初始化失败: {}，视频功能将不可用", e);
    }
    // 使用当前线程运行时，避免多线程运行时与 reqwest::blocking 的兼容问题
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async_main());
}

/// 批量评测子命令。
///
/// 遍历给定的图片/目录，跑完整特征→评分流水线，输出诊断报告。
/// 不启动 HTTP 服务、不下载 ffmpeg（纯 CPU 计算，便于 CI / 脚本化）。
fn bench_cli(args: &[String]) {
    let roots: Vec<PathBuf> = args
        .iter()
        .filter(|a| !a.starts_with("--"))
        .map(|a| PathBuf::from(a.as_str()))
        .collect();

    if roots.is_empty() {
        eprintln!("用法: image-scheduler-rs bench <图片/目录> [<更多路径>...]");
        return;
    }

    let mut all: Vec<PathBuf> = Vec::new();
    for root in &roots {
        match diagnostics::collect_image_paths(root) {
            Ok(mut p) => all.append(&mut p),
            Err(e) => {
                eprintln!("{}: {}", root.display(), e);
                std::process::exit(1);
            }
        }
    }
    if all.is_empty() {
        eprintln!("未找到任何图片（支持 jpg/jpeg/png/bmp/webp/gif）");
        std::process::exit(1);
    }

    let opts = diagnostics::BenchOptions::default();
    match diagnostics::run_bench(&all, &opts) {
        Ok(report) => diagnostics::print_report(&report),
        Err(e) => {
            eprintln!("评测失败: {}", e);
            std::process::exit(1);
        }
    }
}

fn print_usage() {
    println!("智能图像调度系统");
    println!();
    println!("用法:");
    println!("  image-scheduler-rs                 启动 Web 服务 (http://127.0.0.1:5000)");
    println!("  image-scheduler-rs bench <路径>...  批量评测图片/目录，输出特征诊断与阈值推荐");
    println!("  image-scheduler-rs help            显示本帮助");
    println!();
    println!("bench 示例:");
    println!("  image-scheduler-rs bench ./samples");
    println!("  image-scheduler-rs bench a.jpg b.png ./frames");
}

// ============================================================
// HTTP 服务
// ============================================================

async fn async_main() {
    // 运行时共享状态：关注的半区方向 + 自适应阈值器 + 车组面板监听节点
    let fleet = fleet::Fleet::spawn(fleet::HUB_PORT).await;
    if fleet.is_none() {
        eprintln!("⚠️ UDP {} 端口被占用，V2V 面板不可用", fleet::HUB_PORT);
    }
    let state = AppState {
        region: Arc::new(Mutex::new(config::DEFAULT_REGION)),
        threshold: Arc::new(Mutex::new(diagnostics::AdaptiveThreshold::from_config())),
        fleet,
    };

    // Router::new() 创建一个空路由，把 URL 路径和处理器函数绑定
    let mut app = Router::new()
        .route("/", get(handlers::index))
        .route("/upload", post(handlers::upload))
        .route("/upload-video", post(handlers::upload_video))
        .route("/analyze-frame", post(handlers::analyze_frame))
        .route("/set-region", post(handlers::set_region))
        .route("/v2v", get(fleet::panel_page));
    // 车队面板 WebSocket：仅在监听节点可用时注册（它需要 fleet state）
    if state.fleet.is_some() {
        app = app.route("/ws/v2v", get(fleet::ws_handler));
    }
    let app = app
        .with_state(state)
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024)) // 256MB，匹配前端提示
        .layer(TraceLayer::new_for_http());

    // 尝试在 127.0.0.1:5000 上监听 HTTP 请求
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:5000").await {
        Ok(l) => l,
        Err(e) => {
            // 绑定失败：打印错误信息，并等待用户按回车键退出，避免双击 exe 时窗口闪退
            println!("❌ 端口绑定失败: {}", e);
            println!("按回车键退出...");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            return;
        }
    };
    println!("✅ Web 服务已启动: http://127.0.0.1:5000");

    // 启动 HTTP/1.1 服务器
    if let Err(e) = axum::serve(listener, app).await {
        println!("❌ 服务运行失败: {}", e);
        println!("按回车键退出...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
    }
}
