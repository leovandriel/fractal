/// A two-dimensional size with width and height as 32-bit unsigned integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Size32 {
    pub w: u32,
    pub h: u32,
}

/// A two-dimensional point with integer coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Point32 {
    pub x: i32,
    pub y: i32,
}

/// Direction to scale the buffer - either doubling or halving dimensions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleDirection {
    /// Double the dimensions (2x scale)
    Up,
    /// Halve the dimensions (0.5x scale)
    Down,
}

/// Translates a pixel buffer by the specified delta
///
/// # Arguments
/// * `src_buffer` - Source buffer containing RGBA pixel data
/// * `size` - Dimensions of the buffer in pixels
/// * `pitch` - Number of bytes per row in the buffer
/// * `delta` - Pixel offset to apply
///
/// # Returns
/// A new buffer containing the translated pixel data
pub fn translate_rect(src_buffer: &[u8], size: Size32, pitch: u32, delta: Point32) -> Vec<u8> {
    let mut dst_buffer = vec![0; (pitch * size.h) as usize];
    let width = (size.w.saturating_sub(delta.x.unsigned_abs())) as usize;
    let height = (size.h.saturating_sub(delta.y.unsigned_abs())) as usize;
    let src_offset = (delta.y.max(0) * pitch as i32 + delta.x.max(0) * 4) as usize;
    let dst_offset = ((-delta.y).max(0) * pitch as i32 + (-delta.x).max(0) * 4) as usize;

    for y in (0..height * pitch as usize).step_by(pitch as usize) {
        let src = y + src_offset;
        let dst = y + dst_offset;
        dst_buffer[dst..dst + width * 4].copy_from_slice(&src_buffer[src..src + width * 4]);
    }

    dst_buffer
}

/// Extends a pixel buffer to a new size
///
/// # Arguments
/// * `src_buffer` - Source buffer containing RGBA pixel data
/// * `src_size` - Dimensions of the source buffer in pixels
/// * `src_pitch` - Number of bytes per row in the source buffer
/// * `dst_size` - Dimensions of the target buffer in pixels
/// * `dst_pitch` - Number of bytes per row in the target buffer
///
/// # Returns
/// A new buffer with the extended dimensions
pub fn extend_buffer(
    src_buffer: &[u8],
    src_size: Size32,
    src_pitch: u32,
    dst_size: Size32,
    dst_pitch: u32,
) -> Vec<u8> {
    let mut dst_buffer = vec![0; (dst_pitch * dst_size.h) as usize];
    let width = dst_size.w.min(src_size.w) as usize;
    let height = dst_size.h.min(src_size.h) as usize;

    for y in 0..height {
        let src = y * src_pitch as usize;
        let dst = y * dst_pitch as usize;
        dst_buffer[dst..dst + width * 4].copy_from_slice(&src_buffer[src..src + width * 4]);
    }

    dst_buffer
}

/// Scales a pixel buffer up or down by a factor of 2, with pixel offset
///
/// # Arguments
/// * `src_buffer` - Source buffer containing RGBA pixel data
/// * `size` - Dimensions of the buffer in pixels
/// * `pitch` - Number of bytes per row in the buffer
/// * `delta` - Pixel offset to apply during scaling
/// * `direction` - Whether to scale up (2x) or down (0.5x)
///
/// # Returns
/// A new buffer containing the scaled pixel data
pub fn scale_rect(
    src_buffer: &[u8],
    size: Size32,
    pitch: u32,
    delta: Point32,
    direction: ScaleDirection,
) -> Vec<u8> {
    let mut dst_buffer = vec![0; (pitch * size.h) as usize];
    let pitch = pitch as usize / 4;

    match direction {
        ScaleDirection::Up => copy_rows_up(
            src_buffer,
            &mut dst_buffer,
            delta.x as usize,
            delta.y as usize,
            size.w as usize / 2,
            size.h as usize / 2,
            pitch,
            pitch,
        ),
        ScaleDirection::Down => copy_rows_down(
            src_buffer,
            &mut dst_buffer,
            -delta.x as usize / 2,
            -delta.y as usize / 2,
            size.w as usize / 2,
            size.h as usize / 2,
            pitch,
            pitch,
        ),
    }

    dst_buffer
}

