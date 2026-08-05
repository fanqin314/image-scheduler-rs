// handlers.rs - Web 路由处理器
// 负责处理前端发起的 HTTP 请求（首页展示、图片上传与处理）
// 该模块将请求、特征提取、评估、可视化串联起来，返回 JSON 响应

use axum::{
    extract::{Multipart, State},
    response::{Html, IntoResponse, Json},
    Json as AxumJson,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::imageops::FilterType;
use std::sync::{Arc, Mutex};
use crate::{
    config,
    config::HalfRegion,
    evaluator,
    features,
    types::{AnalyzeFrameRequest, AnalyzeFrameResponse, EvaluationResponse, FeatureResponse, UploadResponse},
    visualization,
};
use std::io::Cursor;

/// 首页处理器
/// 返回嵌入在二进制文件中的 HTML 模板（通过 include_str! 编译时嵌入）
pub async fn index() -> Html<&'static str> {
    // include_str! 会将 templates/index.html 的内容在编译时读入字符串
    Html(include_str!("../templates/index.html"))
}

/// 图片上传处理器
/// 接收前端上传的图片，执行特征提取、价值评估、生成可视化标注图，
/// 最后以 JSON 格式返回所有结果（含 Base64 图片）
pub async fn upload(State(region): State<Arc<Mutex<HalfRegion>>>, mut multipart: Multipart) -> impl IntoResponse {
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

    // --- 4. 调用特征提取模块，计算 10 维特征 ---
    let region = *region.lock().unwrap();
    let feature_map = features::extract_features(&img, region);

    // --- 5. 调用价值评估模块，计算价值分和决策动作 ---
    let evaluation = evaluator::evaluate(&feature_map);

    // --- 6. 生成可视化标注图 ---
    // 6a. 获取边缘图（用于标注）
    let edge_map = features::get_edge_map(&img);
    // 6b. 生成灰度缩略图作为标注背景
    let gray_thumb_raw = image::imageops::resize(&img, 128, 128, FilterType::Nearest);
    let gray_thumb = image::DynamicImage::from(gray_thumb_raw).into_luma8();
    // 6c. 根据特征和边缘图绘制标注图，返回 JPEG 字节
    let vis_bytes = visualization::generate_visualization(&gray_thumb, &edge_map, &feature_map);

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
            // === 新增三个特征（现在从 feature_map 读取真实值） ===
            contour_count: feature_map["contour_count"],
            contour_area_variance: feature_map["contour_area_variance"],
            color_richness: feature_map["color_richness"],
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

/// 视频上传处理器
/// 接收前端上传的视频文件，逐帧提取特征、评估决策，
/// 返回每帧结果 + 摘要统计
pub async fn upload_video(State(region): State<Arc<Mutex<HalfRegion>>>, mut multipart: Multipart) -> impl IntoResponse {
    // --- 1. 解析 multipart，提取视频文件 ---
    let mut file_data = None;

    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            if let Ok(data) = field.bytes().await {
                file_data = Some(data.to_vec());
                break;
            }
        }
    }

    let bytes = match file_data {
        Some(b) => b,
        None => {
            return Json(serde_json::json!({ "error": "未找到视频文件或读取失败" }));
        }
    };

    // --- 2. 写入临时文件（ffmpeg 需要文件路径） ---
    let temp_input = match tempfile::Builder::new()
        .suffix(".mp4")
        .tempfile()
    {
        Ok(f) => f,
        Err(e) => {
            return Json(serde_json::json!({ "error": format!("创建临时文件失败: {}", e) }));
        }
    };

    use std::io::Write;
    {
        let mut f = temp_input.as_file();
        if let Err(e) = f.write_all(&bytes) {
            return Json(serde_json::json!({ "error": format!("写入临时文件失败: {}", e) }));
        }
    }

    let temp_path = temp_input.path().to_path_buf();

    // --- 3. 调 video 模块解码视频 ---
    let video_frames = match crate::video::decode_video(&temp_path) {
        Ok(v) => v,
        Err(e) => {
            return Json(serde_json::json!({ "error": format!("视频解码失败: {}", e) }));
        }
    };

    if video_frames.frames.is_empty() {
        return Json(serde_json::json!({ "error": "视频未提取到任何帧" }));
    }

    // --- 4. 两阶段分析：轻量检测（串行）+ 关键帧完整分析（并行） ---
    // 第一阶段用低分辨率缩略图快速筛出"值得分析的帧"（关键帧），
    // 第二阶段只对关键帧做完整 10 维特征提取，且用 rayon 并行加速，
    // 避免对每一帧都做完整分析导致耗时过长。
    let n = video_frames.frames.len();

    // Phase 1: 串行轻量检测，收集关键帧索引
    let mut light_cache: Vec<(f64, f64, bool)> = Vec::with_capacity(n); // (brightness, motion, is_keyframe)
    let mut prev_gray: Option<image::GrayImage> = None;
    let mut last_brightness = 0.0;

    for (i, (_, img)) in video_frames.frames.iter().enumerate() {
        let small = image::imageops::resize(
            &img.to_luma8(),
            config::LIGHT_RES, config::LIGHT_RES,
            image::imageops::FilterType::Nearest,
        );
        let total_px = (config::LIGHT_RES * config::LIGHT_RES) as f64;
        let brightness = small.pixels().map(|p| p[0] as f64).sum::<f64>() / total_px / 255.0;
        let motion = match &prev_gray {
            Some(p) => {
                let diff: u64 = p.pixels().zip(small.pixels())
                    .map(|(a, b)| (a[0] as i32 - b[0] as i32).unsigned_abs() as u64)
                    .sum();
                diff as f64 / total_px
            }
            None => 0.0,
        };
        let is_kf = i == 0
            || (i % config::FORCE_INTERVAL == 0)
            || motion > config::MOTION_THRESHOLD
            || (brightness - last_brightness).abs() > config::BRIGHTNESS_CHANGE;
        if is_kf { last_brightness = brightness; }
        light_cache.push((brightness, motion, is_kf));
        prev_gray = Some(small);
    }

    // Phase 2: 并行处理关键帧（region 需在闭包外提取，避免跨线程锁竞争）
    let region = *region.lock().unwrap();
    let kf_indices: Vec<usize> = light_cache.iter()
        .enumerate()
        .filter(|(_, (_, _, is_kf))| *is_kf)
        .map(|(i, _)| i)
        .collect();

    use rayon::prelude::*;
    let kf_results: Vec<(usize, FeatureResponse, EvaluationResponse)> = kf_indices
        .par_iter()
        .map(|&i| {
            let (_timestamp, img) = &video_frames.frames[i];
            let prev_luma = if i > 0 {
                video_frames.frames[i - 1].1.to_luma8()
            } else {
                video_frames.frames[0].1.to_luma8()
            };
            let feature_map = crate::features::extract_features_with_motion(img, Some(&prev_luma), region);
            let evaluation = crate::evaluator::evaluate(&feature_map);
            let fr = FeatureResponse {
                entropy: feature_map["entropy"],
                edge_ratio: feature_map["edge_ratio"],
                brightness: feature_map["brightness"],
                local_peak: feature_map["local_peak"],
                local_variance: feature_map["local_variance"],
                lower_advantage: feature_map["lower_advantage"],
                motion: feature_map["motion"],
                contour_count: feature_map["contour_count"],
                contour_area_variance: feature_map["contour_area_variance"],
                color_richness: feature_map["color_richness"],
            };
            // 传回 frame index + 结果
            (i, fr, evaluation)
        })
        .collect();

    // 构建关键帧查询表
    use std::collections::HashMap;
    let kf_map: HashMap<usize, (FeatureResponse, EvaluationResponse)> = kf_results
        .iter()
        .map(|(i, fr, ev)| (*i, (fr.clone(), ev.clone())))
        .collect();

    // Phase 3: 组装最终结果
    let mut results: Vec<crate::types::VideoFrameResult> = Vec::with_capacity(n);
    let (mut total_score, mut cloud_count, mut local_count, mut drop_count) = (0.0, 0, 0, 0);
    let (mut max_entropy, mut max_motion) = (0.0, 0.0);
    let mut last_kf_res: Option<(FeatureResponse, EvaluationResponse)> = None;
    let mut prev_action: Option<String> = None;  // 滞后防抖

    for i in 0..n {
        let (timestamp, _img) = &video_frames.frames[i];
        let (brightness, motion, is_keyframe) = light_cache[i];

        if is_keyframe {
            let (fr, mut ev) = kf_map[&i].clone();
            // 滞后防抖（在此做而非并行求值中，因 rayon 无序无法传 prev_action）
            // 效果：消除相邻帧评分 0.001 波动导致的 CLOUD↔LOCAL 决策翻转
            if let Some(ref prev) = prev_action {
                if prev == "CLOUD" && ev.action != "CLOUD" && ev.score >= config::THRESHOLD_CLOUD - config::HYSTERESIS {
                    ev.action = "CLOUD".to_string();
                }
            }
            prev_action = Some(ev.action.clone());
            total_score += ev.score;
            match ev.action.as_str() {
                "CLOUD" => cloud_count += 1,
                "LOCAL" => local_count += 1,
                _ => drop_count += 1,
            }
            if fr.entropy > max_entropy { max_entropy = fr.entropy; }
            if fr.motion > max_motion { max_motion = fr.motion; }
            last_kf_res = Some((fr.clone(), ev.clone()));
            results.push(crate::types::VideoFrameResult {
                frame_index: i,
                timestamp_secs: *timestamp,
                is_keyframe: true,
                features: fr,
                evaluation: ev,
            });
        } else {
            let (feat, eval) = last_kf_res.as_ref().unwrap().clone();
            total_score += eval.score;
            match eval.action.as_str() {
                "CLOUD" => cloud_count += 1,
                "LOCAL" => local_count += 1,
                _ => drop_count += 1,
            }
            results.push(crate::types::VideoFrameResult {
                frame_index: i,
                timestamp_secs: *timestamp,
                is_keyframe: false,
                features: FeatureResponse { brightness, motion, ..feat },
                evaluation: eval,
            });
        }
    }

    let n = results.len() as f64;
    let summary = crate::types::VideoSummary {
        cloud_count,
        local_count,
        drop_count,
        avg_score: if n > 0.0 { total_score / n } else { 0.0 },
        max_entropy,
        max_motion,
    };

    let response = crate::types::VideoUploadResponse {
        total_frames: results.len(),
        fps: video_frames.fps,
        duration_secs: video_frames.total_duration,
        frames: results,
        summary,
    };

    Json(serde_json::to_value(&response).unwrap())
}

