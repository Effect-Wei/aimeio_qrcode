use crate::debug_window;
use crate::debug_window::DebugWindow;
use image::{ImageBuffer, Luma};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::FrameFormat;
use rqrr::PreparedImage;
use std::time::{Duration, Instant};

/// 扫码器实例，持有预分配的缓冲区以实现零内存分配 (Zero-Allocation)
pub struct QrScanner {
    width: u32,
    height: u32,
    luma_buffer: Vec<u8>,      // 预分配的灰度缓冲区
    debug_buffer: Vec<u32>,    // 预分配的 Debug 窗口 ARGB 缓冲区
    last_decode: Instant,      // 上次解码时间
    decode_interval: Duration, // 解码时间间隔 (用于降频)
}

impl QrScanner {
    /// 初始化扫码器，预分配所需的全部内存
    /// `decode_fps`: 期望的解码帧率 (例如传入 15.0，意味着画面 30帧 但每秒只解 15 次以节省 CPU)
    pub fn new(width: u32, height: u32, decode_fps: f64) -> Self {
        let pixels = (width * height) as usize;
        Self {
            width,
            height,
            luma_buffer: vec![0; pixels],
            debug_buffer: vec![0; pixels],
            last_decode: Instant::now() - Duration::from_secs(1), // 确保第一帧立刻解码
            decode_interval: Duration::from_secs_f64(1.0 / decode_fps),
        }
    }

    pub fn decode_qr(
        &mut self,
        frame: &nokhwa::Buffer,
        window: &mut Option<DebugWindow>,
    ) -> Option<[u8; 10]> {
        if frame.source_frame_format() == FrameFormat::YUYV {
            self.decode_qr_yuyv(frame, window)
        } else {
            self.decode_qr_fallback(frame, window)
        }
    }

    /// 高速通道：处理 YUYV 帧
    pub fn decode_qr_yuyv(
        &mut self,
        frame: &nokhwa::Buffer,
        window: &mut Option<DebugWindow>,
    ) -> Option<[u8; 10]> {
        let yuyv_buffer = frame.buffer();
        let format = frame.source_frame_format();
        if format != FrameFormat::YUYV {
            println!(
                "AimeIO DLL: Received non-YUYV frame (format: {:?}).",
                format
            );
            return None;
        }

        let expected_len = (self.width * self.height * 2) as usize;
        if yuyv_buffer.len() < expected_len {
            return None;
        }

        // 优化 3：抛弃 step_by(2) 并避免 collect 分配，使用 chunks_exact 直接覆盖预分配内存
        // YUYV 的排列为[Y0, U0, Y1, V0]... 每次取 2 个字节，第 0 个字节即为 Y 亮度
        for (src, dst) in yuyv_buffer.chunks_exact(2).zip(self.luma_buffer.iter_mut()) {
            *dst = src[0];
        }

        self.process_luma_and_detect(window)
    }

    /// 慢速通道：Fallback 转换
    pub fn decode_qr_fallback(
        &mut self,
        frame: &nokhwa::Buffer,
        window: &mut Option<DebugWindow>,
    ) -> Option<[u8; 10]> {
        let rgb_img = frame
            .decode_image::<RgbFormat>()
            .unwrap_or_else(|_| ImageBuffer::new(self.width, self.height));

        // 优化 3：使用 chunks_exact 遍历 RGB，直接复用 luma_buffer
        for (src, dst) in rgb_img
            .as_raw()
            .chunks_exact(3)
            .zip(self.luma_buffer.iter_mut())
        {
            // 标准灰度计算。如果在超低端 CPU 上，可以直接用绿色通道代替灰度: *dst = src[1];
            *dst = ((src[0] as u16 * 77 + src[1] as u16 * 150 + src[2] as u16 * 29) >> 8) as u8;
        }

        self.process_luma_and_detect(window)
    }

    /// 内部核心逻辑：降频解码 + Debug 渲染
    fn process_luma_and_detect(&mut self, window: &mut Option<DebugWindow>) -> Option<[u8; 10]> {
        let mut update_window = false;

        // 1. 如果有窗口，生成 ARGB 像素 (只更新内存，无需新分配)
        if let Some(w) = window.as_mut() {
            if w.is_open() {
                update_window = true;
                // 优化 3：利用 zip 完美向量化，替代原先的 iter().map().collect()
                for (src, dst) in self.luma_buffer.iter().zip(self.debug_buffer.iter_mut()) {
                    let y = *src as u32;
                    *dst = (255 << 24) | (y << 16) | (y << 8) | y;
                }
            } else {
                *window = None; // 窗口已关闭，清理句柄
            }
        }

        let mut found_id = None;

        // 2. 优化 2：降频解码，如果时间未到直接跳过解码 (但画面依然保持 30FPS 流畅)
        if self.last_decode.elapsed() >= self.decode_interval {
            self.last_decode = Instant::now();

            // ⚠️ 妥协与极速的平衡：
            // 因为 rqrr 强制要求容器同时实现 DerefMut(可变) 和 Clone(可克隆)，&mut [u8] 无法胜任。
            // 我们在此处 .clone() 一份 300KB 的灰度图专供 rqrr 消耗。
            // 在 15FPS 降频下，这仅带来不到 0.1ms 的极微小开销，但避免了更严重的全量反复重新计算。
            let luma_clone = self.luma_buffer.clone();

            if let Some(luma_img) =
                ImageBuffer::<Luma<u8>, Vec<u8>>::from_raw(self.width, self.height, luma_clone)
            {
                let mut prepared_img = PreparedImage::prepare(luma_img);
                let grids = prepared_img.detect_grids();

                for grid in grids {
                    // 绘制 Debug 边框
                    if update_window {
                        let [p0, p1, p2, p3] = grid.bounds;
                        let color = 0xFF00FF00; // 绿色
                        debug_window::draw_square(
                            &mut self.debug_buffer,
                            self.width,
                            self.height,
                            p0,
                            p1,
                            p2,
                            p3,
                            color,
                        );
                    }

                    // 尽早短路解码
                    if found_id.is_none() {
                        if let Ok((_meta, content)) = grid.decode() {
                            found_id = fast_parse_aime_hex(&content);
                        }
                    }
                }
            }
        }

        // 3. 将修改后的缓冲区推送到窗口 (60/30FPS 实时更新)
        if update_window {
            if let Some(w) = window.as_mut() {
                w.update(
                    &self.debug_buffer,
                    self.width as usize,
                    self.height as usize,
                );
            }
        }

        found_id
    }
}

/* ----- 辅助函数 ----- */

/// 超快速 Aime Hex 字符串解析器 (零分配，无边界检查)
fn fast_parse_aime_hex(content: &str) -> Option<[u8; 10]> {
    let bytes = content.as_bytes();
    if bytes.len() != 20 {
        return None;
    }

    let mut res = [0u8; 10];
    for i in 0..10 {
        let hi = hex_char_to_val(bytes[i * 2])?;
        let lo = hex_char_to_val(bytes[i * 2 + 1])?;
        res[i] = (hi << 4) | lo;
    }
    Some(res)
}

/// 辅助函数：ASCII 字符转 Hex 值
#[inline(always)]
fn hex_char_to_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}