/// Copy a range of pixel rows from the source buffer to the destination buffer, scaling them up by 2
fn copy_rows_up(
    src_buffer: &[u8],
    dst_buffer: &mut [u8],
    src_x: usize,
    src_y: usize,
    width: usize,
    height: usize,
    src_pitch: usize,
    dst_pitch: usize,
) {
    let src_offset = src_y * src_pitch + src_x;
    for (src_lower, dst_lower) in (src_offset..height * src_pitch + src_offset)
        .step_by(src_pitch)
        .zip((0..height * dst_pitch).step_by(dst_pitch))
    {
        copy_row_up(src_buffer, dst_buffer, src_lower, dst_lower, width);
        copy_row_up(
            src_buffer,
            dst_buffer,
            src_lower,
            dst_lower + dst_pitch / 2,
            width,
        );
    }
}

/// Copy a single row of pixels from the source buffer to the destination buffer, scaling them up by 2
fn copy_row_up(
    src_buffer: &[u8],
    dst_buffer: &mut [u8],
    src_lower: usize,
    dst_lower: usize,
    width: usize,
) {
    for (src, dst) in (src_lower * 4..(src_lower + width) * 4)
        .step_by(4)
        .zip((dst_lower * 8..(dst_lower + width) * 8).step_by(8))
    {
        // Copy the source pixel to two adjacent pixels in the destination
        let slice = &src_buffer[src..src + 4];
        dst_buffer[dst..dst + 4].copy_from_slice(slice);
        dst_buffer[dst + 4..dst + 8].copy_from_slice(slice);
    }
}

/// Copy a range of pixel rows from the source buffer to the destination buffer, scaling them down by 2
fn copy_rows_down(
    src_buffer: &[u8],
    dst_buffer: &mut [u8],
    dst_x: usize,
    dst_y: usize,
    width: usize,
    height: usize,
    src_pitch: usize,
    dst_pitch: usize,
) {
    let dst_offset = dst_y * dst_pitch + dst_x;
    for (src_lower, dst_lower) in (0..height * src_pitch)
        .step_by(src_pitch as usize)
        .zip((dst_offset..height * dst_pitch + dst_offset).step_by(dst_pitch as usize))
    {
        copy_row_down(src_buffer, dst_buffer, src_lower, dst_lower, width)
    }
}

/// Copy a single row of pixels from the source buffer to the destination buffer, scaling them down by 2
fn copy_row_down(
    src_buffer: &[u8],
    dst_buffer: &mut [u8],
    src_lower: usize,
    dst_lower: usize,
    width: usize,
) {
    for (src, dst) in (src_lower * 8..(src_lower + width) * 8)
        .step_by(8)
        .zip((dst_lower * 4..(dst_lower + width) * 4).step_by(4))
    {
        dst_buffer[dst..dst + 4].copy_from_slice(&src_buffer[src..src + 4]);
    }
}

/// Converts HSV color values to RGB
///
/// # Arguments
/// * `hue` - Color hue in degrees (0-360)
/// * `saturation` - Color saturation (0.0-1.0)
/// * `value` - Color brightness/value (0.0-1.0)
///
/// # Returns
/// An RGB color
pub fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> (u8, u8, u8) {
    let hue = hue % 360.0;
    let c = value * saturation;
    let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = value - c;

    let (r, g, b) = match hue {
        h if h < 60.0 => (c, x, 0.0),
        h if h < 120.0 => (x, c, 0.0),
        h if h < 180.0 => (0.0, c, x),
        h if h < 240.0 => (0.0, x, c),
        h if h < 300.0 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };

    (
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    )
}
