use crate::debug_window;
use crate::debug_window::DebugWindow;
use image::{ImageBuffer, Luma};
use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::FrameFormat;
use rqrr::PreparedImage;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecodedCard {
    Aime([u8; 10]),
    Felica(u64),
}

/// 扫码器实例，持有预分配的缓冲区以实现零内存分配 (Zero-Allocation)
pub struct QrScanner {
    width: u32,
    height: u32,
    luma_buffer: Vec<u8>,      // 预分配的灰度缓冲区
    debug_buffer: Vec<u32>,    // 预分配的 Debug 窗口 0RGB 缓冲区
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
        windows: &mut Vec<DebugWindow>,
    ) -> Option<DecodedCard> {
        if frame.source_frame_format() == FrameFormat::YUYV {
            self.decode_qr_yuyv(frame, windows)
        } else {
            self.decode_qr_fallback(frame, windows)
        }
    }

    /// 高速通道：处理 YUYV 帧
    fn decode_qr_yuyv(
        &mut self,
        frame: &nokhwa::Buffer,
        windows: &mut Vec<DebugWindow>,
    ) -> Option<DecodedCard> {
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

        let show_debug = self.prepare_debug_windows(windows);

        // YUYV 的排列为 [Y0, U, Y1, V]。这里同时生成解码所需的灰度图和 debug 窗口所需的彩色图。
        for (i, src) in yuyv_buffer.chunks_exact(4).enumerate() {
            let y0 = src[0];
            let u = src[1];
            let y1 = src[2];
            let v = src[3];
            let pixel_index = i * 2;

            self.luma_buffer[pixel_index] = y0;
            self.luma_buffer[pixel_index + 1] = y1;

            if show_debug {
                self.debug_buffer[pixel_index] = yuv_to_rgb0(y0, u, v);
                self.debug_buffer[pixel_index + 1] = yuv_to_rgb0(y1, u, v);
            }
        }

        self.process_luma_and_detect(windows)
    }

    /// 慢速通道：Fallback 转换
    fn decode_qr_fallback(
        &mut self,
        frame: &nokhwa::Buffer,
        windows: &mut Vec<DebugWindow>,
    ) -> Option<DecodedCard> {
        let show_debug = self.prepare_debug_windows(windows);
        let rgb_img = frame
            .decode_image::<RgbFormat>()
            .unwrap_or_else(|_| ImageBuffer::new(self.width, self.height));

        // 遍历 RGB 数据，同时准备灰度识别输入和用于显示的彩色缓冲。
        for (i, src) in rgb_img.as_raw().chunks_exact(3).enumerate() {
            let r = src[0];
            let g = src[1];
            let b = src[2];

            // 标准灰度计算。如果在超低端 CPU 上，可以直接用绿色通道代替灰度。
            self.luma_buffer[i] = ((r as u16 * 77 + g as u16 * 150 + b as u16 * 29) >> 8) as u8;

            if show_debug {
                self.debug_buffer[i] = rgb_to_rgb0(r, g, b);
            }
        }

        self.process_luma_and_detect(windows)
    }

    /// 内部核心逻辑：降频解码 + Debug 渲染
    fn process_luma_and_detect(&mut self, windows: &mut Vec<DebugWindow>) -> Option<DecodedCard> {
        let update_window = !windows.is_empty();

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
                        let color = 0x0000FF00; // 绿色，minifb 使用 0RGB 编码
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
                            found_id = parse_card_payload(&content);
                        }
                    }
                }
            }
        }

        // 3. 将修改后的缓冲区推送到窗口 (60/30FPS 实时更新)
        if update_window {
            for w in windows.iter_mut() {
                w.update(
                    &self.debug_buffer,
                    self.width as usize,
                    self.height as usize,
                );
            }
        }

        found_id
    }

    fn prepare_debug_windows(&mut self, windows: &mut Vec<DebugWindow>) -> bool {
        windows.retain(|w| w.is_open());
        !windows.is_empty()
    }
}

/* ----- 辅助函数 ----- */

fn parse_card_payload(content: &str) -> Option<DecodedCard> {
    let trimmed = content.trim();
    if trimmed.len() == 20 && trimmed.as_bytes().iter().all(|c| c.is_ascii_digit()) {
        return parse_aime_access_code(trimmed).map(DecodedCard::Aime);
    }

    if trimmed.len() == 16 {
        return parse_felica_idm(trimmed).map(DecodedCard::Felica);
    }

    None
}

fn parse_aime_access_code(content: &str) -> Option<[u8; 10]> {
    let bytes = content.as_bytes();
    if bytes.len() != 20 {
        return None;
    }

    let mut res = [0u8; 10];
    for i in 0..10 {
        let hi = decimal_char_to_val(bytes[i * 2])?;
        let lo = decimal_char_to_val(bytes[i * 2 + 1])?;
        res[i] = (hi << 4) | lo;
    }
    Some(res)
}

fn parse_felica_idm(content: &str) -> Option<u64> {
    let bytes = content.as_bytes();
    if bytes.len() != 16 {
        return None;
    }

    let mut res = [0u8; 8];
    for i in 0..8 {
        let hi = hex_char_to_val(bytes[i * 2])?;
        let lo = hex_char_to_val(bytes[i * 2 + 1])?;
        res[i] = (hi << 4) | lo;
    }

    Some(u64::from_be_bytes(res))
}

#[inline(always)]
fn decimal_char_to_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        _ => None,
    }
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

#[inline(always)]
fn rgb_to_rgb0(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

#[inline(always)]
fn yuv_to_rgb0(y: u8, u: u8, v: u8) -> u32 {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;

    let r = clamp_u8((298 * c + 409 * e + 128) >> 8);
    let g = clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8);
    let b = clamp_u8((298 * c + 516 * d + 128) >> 8);

    rgb_to_rgb0(r, g, b)
}

#[inline(always)]
fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::{DecodedCard, parse_card_payload};

    #[test]
    fn parse_card_payload_parses_aime_access_code() {
        assert_eq!(
            parse_card_payload("01234567890123456789"),
            Some(DecodedCard::Aime([
                0x01, 0x23, 0x45, 0x67, 0x89, 0x01, 0x23, 0x45, 0x67, 0x89,
            ]))
        );
    }

    #[test]
    fn parse_card_payload_parses_felica_idm() {
        assert_eq!(
            parse_card_payload("0123456789ABCDEF"),
            Some(DecodedCard::Felica(0x0123456789ABCDEF))
        );
    }

    #[test]
    fn parse_card_payload_rejects_invalid_length() {
        assert_eq!(parse_card_payload("1234"), None);
    }

    #[test]
    fn parse_card_payload_rejects_non_decimal_access_code() {
        assert_eq!(parse_card_payload("0123456789ABCDE12345"), None);
    }

    #[test]
    fn parse_card_payload_rejects_invalid_felica_characters() {
        assert_eq!(parse_card_payload("0123456789ABCDEG"), None);
    }
}