/// 实时帧分析处理器（Chrome 扩展用）
/// 接收当前帧 + 上一帧（可选），即时返回特征与评估
pub async fn analyze_frame(
    State(region): State<Arc<Mutex<HalfRegion>>>,
    AxumJson(payload): AxumJson<AnalyzeFrameRequest>,
) -> impl IntoResponse {
    // --- 辅助：从 base64 解码为 DynamicImage ---
    fn decode_base64_image(b64: &str) -> Result<image::DynamicImage, String> {
        let b64 = if b64.contains("base64,") {
            b64.split("base64,").nth(1).unwrap_or(b64)
        } else {
            b64
        };
        let bytes = STANDARD.decode(b64).map_err(|e| format!("Base64 解码失败: {}", e))?;
        image::load_from_memory(&bytes).map_err(|e| format!("图片解码失败: {}", e))
    }

    // --- 1. 解码当前帧 ---
    let cur_img = match decode_base64_image(&payload.image_base64) {
        Ok(img) => img,
        Err(e) => return Json(serde_json::json!({ "error": e })),
    };

    // --- 2. 解码上一帧（如果有），提取灰度图 ---
    let prev_gray = match &payload.prev_image_base64 {
        Some(b64) => match decode_base64_image(b64) {
            Ok(prev) => {
                let prev_small = image::imageops::resize(
                    &prev.to_luma8(),
                    config::THUMBNAIL_SIZE,
                    config::THUMBNAIL_SIZE,
                    FilterType::Nearest,
                );
                Some(prev_small)
            }
            Err(_) => None,
        },
        None => None,
    };

    // --- 3. 特征提取 ---
    let region = *region.lock().unwrap();
    let feature_map = features::extract_features_with_motion(
        &cur_img,
        prev_gray.as_ref(),
        region,
    );

    // --- 4. 评估 ---
    let evaluation = evaluator::evaluate(&feature_map);

    // --- 4.5 生成标注图（原图灰度 + 边缘 + 轮廓 + 峰值窗） ---
    let edge_map = features::get_edge_map(&cur_img);
    let gray_thumb = cur_img.to_luma8();
    let vis_bytes = visualization::generate_visualization(&gray_thumb, &edge_map, &feature_map);
    let vis_b64 = STANDARD.encode(&vis_bytes);

    // --- 5. 组装响应 ---
    let response = AnalyzeFrameResponse {
        features: FeatureResponse {
            entropy: feature_map["entropy"],
            edge_ratio: feature_map["edge_ratio"],
            brightness: feature_map["brightness"],
            local_peak: feature_map["local_peak"],
            local_variance: feature_map["local_variance"],
            lower_advantage: feature_map["lower_advantage"],
            motion: feature_map["motion"],
            contour_count: feature_map["contour_count"],
            contour_area_variance: feature_map["contour_area_variance"],
            color_richness: feature_map["color_richness"],
        },
        evaluation: EvaluationResponse {
            score: evaluation.score,
            action: evaluation.action,
        },
        visualized_image: Some(vis_b64),
    };

    Json(serde_json::to_value(&response).unwrap())
}

// ============================================================
// /set-region — 运行时切换关注的半区方向
// ============================================================
/// POST /set-region  body: {"region":"lower"}  (lower/upper/left/right)
/// 下一次分析即生效，无需重启服务。
pub async fn set_region(
    State(region): State<Arc<Mutex<HalfRegion>>>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> impl IntoResponse {
    let val = body.get("region").and_then(|v| v.as_str()).unwrap_or("lower");
    let new = match val {
        "upper" => HalfRegion::Upper,
        "left"  => HalfRegion::Left,
        "right" => HalfRegion::Right,
        _       => HalfRegion::Lower,
    };
    *region.lock().unwrap() = new;
    Json(serde_json::json!({"ok": true, "region": val}))
}