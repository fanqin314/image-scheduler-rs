// ================================================================
// 模块：visualization.rs
// 功能：生成可视化标注图（将边缘检测结果和特征信息绘制成图片）
// 依赖：image、imageproc、标准库
// ================================================================

use image::{DynamicImage, GrayImage, ImageBuffer, Rgb};
use imageproc::drawing::draw_hollow_rect;
use imageproc::rect::Rect;
use crate::types::FeatureMap;
use std::io::Cursor;

/// 生成可视化标注图，返回 JPEG 格式的字节数据
///
/// # 参数
/// - `edge_map`: 边缘检测的灰度图（每个像素值代表边缘强度，0=无边缘，>0=边缘）
/// - `features`: 特征字典，包含 `local_peak` 等值，用于确定高亮区域
///
/// # 返回值
/// - `Vec<u8>`: JPEG 图像的字节数据，可直接用于 Base64 编码或保存为文件
pub fn generate_visualization(edge_map: &GrayImage, features: &FeatureMap) -> Vec<u8> {
    // ---------- 第1步：创建黑色画布 ----------
    // 获取边缘图的宽高（通常是 128x128）
    let (w, h) = edge_map.dimensions();

    // 创建一个 RGB 彩色图像，初始为纯黑色
    // Rgb([0u8, 0u8, 0u8]) 表示黑色（红0，绿0，蓝0）
    let mut rgb: ImageBuffer<Rgb<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(w, h, Rgb([0u8, 0u8, 0u8]));

    // ---------- 第2步：将边缘叠加到画布上（白色显示） ----------
    // 遍历每个像素，如果边缘图对应位置的值 > 0，则将该像素设为白色
    // 这样我们就能看到边缘的轮廓
    for y in 0..h {
        for x in 0..w {
            let e = edge_map.get_pixel(x, y)[0];  // 获取边缘强度
            if e > 0 {
                rgb.put_pixel(x, y, Rgb([255u8, 255u8, 255u8])); // 白色
            }
        }
    }

    // ---------- 第3步：绘制滑动窗口网格（灰色虚线框） ----------
    // 窗口大小 32x32，步长 16，和特征提取时保持一致
    let win_size = 32;
    let step = 16;

    // 从 (0,0) 开始，按步长滑动，绘制每个窗口的矩形边框
    // 注意：窗口不能超出图像边界，所以循环条件是 `y + win_size <= h`
    for y in (0..(h - win_size + 1)).step_by(step as usize) {
        for x in (0..(w - win_size + 1)).step_by(step as usize) {
            // 定义矩形区域：左上角 (x, y)，宽 win_size，高 win_size
            let rect = Rect::at(x as i32, y as i32).of_size(win_size, win_size);
            // 绘制空心矩形，颜色为灰色 (100,100,100)
            // draw_hollow_rect 返回 Result，我们用 `_` 忽略可能的错误
            let _ = draw_hollow_rect(&mut rgb, rect, Rgb([100u8, 100u8, 100u8]));
        }
    }

    // ---------- 第4步：高亮显示局部峰值窗口（绿色粗框） ----------
    // 从特征字典中获取 local_peak（局部最高边缘密度）
    let peak = *features.get("local_peak").unwrap_or(&0.0);

    // 只有当局部峰值 > 0 时才绘制高亮框
    if peak > 0.0 {
        // 重新扫描所有窗口，找出边缘密度最高的窗口位置
        let mut max_ratio = 0.0;
        let mut best_x = 0;
        let mut best_y = 0;

        let mut y = 0;
        while y + win_size <= h {
            let mut x = 0;
            while x + win_size <= w {
                // 统计当前窗口内的边缘像素数量（edge_map 中值 > 0 的像素）
                let mut count = 0u64;
                for i in 0..win_size {
                    for j in 0..win_size {
                        if edge_map.get_pixel(x + i, y + j)[0] > 0 {
                            count += 1;
                        }
                    }
                }
                // 计算窗口边缘密度 = 边缘像素数 / 窗口总面积
                let ratio = count as f64 / (win_size * win_size) as f64;

                // 更新最大值和位置
                if ratio > max_ratio {
                    max_ratio = ratio;
                    best_x = x;
                    best_y = y;
                }
                x += step;
            }
            y += step;
        }

        // 在找到的峰值位置绘制绿色矩形边框（颜色 0,255,0）
        let rect = Rect::at(best_x as i32, best_y as i32).of_size(win_size, win_size);
        let _ = draw_hollow_rect(&mut rgb, rect, Rgb([0u8, 255u8, 0u8]));
    }

    // ---------- 第5步：绘制下半区分隔线（黄色水平线） ----------
    // 将图像水平分成上下两半，用于示意“下半区优势比”
    let half_y = (h / 2) as i32;
    for x in 0..w as i32 {
        // 在 y = half_y 这条线上绘制黄色像素 (255,255,0)
        rgb.put_pixel(x as u32, half_y as u32, Rgb([255u8, 255u8, 0u8]));
    }

    // ---------- 第6步：将生成的 RGB 图像编码为 JPEG 格式 ----------
    // 首先将 ImageBuffer 转换为 DynamicImage（统一图像类型）
    let dyn_img = DynamicImage::ImageRgb8(rgb);

    // 创建一个内存缓冲区（Vec<u8>），用于存储 JPEG 数据
    let mut buf = Vec::new();
    // 用 Cursor 包装，实现 Seek  trait，满足 write_to 的要求
    let mut cursor = Cursor::new(&mut buf);
    // 以 JPEG 格式写入，质量参数 85（范围 0~100，值越大质量越好，文件也越大）
    let _ = dyn_img.write_to(&mut cursor, image::ImageOutputFormat::Jpeg(85));

    // 返回 JPEG 字节数据
    buf
}