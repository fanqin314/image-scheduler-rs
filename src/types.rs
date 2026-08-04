// ============================================================
// types.rs - 公共数据结构定义
//
// 本文件定义了整个系统各模块之间传递的数据类型。
// 所有模块（features、evaluator、visualization、handlers）
// 都依赖这些结构体，保持数据格式统一。
// ============================================================

use serde::{Serialize, Deserialize};
use std::collections::HashMap;

// ------------------------------------------------------------
// 1. 特征响应结构体
// 用途：handlers 返回给前端 JSON 时使用的 10 维特征数据
// ------------------------------------------------------------
#[derive(Serialize, Clone)]
pub struct FeatureResponse {
    pub entropy: f64,
    pub edge_ratio: f64,
    pub brightness: f64,
    pub local_peak: f64,
    pub local_variance: f64,
    pub lower_advantage: f64,
    pub motion: f64,
    pub contour_count: f64,
    pub contour_area_variance: f64,
    pub color_richness: f64,
}

// ------------------------------------------------------------
// 2. 价值评估响应结构体
// ------------------------------------------------------------
#[derive(Serialize, Clone)]
pub struct EvaluationResponse {
    pub score: f64,
    pub action: String,
}

// ------------------------------------------------------------
// 3. 单图上传响应结构体
// ------------------------------------------------------------
#[derive(Serialize)]
pub struct UploadResponse {
    pub features: FeatureResponse,
    pub evaluation: EvaluationResponse,
    pub original_image: String,
    pub visualized_image: String,
}

// ------------------------------------------------------------
// 4. 视频帧结果
// ------------------------------------------------------------
#[derive(Serialize)]
pub struct VideoFrameResult {
    pub frame_index: usize,
    pub timestamp_secs: f64,
    pub is_keyframe: bool,
    pub features: FeatureResponse,
    pub evaluation: EvaluationResponse,
}

// ------------------------------------------------------------
// 5. 视频分析摘要
// ------------------------------------------------------------
#[derive(Serialize)]
pub struct VideoSummary {
    pub cloud_count: usize,
    pub local_count: usize,
    pub drop_count: usize,
    pub avg_score: f64,
    pub max_entropy: f64,
    pub max_motion: f64,
}

// ------------------------------------------------------------
// 6. 视频上传完整响应
// ------------------------------------------------------------
#[derive(Serialize)]
pub struct VideoUploadResponse {
    pub total_frames: usize,
    pub fps: f64,
    pub duration_secs: f64,
    pub frames: Vec<VideoFrameResult>,
    pub summary: VideoSummary,
}

// ------------------------------------------------------------
// 7. 特征映射类型别名
// ------------------------------------------------------------
pub type FeatureMap = HashMap<String, f64>;

// ------------------------------------------------------------
// 8. 实时帧分析请求（Chrome 扩展用）
// ------------------------------------------------------------
#[derive(Deserialize)]
pub struct AnalyzeFrameRequest {
    /// 当前帧的 Base64 编码（data:image/jpeg;base64,... 也可以）
    pub image_base64: String,
    /// 上一帧的 Base64 编码（首次为 null，用于计算 motion）
    #[serde(default)]
    pub prev_image_base64: Option<String>,
}

// ------------------------------------------------------------
// 9. 实时帧分析响应
// ------------------------------------------------------------
#[derive(Serialize)]
pub struct AnalyzeFrameResponse {
    pub features: FeatureResponse,
    pub evaluation: EvaluationResponse,
}