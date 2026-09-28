// ================================================================
// 模块：visualization.rs
// 功能：生成可视化标注图（边缘叠加 + 网格 + 轮廓 + 峰值窗口高亮）
// ================================================================

use image::{DynamicImage, GrayImage, ImageBuffer, Rgb};
use imageproc::contours::find_contours;
use imageproc::drawing::draw_hollow_rect;
use imageproc::rect::Rect;
use scheduler_core::{config, features}; // features 用于 segment_foreground 与 sliding_window_features
use std::io::Cursor;

/// 生成可视化标注图（原图灰度 + 边缘 + Otsu分割轮廓 + 峰值窗），返回 JPEG 字节
///
/// # 参数
/// - `gray_thumb`: 原图的 THUMBNAIL_SIZE 见方灰度缩略图（作为背景）
/// - `edge_map`: Sobel **二值**边缘图（白色边缘叠加在背景上）
///
/// 峰值窗口位置由本函数基于 `edge_map` 现算，与特征提取口径一致，
/// 因此无需额外传入特征映射。
///
/// # 绘制图例
/// | 颜色 | 含义 |
/// |------|------|
/// | 背景 | 原视频帧灰度 |
/// | 白色高亮 | Sobel 边缘像素 |
/// | 绿色细线 | 检测到的轮廓 |
/// | 绿色粗框 | 局部峰值窗口 |
pub fn generate_visualization(gray_thumb: &GrayImage, edge_map: &GrayImage) -> Vec<u8> {
    let (w, h) = edge_map.dimensions();
    let mut rgb: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(w, h, Rgb([0u8, 0u8, 0u8]));

    // 背景：原图灰度缩略图
    for y in 0..h {
        for x in 0..w {
            let g = gray_thumb.get_pixel(x, y)[0];
            rgb.put_pixel(x, y, Rgb([g, g, g]));
        }
    }

    // 叠加边缘（白色高亮，alpha=0.7 与原图混合）
    for y in 0..h {
        for x in 0..w {
            let e = edge_map.get_pixel(x, y)[0];
            if e > 0 {
                let bg = gray_thumb.get_pixel(x, y)[0];
                let blended = ((bg as u16 * 3 + 255u16 * 7) / 10) as u8;
                rgb.put_pixel(x, y, Rgb([blended, blended, blended]));
            }
        }
    }

    // === 绘制物体轮廓（绿色，基于 Otsu 前景分割） ===
    let binary = features::segment_foreground(gray_thumb);
    let contours = find_contours::<i32>(&binary);

    // 128x128 图中轮廓点仅 1-2px 宽→膨胀到 3x3 使其肉眼可见
    for contour in contours.iter() {
        if contour.points.len() < 4 { continue; }
        for pt in &contour.points {
            let px = pt.x as i32;
            let py = pt.y as i32;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let nx = (px + dx).max(0).min(w as i32 - 1) as u32;
                    let ny = (py + dy).max(0).min(h as i32 - 1) as u32;
                    rgb.put_pixel(nx, ny, Rgb([0u8, 255u8, 0u8]));
                }
            }
        }
    }

    // 高亮局部峰值窗口（绿色粗框）
    // 复用特征提取模块的滑动窗口函数（积分图实现，整体 O(N)），
    // 避免在此再逐像素重扫全部窗口（原实现约 5 万次采样），
    // 同时保证高亮的窗口与 local_peak 特征的取值口径完全一致。
    let (peak, _variance, (best_x, best_y)) =
        features::sliding_window_features(edge_map, config::WINDOW_SIZE, config::WINDOW_STEP);
    if peak > 0.0 {
        let rect = Rect::at(best_x as i32, best_y as i32)
            .of_size(config::WINDOW_SIZE, config::WINDOW_SIZE);
        let _ = draw_hollow_rect(&mut rgb, rect, Rgb([0u8, 255u8, 0u8]));
    }

    // 编码为 JPEG
    let dyn_img = DynamicImage::ImageRgb8(rgb);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    let _ = dyn_img.write_to(&mut cursor, image::ImageOutputFormat::Jpeg(85));
    buf
}