//! 视频解码模块
//!
//! 通过 ffmpeg-sidecar 自动下载并调用 ffmpeg CLI，
//! 首次运行时自动下载 ffmpeg 二进制（~30MB），之后零配置。

use anyhow::{Context, Result};
use image::DynamicImage;
use std::path::Path;
use tempfile::TempDir;
use crate::config;

/// 解码后的视频帧集合
pub struct VideoFrames {
    /// (时间戳秒, 帧图像)，按时间顺序排列
    pub frames: Vec<(f64, DynamicImage)>,
    /// 采样帧率（实际提取时的 fps）
    pub fps: f64,
    /// 总时长（秒）
    pub total_duration: f64,
}

/// 确保 ffmpeg 已下载就绪，首次调用时自动下载
pub fn ensure_ffmpeg() -> Result<()> {
    ffmpeg_sidecar::download::auto_download()
        .context("ffmpeg 自动下载失败，请检查网络连接")?;
    Ok(())
}

/// 解码视频文件，返回采样后的帧集合
///
/// 采样策略：最多 VIDEO_MAX_FRAMES 帧，超长视频自动拉大采样间隔，
/// 控制解码耗时与内存占用（帧以 128x128 缩略图形式解码）。
pub fn decode_video(video_path: &Path) -> Result<VideoFrames> {
    // 1. 获取视频信息
    let fps = get_fps(video_path).unwrap_or(30.0);
    let duration = get_duration(video_path).unwrap_or(10.0);

    // 2. 计算采样间隔：总帧数超过上限时，等间隔抽帧
    //    duration * fps 为视频总帧数，VIDEO_MAX_FRAMES 为允许分析的最大帧数
    let sample_interval = if duration * fps <= config::VIDEO_MAX_FRAMES {
        1.0 / fps  // 视频很短：逐帧采样
    } else {
        duration / config::VIDEO_MAX_FRAMES  // 视频较长：等间隔抽帧
    };

    // 3. 创建临时目录
    let temp_dir = TempDir::new().context("创建临时目录失败")?;
    let output_pattern = temp_dir.path().join("frame_%06d.jpg");

    // 4. 调用 ffmpeg 提取帧，直接用 THUMBNAIL_SIZE 缩略图
    //    减少解码体积，与 features.rs 的特征提取分辨率保持一致
    let scale = format!("{}:{}", config::THUMBNAIL_SIZE, config::THUMBNAIL_SIZE);
    let mut child = ffmpeg_sidecar::command::FfmpegCommand::new()
        .arg("-i").arg(video_path)
        .arg("-vf").arg(format!("fps=1/{},scale={}", sample_interval, scale))
        .arg("-q:v").arg("2")
        .arg("-loglevel").arg("error")
        .arg(&output_pattern)
        .spawn()
        .context("ffmpeg 启动失败")?;

    let status = child.wait().context("ffmpeg 等待失败")?;

    if !status.success() {
        anyhow::bail!("ffmpeg 解码失败，请确认视频文件格式正确");
    }

    // 5. 读取帧文件
    let mut frame_paths: Vec<_> = std::fs::read_dir(temp_dir.path())
        .context("读取临时目录失败")?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |ext| ext.eq_ignore_ascii_case("jpg")))
        .collect();
    frame_paths.sort();

    if frame_paths.is_empty() {
        anyhow::bail!("视频解码后未产生任何帧");
    }

    // 6. 加载为 DynamicImage
    let mut frames = Vec::with_capacity(frame_paths.len());
    for (i, path) in frame_paths.iter().enumerate() {
        let img = image::open(path)
            .with_context(|| format!("无法读取帧文件: {:?}", path))?;
        let timestamp = i as f64 * sample_interval;
        frames.push((timestamp, img));
    }

    Ok(VideoFrames {
        total_duration: duration,
        fps: 1.0 / sample_interval,
        frames,
    })
}

fn get_fps(path: &Path) -> Result<f64> {
    let output = std::process::Command::new(ffmpeg_sidecar::ffprobe::ffprobe_path())
        .arg("-v").arg("error")
        .arg("-select_streams").arg("v:0")
        .arg("-show_entries").arg("stream=r_frame_rate")
        .arg("-of").arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .context("ffprobe 执行失败")?;

    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if let Some((num, den)) = raw.split_once('/') {
        let n: f64 = num.parse().unwrap_or(30.0);
        let d: f64 = den.parse().unwrap_or(1.0);
        Ok(n / d)
    } else {
        raw.parse::<f64>().ok().context("无法解析 FPS")
    }
}

fn get_duration(path: &Path) -> Result<f64> {
    let output = std::process::Command::new(ffmpeg_sidecar::ffprobe::ffprobe_path())
        .arg("-v").arg("error")
        .arg("-show_entries").arg("format=duration")
        .arg("-of").arg("default=noprint_wrappers=1:nokey=1")
        .arg(path)
        .output()
        .context("ffprobe 执行失败")?;

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .context("无法解析视频时长")
}
