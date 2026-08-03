// handlers.rs - Web 路由处理器
// 负责处理前端发起的 HTTP 请求（首页展示、图片上传与处理）
// 该模块将请求、特征提取、评估、可视化串联起来，返回 JSON 响应

use axum::{
    extract::Multipart,      // 解析 multipart/form-data 格式的文件上传
    response::{Html, IntoResponse, Json}, // 响应类型：HTML、JSON 等
};
use base64::{engine::general_purpose::STANDARD, Engine}; // Base64 编码
use image::imageops::FilterType; // 图像缩放算法（最近邻）
use crate::{
    evaluator,      // 价值评估模块
    features,       // 特征提取模块
    types::UploadResponse, // 统一响应结构
    visualization,  // 可视化标注图生成模块
};
use std::io::Cursor; // 内存中的读写指针，用于将图片编码为 JPEG

/// 首页处理器
/// 返回嵌入在二进制文件中的 HTML 模板（通过 include_str! 编译时嵌入）
pub async fn index() -> Html<&'static str> {
    // include_str! 会将 templates/index.html 的内容在编译时读入字符串
    Html(include_str!("../templates/index.html"))
}

/// 图片上传处理器
/// 接收前端上传的图片，执行特征提取、价值评估、生成可视化标注图，
/// 最后以 JSON 格式返回所有结果（含 Base64 图片）
pub async fn upload(mut multipart: Multipart) -> impl IntoResponse {
    // --- 1. 解析 multipart，提取文件数据 ---
    // 初始化文件数据容器（Option<Vec<u8>>）
    let mut file_data = None;

    // 迭代 multipart 中的字段，查找 name="file" 的字段
    while let Ok(Some(field)) = multipart.next_field().await {
        // 检查字段名称是否为 "file"
        if field.name() == Some("file") {
            // 尝试读取字段的二进制数据（图片字节）
            if let Ok(data) = field.bytes().await {
                file_data = Some(data.to_vec()); // 保存到容器中
                break; // 找到第一个文件后退出循环
            }
        }
    }

    // --- 2. 检查是否成功获取到文件 ---
    let bytes = match file_data {
        Some(b) => b,        // 有文件数据，继续
        None => {
            // 无文件或读取失败，返回 JSON 错误信息
            return Json(serde_json::json!({ "error": "未找到文件或读取失败" }));
        }
    };

    // --- 3. 解码图片（支持 JPEG/PNG/BMP/WebP 等）---
    let img = match image::load_from_memory(&bytes) {
        Ok(img) => img,      // 解码成功，返回 DynamicImage
        Err(e) => {
            // 解码失败，返回错误信息
            return Json(serde_json::json!({ "error": format!("图片解码失败: {}", e) }));
        }
    };

    // --- 4. 调用特征提取模块，计算 7 维特征 ---
    let feature_map = features::extract_features(&img);

    // --- 5. 调用价值评估模块，计算价值分和决策动作 ---
    let evaluation = evaluator::evaluate(&feature_map);

    // --- 6. 生成可视化标注图 ---
    // 6a. 获取边缘图（用于标注）
    let edge_map = features::get_edge_map(&img);
    // 6b. 根据特征和边缘图绘制标注图（绿框、黄线等），返回 JPEG 字节
    let vis_bytes = visualization::generate_visualization(&edge_map, &feature_map);

    // --- 7. 生成原图缩略图（Base64 编码，便于前端直接显示）---
    // 缩放到 128x128 以减少传输量
    let thumb = image::imageops::resize(&img, 128, 128, FilterType::Nearest);
    let mut thumb_buf = Vec::new();          // 内存缓冲区
    let mut cursor = Cursor::new(&mut thumb_buf); // 包装为可 Seek 的写入器
    // 以 JPEG 格式（质量 85）写入缩略图
    let _ = thumb.write_to(&mut cursor, image::ImageOutputFormat::Jpeg(85));
    let original_base64 = STANDARD.encode(&thumb_buf); // 编码为 Base64 字符串

    // --- 8. 标注图也编码为 Base64 ---
    let vis_base64 = STANDARD.encode(&vis_bytes);

    // --- 9. 构建统一的响应结构 ---
    let response = UploadResponse {
        // 特征响应
        features: crate::types::FeatureResponse {
            entropy: feature_map["entropy"],
            edge_ratio: feature_map["edge_ratio"],
            brightness: feature_map["brightness"],
            local_peak: feature_map["local_peak"],
            local_variance: feature_map["local_variance"],
            lower_advantage: feature_map["lower_advantage"],
            motion: feature_map["motion"],
        },
        // 评估响应（分数 + 动作）
        evaluation,
        // 原图缩略图 Base64
        original_image: original_base64,
        // 可视化标注图 Base64
        visualized_image: vis_base64,
    };

    // --- 10. 将响应序列化为 JSON 并返回 ---
    Json(serde_json::to_value(&response).unwrap())
}