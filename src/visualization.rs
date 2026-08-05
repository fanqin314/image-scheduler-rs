// ================================================================
// 模块：visualization.rs
// 功能：生成可视化标注图（边缘叠加 + 网格 + 轮廓 + 峰值窗口高亮）
// ================================================================

use image::{DynamicImage, GrayImage, ImageBuffer, Luma, Rgb};
use imageproc::contours::find_contours;
use imageproc::drawing::draw_hollow_rect;
use imageproc::rect::Rect;
use crate::config;
use crate::features; // 用于 segment_foreground
use crate::types::FeatureMap;
use std::io::Cursor;

/// 生成可视化标注图（原图灰度 + 边缘 + Otsu分割轮廓 + 峰值窗），返回 JPEG 字节
///
/// # 参数
/// - `gray_thumb`: 原图的 128×128 灰度缩略图（作为背景）
/// - `edge_map`: Sobel 边缘图（白色边缘叠加在背景上）
/// - `features`: 特征映射，用于定位"局部峰值窗口"
///
/// # 绘制图例
/// | 颜色 | 含义 |
/// |------|------|
/// | 背景 | 原视频帧灰度 |
/// | 白色高亮 | Sobel 边缘像素 |
/// | 绿色细线 | 检测到的轮廓 |
/// | 绿色粗框 | 局部峰值窗口 |
pub fn generate_visualization(
    gray_thumb: &GrayImage,
    edge_map: &GrayImage,
    features: &FeatureMap,
) -> Vec<u8> {
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
    let win_size = config::WINDOW_SIZE;
    let step = config::WINDOW_STEP;
    let peak = *features.get("local_peak").unwrap_or(&0.0);
    if peak > 0.0 {
        let mut max_ratio = 0.0;
        let mut best_x = 0;
        let mut best_y = 0;
        let mut y = 0;
        while y + win_size <= h {
            let mut x = 0;
            while x + win_size <= w {
                let mut count = 0u64;
                for i in 0..win_size {
                    for j in 0..win_size {
                        if edge_map.get_pixel(x + i, y + j)[0] > 0 {
                            count += 1;
                        }
                    }
                }
                let ratio = count as f64 / (win_size * win_size) as f64;
                if ratio > max_ratio {
                    max_ratio = ratio;
                    best_x = x;
                    best_y = y;
                }
                x += step;
            }
            y += step;
        }
        let rect = Rect::at(best_x as i32, best_y as i32).of_size(win_size, win_size);
        let _ = draw_hollow_rect(&mut rgb, rect, Rgb([0u8, 255u8, 0u8]));
    }

    // 编码为 JPEG
    let dyn_img = DynamicImage::ImageRgb8(rgb);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    let _ = dyn_img.write_to(&mut cursor, image::ImageOutputFormat::Jpeg(85));
    buf
}