// ============================================================
// 模块声明
// 告诉 Rust 编译器这些模块在 src/ 目录下的对应文件中
// ============================================================
mod types;          // 公共数据结构（FeatureResponse, EvaluationResponse 等）
mod features;       // 特征提取核心（熵、Sobel、滑动窗口）
mod evaluator;      // 价值评估（打分 + 决策）
mod visualization;  // 可视化标注图生成（绿框、黄线）
mod handlers;       // Web 路由处理器（首页 / 上传接口）

// ============================================================
// 外部依赖导入
// ============================================================
use axum::{
    routing::{get, post},  // get: 处理 GET 请求, post: 处理 POST 请求
    Router,                // 路由构建器，把 URL 路径和处理器函数绑定
};
use tower_http::trace::TraceLayer;  // 日志中间件，自动打印请求日志

// ============================================================
// 主函数入口
// #[tokio::main] 是 tokio 运行时宏，将 async main 转换为同步入口，
// 并启动一个多线程异步运行时。
// ============================================================
#[tokio::main]
async fn main() {
    // ---------- 1. 打印启动信息 ----------
    println!("🦀 启动 Rust 图像调度服务...");

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
    let app = Router::new()
        .route("/", get(handlers::index))
        .route("/upload", post(handlers::upload))
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

    // ---------- 4. 自动打开浏览器 ----------
    // 调用 Windows 的 cmd 命令，用默认浏览器打开 http://127.0.0.1:5000
    // _ = ... 表示忽略返回结果（即使打开失败也不影响服务运行）
    let _ = std::process::Command::new("cmd")
        .args(&["/C", "start", "http://127.0.0.1:5000"])
        .spawn();

    // ---------- 5. 启动 HTTP 服务 ----------
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