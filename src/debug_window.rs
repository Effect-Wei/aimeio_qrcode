use minifb::{Window, WindowOptions};
use rqrr::Point;

pub struct DebugWindow {
    width: usize,
    height: usize,
    scaled_buffer: Vec<u32>,
    window: Window,
}

impl DebugWindow {
    pub fn new(width: usize, height: usize, fps: usize, x: isize, y: isize) -> Option<Self> {
        let mut w = Window::new(
            "AimeIO QR",
            width,
            height,
            WindowOptions {
                borderless: true,
                title: false,
                resize: false,
                topmost: true,
                none: true,
                ..WindowOptions::default()
            },
        )
        .ok()?;
        w.set_target_fps(fps);
        w.set_position(x, y);
        Some(Self {
            width,
            height,
            scaled_buffer: vec![0; width * height],
            window: w,
        })
    }

    pub fn is_open(&self) -> bool {
        self.window.is_open()
    }

    pub fn size(&self) -> (usize, usize) {
        (self.width, self.height)
    }

    pub fn set_position(&mut self, x: isize, y: isize) {
        self.window.set_position(x, y);
    }

    pub fn update(&mut self, buf: &[u32], width: usize, height: usize) {
        if width == self.width && height == self.height {
            self.window
                .update_with_buffer(buf, self.width, self.height)
                .ok();
            return;
        }

        scale_buffer_nearest(
            buf,
            width,
            height,
            &mut self.scaled_buffer,
            self.width,
            self.height,
        );
        self.window
            .update_with_buffer(&self.scaled_buffer, self.width, self.height)
            .ok();
    }
}

fn scale_buffer_nearest(
    src: &[u32],
    src_width: usize,
    src_height: usize,
    dst: &mut [u32],
    dst_width: usize,
    dst_height: usize,
) {
    if src_width == 0 || src_height == 0 || dst_width == 0 || dst_height == 0 {
        return;
    }

    for y in 0..dst_height {
        let src_y = y * src_height / dst_height;
        let dst_row = y * dst_width;
        let src_row = src_y * src_width;

        for x in 0..dst_width {
            let src_x = x * src_width / dst_width;
            dst[dst_row + x] = src[src_row + src_x];
        }
    }
}

pub fn draw_square(
    buf: &mut [u32],
    width: u32,
    height: u32,
    p0: Point,
    p1: Point,
    p2: Point,
    p3: Point,
    color: u32,
) {
    draw_line(buf, width, height, p0.x, p0.y, p1.x, p1.y, color);
    draw_line(buf, width, height, p1.x, p1.y, p2.x, p2.y, color);
    draw_line(buf, width, height, p2.x, p2.y, p3.x, p3.y, color);
    draw_line(buf, width, height, p3.x, p3.y, p0.x, p0.y, color);
}

pub fn draw_line(
    buf: &mut [u32],
    width: u32,
    height: u32,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: u32,
) {
    let mut x = x0;
    let mut y = y0;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;

    loop {
        if x >= 0 && x < width as i32 && y >= 0 && y < height as i32 {
            buf[(y as u32 * width + x as u32) as usize] = color;
        }
        if x == x1 && y == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::scale_buffer_nearest;

    #[test]
    fn scale_buffer_nearest_expands_pixels() {
        let src = [1u32, 2, 3, 4];
        let mut dst = [0u32; 16];
        scale_buffer_nearest(&src, 2, 2, &mut dst, 4, 4);

        assert_eq!(dst, [1, 1, 2, 2, 1, 1, 2, 2, 3, 3, 4, 4, 3, 3, 4, 4,]);
    }
}
