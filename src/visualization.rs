// ================================================================
// 模块：visualization.rs
// 功能：生成可视化标注图（边缘叠加 + 网格 + 轮廓 + 峰值窗口高亮）
// ================================================================

use image::{DynamicImage, GrayImage, ImageBuffer, Luma, Rgb};
use imageproc::contours::find_contours;
use imageproc::drawing::{draw_hollow_rect, draw_line_segment};
use imageproc::point::Point;
use imageproc::rect::Rect;
use crate::config;
use crate::types::FeatureMap;
use std::io::Cursor;

/// 生成可视化标注图，返回 JPEG 字节数据
///
/// # 参数
/// - `edge_map`: 来自 features::get_edge_map 的边缘强度图
/// - `features`: 特征映射，用于定位"局部峰值窗口"（local_peak 对应的窗口位置）
///
/// # 返回值
/// - JPEG 编码的图像字节，前端以 Base64 展示
///
/// # 绘制图例
/// | 颜色 | 含义 |
/// |------|------|
/// | 白色 | Sobel 边缘像素 |
/// | 灰色网格 | 滑动窗口扫描范围 |
/// | 绿色粗框 | 局部峰值窗口（边缘最密集的区域） |
/// | 绿色细线 | 检测到的轮廓 |
/// | 黄色横线 | 上下半区分界线（车行场景路面/天空） |
pub fn generate_visualization(edge_map: &GrayImage, features: &FeatureMap) -> Vec<u8> {
    let (w, h) = edge_map.dimensions();
    let mut rgb: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(w, h, Rgb([0u8, 0u8, 0u8]));

    // 叠加边缘（白色）
    for y in 0..h {
        for x in 0..w {
            let e = edge_map.get_pixel(x, y)[0];
            if e > 0 {
                rgb.put_pixel(x, y, Rgb([255u8, 255u8, 255u8]));
            }
        }
    }

    // 网格（灰色）：与 features.rs 的滑动窗口参数保持一致
    let win_size = config::WINDOW_SIZE;
    let step = config::WINDOW_STEP;
    for y in (0..(h - win_size + 1)).step_by(step as usize) {
        for x in (0..(w - win_size + 1)).step_by(step as usize) {
            let rect = Rect::at(x as i32, y as i32).of_size(win_size, win_size);
            let _ = draw_hollow_rect(&mut rgb, rect, Rgb([100u8, 100u8, 100u8]));
        }
    }

    // === 绘制轮廓线（绿色） ===
    // 二值化边缘图
    let mut binary = GrayImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = edge_map.get_pixel(x, y)[0];
            binary.put_pixel(x, y, Luma([if v > 0 { 255u8 } else { 0u8 }]));
        }
    }

    let contours = find_contours(&binary);

    for contour in contours.iter() {
        if contour.points.len() < 3 {
            continue;
        }
        // 连接相邻点
        for i in 0..contour.points.len() - 1 {
            let p1: Point<i32> = contour.points[i];
            let p2: Point<i32> = contour.points[i + 1];
            let _ = draw_line_segment(
                &mut rgb,
                (p1.x as f32, p1.y as f32),
                (p2.x as f32, p2.y as f32),
                Rgb([0u8, 255u8, 0u8]),
            );
        }
        // 闭合轮廓
        if let (Some(first), Some(last)) = (contour.points.first(), contour.points.last()) {
            let p1: Point<i32> = *first;
            let p2: Point<i32> = *last;
            let _ = draw_line_segment(
                &mut rgb,
                (p1.x as f32, p1.y as f32),
                (p2.x as f32, p2.y as f32),
                Rgb([0u8, 255u8, 0u8]),
            );
        }
    }

    // 高亮局部峰值窗口（绿色粗框）
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

    // 下半区分隔线（黄色）
    let half_y = (h / 2) as i32;
    for x in 0..w as i32 {
        rgb.put_pixel(x as u32, half_y as u32, Rgb([255u8, 255u8, 0u8]));
    }

    // 编码为 JPEG
    let dyn_img = DynamicImage::ImageRgb8(rgb);
    let mut buf = Vec::new();
    let mut cursor = Cursor::new(&mut buf);
    let _ = dyn_img.write_to(&mut cursor, image::ImageOutputFormat::Jpeg(85));
    buf
}