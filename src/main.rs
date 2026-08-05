//! 智能图像调度系统 — 服务入口
//!
//! 启动 axum HTTP 服务（127.0.0.1:5000），注册路由并自动打开浏览器。
//! 各模块职责见下方 mod 声明。

// ============================================================
// 模块声明
// ============================================================
mod config;         // 全局可调参数中心（特征/权重/阈值/采样）
mod types;          // 公共数据结构
mod features;       // 特征提取核心（熵、Sobel、滑动窗口、轮廓）
mod evaluator;      // 价值评估（打分 + 决策 + 滞后防抖）
mod visualization;  // 可视化标注图生成
mod handlers;       // Web 路由处理器（首页 / 上传 / 实时分析）
mod video;          // 视频解码（ffmpeg 逐帧提取）

// ============================================================
// 外部依赖导入
// ============================================================
use axum::{
    extract::DefaultBodyLimit,  // 请求体大小限制配置
    routing::{get, post},  // get: 处理 GET 请求, post: 处理 POST 请求
    Router,                // 路由构建器，把 URL 路径和处理器函数绑定
};
use tower_http::trace::TraceLayer;  // 日志中间件，自动打印请求日志

// 运行时共享状态：用户可通过 /set-region 端点切换关注的半区方向
use std::sync::{Arc, Mutex};
use crate::config::HalfRegion;

// 主函数入口
// 先同步执行 ffmpeg 下载（避免 reqwest::blocking 与 tokio 运行时冲突），
// 再进入 tokio 异步运行时启动 HTTP 服务。
// ============================================================
fn main() {
    println!("\u{1f980} 启动 Rust 图像调度服务...");
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

async fn async_main() {

    // ---------- 1. 打印启动信息 ----------

    // ---------- 2. 构建路由 ----------
    // Router::new() 创建一个空路由
    // .route("/", get(handlers::index))
    //   - 当用户访问根路径 "/" 时，使用 GET 方法，调用 handlers::index
    //   - handlers::index 返回前端 HTML 页面
    // .route("/upload", post(handlers::upload))
    //   - 当用户访问 "/upload" 时，使用 POST 方法，调用 handlers::upload
    //   - handlers::upload 处理图片上传，返回 JSON 格式的特征和决策
    // .layer(TraceLayer::new_for_http())
    //   - 添加日志中间件，每个请求都会打印方法、路径、状态码和耗时
    // 运行时共享状态：关注的半区方向，/set-region 端点可切换
    let region = Arc::new(Mutex::new(config::DEFAULT_REGION));
    let app = Router::new()
        .route("/", get(handlers::index))
        .route("/upload", post(handlers::upload))
        .route("/upload-video", post(handlers::upload_video))
        .route("/analyze-frame", post(handlers::analyze_frame))
        .route("/set-region", post(handlers::set_region))
        .with_state(region)
        .layer(DefaultBodyLimit::max(256 * 1024 * 1024))  // 256MB，匹配前端提示
        .layer(TraceLayer::new_for_http());

    // ---------- 3. 绑定 TCP 端口 ----------
    // 尝试在 127.0.0.1:5000 上监听 HTTP 请求
    // 如果端口被占用（比如 Python 版的服务没关），会返回 Err
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:5000").await {
        Ok(l) => {
            // 绑定成功
            l
        }
        Err(e) => {
            // 绑定失败：打印错误信息，并等待用户按回车键退出
            // 这样双击 exe 时窗口不会闪退，用户能看到错误
            println!("❌ 端口绑定失败: {}", e);
            println!("按回车键退出...");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input).unwrap();
            return;  // 提前退出 main
        }
    };
    println!("✅ Web 服务已启动: http://127.0.0.1:5000");

    // ---------- 4. 启动 HTTP 服务 ----------
    // axum::serve 启动一个 HTTP/1.1 服务器，监听 listener 上的连接
    // app 作为请求处理器，所有请求都会进入路由系统
    // 如果服务运行中出错（比如端口被意外关闭），打印错误并等待按键退出
    if let Err(e) = axum::serve(listener, app).await {
        println!("❌ 服务运行失败: {}", e);
        println!("按回车键退出...");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
    }
}