use rug::Float;
use sdl2::{
    EventPump,
    event::{Event, WindowEvent},
    keyboard::Keycode,
    mouse::MouseButton,
    rect::{FPoint, FRect},
    render::{BlendMode, Texture},
    sys,
};
use std::{
    error::Error,
    fmt,
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

mod pixel;
use pixel::{
    Point32, ScaleDirection, Size32, extend_buffer, hsv_to_rgb, scale_rect, translate_rect,
};

#[derive(Debug, Clone, Copy)]
struct Config {
    /// Size of the window in pixels
    window_size: Size32,
    /// Anti-aliasing multiplier for the render buffer
    aliasing_factor: u32,
    /// Speed multiplier for zooming in/out
    zoom_factor: f32,
    /// Target frames per second for the main loop
    target_fps: f32,
    /// Number of worker threads for parallel rendering
    worker_threads: usize,
    /// Decay factor for motion after input (zoom/pan)
    motion_decay: f32,
    /// Maximum number of iterations for escape calculation
    max_iter: u32,
    /// Iteration divisor for color cycling
    color_cycle: u32,
    /// Color saturation (HSV)
    saturation: f32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            window_size: Size32 { w: 800, h: 600 },
            aliasing_factor: 2,
            zoom_factor: 0.01,
            target_fps: 60.0,
            worker_threads: thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4),
            motion_decay: 0.9,
            max_iter: 10000,
            color_cycle: 10,
            saturation: 0.8,
        }
    }
}

impl Config {
    fn buffer_size(&self) -> Size32 {
        Size32 {
            w: self.window_size.w * self.aliasing_factor,
            h: self.window_size.h * self.aliasing_factor,
        }
    }

    fn buffer_pitch(&self) -> u32 {
        self.window_size.w * self.aliasing_factor * 4
    }

    fn buffer_length(&self) -> u32 {
        self.window_size.w * self.window_size.h * self.aliasing_factor * self.aliasing_factor * 4
    }

    fn target_frame_duration(&self) -> Duration {
        Duration::from_secs_f32(1.0 / self.target_fps)
    }
}

#[derive(Debug)]
enum AppError {
    SdlError(String),
    IoError(std::io::Error),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SdlError(msg) => write!(f, "SDL error: {msg}"),
            Self::IoError(e) => write!(f, "IO error: {e}"),
        }
    }
}

impl Error for AppError {}

impl From<String> for AppError {
    fn from(err: String) -> Self {
        Self::SdlError(err)
    }
}

impl From<std::io::Error> for AppError {
    fn from(err: std::io::Error) -> Self {
        Self::IoError(err)
    }
}

struct App {
    config: Config,
    buffer: Arc<Mutex<Buffer>>,
    canvas: Canvas,
    input: Input,
    update_title: bool,
}

struct Buffer {
    size: Size32,
    data: Vec<u8>,
    rect: Rect,
    progress: u32,
    max_iter: u32,
    flush: bool,
    exit: bool,
}

#[derive(Debug, Clone)]
struct Rect {
    x: Float,
    y: Float,
    scale_exp: f32,
    scale_prec: u32,
}

impl Rect {
    fn new(window_size: Size32, aliasing_factor: u32) -> Self {
        const SCALE_PRECISION: u32 = 16;
        let mut rect = Self {
            x: Float::new(1),
            y: Float::new(1),
            scale_exp: 0.0,
            scale_prec: SCALE_PRECISION,
        };
        let min_size = window_size.w.min(window_size.h);
        rect.scale_mul(1.0 / (min_size as f32 * aliasing_factor as f32));
        rect.offset_add(Point32 {
            x: window_size.w as i32 * aliasing_factor as i32 / -2,
            y: window_size.h as i32 * aliasing_factor as i32 / -2,
        });
        rect
    }

    fn precision(&self) -> u32 {
        self.scale_exp as u32 + self.scale_prec
    }

    fn scale_mul(&mut self, factor: f32) {
        self.scale_exp -= factor.log2();
        let prec = self.precision();
        self.x.set_prec(prec);
        self.y.set_prec(prec);
    }

    fn scale(&self) -> Float {
        Float::with_val(self.precision(), -self.scale_exp).exp2()
    }

    fn offset_add(&mut self, delta: Point32) {
        let scale: Float = self.scale();
        self.x += Float::with_val(self.x.prec(), delta.x) * &scale;
        self.y += Float::with_val(self.y.prec(), delta.y) * &scale;
    }

    fn high_precision(&self) -> bool {
        const F64_BITS: u32 = 52;
        self.scale_exp > F64_BITS as f32
    }
}

struct Canvas {
    offset: FPoint,
    scale: f32,
    flush: bool,
    recreate: bool,
}

struct Input {
    mouse_position: Point32,
    mouse_movement: FPoint,
    mouse_moving: bool,
    mouse_scroll: f32,
    mouse_scrolling: bool,
    mouse_down: bool,
    shift_down: bool,
}

impl App {
    fn new(config: Config) -> Self {
        Self {
            config,
            update_title: true,
            buffer: Arc::new(Mutex::new(Buffer {
                size: config.buffer_size(),
                data: vec![0; config.buffer_length() as usize],
                rect: Rect::new(config.window_size, config.aliasing_factor),
                progress: 0,
                max_iter: config.max_iter,
                flush: false,
                exit: false,
            })),
            canvas: Canvas {
                offset: FPoint::new(0.0, 0.0),
                scale: 1.0,
                flush: true,
                recreate: false,
            },
            input: Input {
                mouse_position: Point32 { x: 0, y: 0 },
                mouse_movement: FPoint::new(0.0, 0.0),
                mouse_moving: false,
                mouse_scroll: 0.0,
                mouse_scrolling: false,
                mouse_down: false,
                shift_down: false,
            },
        }
    }

    fn update_window_title(&mut self, window: &mut sdl2::video::Window) {
        let buffer = self.buffer.lock().unwrap();
        let min_size: u32 = self.config.window_size.w.min(self.config.window_size.h);
        let offset = (min_size as f32 * self.config.aliasing_factor as f32).log2();
        let ooms = (buffer.rect.scale_exp - offset) * (2.0 as f32).log10();
        let precision = if buffer.rect.high_precision() {
            "MPFR"
        } else {
            "f64"
        };
        let title = format!("Fractal - 10^{:.0} - {}", ooms, precision);
        window.set_title(&title).unwrap_or_else(|e| {
            eprintln!("Failed to update window title: {}", e);
        });
    }

    fn run(&mut self) -> Result<(), AppError> {
        let sdl_context = sdl2::init().map_err(|e| AppError::SdlError(e.to_string()))?;
        let video_subsystem = sdl_context
            .video()
            .map_err(|e| AppError::SdlError(e.to_string()))?;

        let size = self.config.window_size;
        let window = video_subsystem
            .window("Fractal", size.w, size.h)
            .position_centered()
            .resizable()
            .build()
            .map_err(|e| AppError::SdlError(e.to_string()))?;

        let mut canvas = window
            .into_canvas()
            .build()
            .map_err(|e| AppError::SdlError(e.to_string()))?;

        let texture_creator = canvas.texture_creator();
        let mut texture = self.create_texture(&texture_creator)?;

        let mut event_pump = sdl_context
            .event_pump()
            .map_err(|e| AppError::SdlError(e.to_string()))?;

        let workers = self.start_workers();

        while self.handle_events(&mut event_pump) {
            let frame_start = Instant::now();

            // Check if texture needs to be recreated after a resize
            if self.canvas.recreate {
                texture = self.create_texture(&texture_creator)?;
                self.canvas.flush = true;
                self.canvas.recreate = false;
            }

            // Pan on mouse down
            if self.input.mouse_moving
                || self.input.mouse_movement.x.abs() > 0.5
                || self.input.mouse_movement.y.abs() > 0.5
            {
                self.pan(self.input.mouse_movement);
                self.input.mouse_movement.x *= self.config.motion_decay;
                self.input.mouse_movement.y *= self.config.motion_decay;
                self.input.mouse_moving = false;
                self.canvas.flush = true;
            }

            // Zoom on mouse scroll
            if self.input.mouse_scrolling || self.input.mouse_scroll.abs() > 0.5 {
                self.zoom(self.input.mouse_scroll);
                self.input.mouse_scroll *= self.config.motion_decay;
                self.input.mouse_scrolling = false;
                self.canvas.flush = true;
            }

            // Scale up or down when scale out of bounds
            if self.canvas.scale > 2.2 {
                self.scale(ScaleDirection::Up)
            } else if self.canvas.scale < 1.1 {
                self.scale(ScaleDirection::Down);
            }

            // Pan buffer when out of bounds
            if self.canvas.offset.x > 0.0
                || self.canvas.offset.x
                    < self.config.window_size.w as f32 * (1.0 - self.canvas.scale)
                || self.canvas.offset.y > 0.0
                || self.canvas.offset.y
                    < self.config.window_size.h as f32 * (1.0 - self.canvas.scale)
            {
                self.translate();
            }

            // Update texture
            {
                let mut buffer = self.buffer.lock().unwrap();
                if buffer.flush {
                    texture
                        .update(None, &buffer.data, self.config.buffer_pitch() as usize)
                        .map_err(|e| AppError::SdlError(e.to_string()))?;
                    self.canvas.flush = true;
                    buffer.flush = false;
                }
            }

            // Render texture
            if self.canvas.flush {
                canvas.clear();
                let rect = FRect::new(
                    self.canvas.offset.x,
                    self.canvas.offset.y,
                    self.canvas.scale * self.config.window_size.w as f32,
                    self.canvas.scale * self.config.window_size.h as f32,
                );
                canvas.copy_f(&texture, None, rect)?;
                canvas.present();
                self.canvas.flush = false;
            }

            if self.update_title {
                self.update_window_title(canvas.window_mut());
                self.update_title = false;
            }

            // Sleep if we're running too fast
            let frame_duration = frame_start.elapsed();
            if frame_duration < self.config.target_frame_duration() {
                thread::sleep(self.config.target_frame_duration() - frame_duration);
            }
        }

        // Wait for all workers to finish
        self.join_workers(workers)?;

        Ok(())
    }

    fn create_texture<'a>(
        &self,
        texture_creator: &'a sdl2::render::TextureCreator<sdl2::video::WindowContext>,
    ) -> Result<sdl2::render::Texture<'a>, AppError> {
        let size = self.config.buffer_size();
        let mut texture = texture_creator
            .create_texture_streaming(None, size.w, size.h)
            .map_err(|e| AppError::SdlError(e.to_string()))?;
        texture.set_blend_mode(BlendMode::Blend);
        set_scale_mode_best(&mut texture);
        Ok(texture)
    }

    fn start_workers(&mut self) -> Vec<thread::JoinHandle<()>> {
        let mut handles = Vec::with_capacity(self.config.worker_threads);
        let color_cycle = self.config.color_cycle;
        let saturation = self.config.saturation;

        for _ in 0..self.config.worker_threads {
            let buffer = Arc::clone(&self.buffer);
            let handle = thread::spawn(move || {
                loop {
                    let (progress, rect, size, max_iter) = {
                        let mut buffer = buffer.lock().unwrap();
                        if buffer.exit {
                            break;
                        }
                        buffer.progress += 1;
                        (
                            buffer.progress - 1,
                            buffer.rect.clone(),
                            buffer.size,
                            buffer.max_iter,
                        )
                    };

                    if progress >= size.h {
                        thread::sleep(Duration::from_millis(10));
                        continue;
                    }

                    // interlace randomly
                    let y = (progress * 31) % size.h;
                    let row_buffer =
                        App::fill_pixel_row(y, &rect, size.w, max_iter, color_cycle, saturation);

                    {
                        let mut buffer = buffer.lock().unwrap();
                        if buffer.rect.scale_exp == rect.scale_exp
                            && buffer.rect.x == rect.x
                            && buffer.rect.y == rect.y
                            && buffer.size.w == size.w
                            && buffer.size.h == size.h
                        {
                            let buffer_index = (y * size.w * 4) as usize;
                            buffer.data[buffer_index..buffer_index + size.w as usize * 4]
                                .copy_from_slice(&row_buffer);
                            buffer.flush = true;
                        }
                    }
                }
            });
            handles.push(handle);
        }
        handles
    }

    fn join_workers(
        &mut self,
        worker_handles: Vec<thread::JoinHandle<()>>,
    ) -> Result<(), AppError> {
        self.buffer.lock().unwrap().exit = true;
        for handle in worker_handles {
            handle.join().map_err(|e| {
                AppError::IoError(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("Thread join error: {:?}", e),
                ))
            })?;
        }
        Ok(())
    }

    fn fill_pixel_row(
        y: u32,
        rect: &Rect,
        width: u32,
        max_iter: u32,
        color_cycle: u32,
        saturation: f32,
    ) -> Vec<u8> {
        let mut row_buffer = vec![0; width as usize * 4];
        let prec = rect.precision();
        let scale = rect.scale();
        for x in 0..width as usize {
            let px = Float::with_val(prec, x) * &scale + &(rect.x);
            let py = Float::with_val(prec, y) * &scale + &(rect.y);
            let (r, g, b) = App::get_pixel_color(
                px,
                py,
                max_iter,
                rect.high_precision(),
                color_cycle,
                saturation,
            );
            let index = x * 4;
            row_buffer[index] = r;
            row_buffer[index + 1] = g;
            row_buffer[index + 2] = b;
            row_buffer[index + 3] = 0xFF;
        }
        row_buffer
    }

    fn get_pixel_color(
        x: Float,
        y: Float,
        max_iter: u32,
        high_precision: bool,
        color_cycle: u32,
        saturation: f32,
    ) -> (u8, u8, u8) {
        let mut c_real = x;
        c_real *= 3.0;
        c_real -= 0.5;
        let mut c_imag = y;
        c_imag *= 3.0;
        let (iter, mag_sq) = if high_precision {
            App::get_pixel_color_float(&c_real, &c_imag, max_iter)
        } else {
            App::get_pixel_color_f64(c_real.to_f64(), c_imag.to_f64(), max_iter)
        };
        if mag_sq < 4.0 {
            return (0, 0, 0);
        }
        let sub_iter = 4.5 / mag_sq - 0.125;
        let hue = (iter as f32 + sub_iter).sqrt() / color_cycle as f32 * 360.0;
        return hsv_to_rgb(hue, saturation, 1.0);
    }

    fn get_pixel_color_f64(real: f64, imag: f64, max_iter: u32) -> (u32, f32) {
        let c_real = real;
        let c_imag = imag;
        let mut z_real = 0.0;
        let mut z_imag = 0.0;

        for iter in 0..max_iter {
            let real_sq = z_real * z_real;
            let imag_sq = z_imag * z_imag;
            let mag_sq = real_sq + imag_sq;

            // Check if point escapes
            if mag_sq > 4.0 {
                return (iter, mag_sq as f32);
            }

            // Apply the Mandelbrot iteration: z = z² + c
            z_imag = 2.0 * z_real * z_imag + c_imag;
            z_real = real_sq - imag_sq + c_real;
        }

        // Point is in the Mandelbrot set (black)
        return (0, 0.0);
    }

    fn get_pixel_color_float(real: &Float, imag: &Float, max_iter: u32) -> (u32, f32) {
        let prec: u32 = real.prec();
        let four = Float::with_val(prec, 4);
        let c_real = real.clone();
        let c_imag = imag.clone();
        let mut z_real = Float::with_val(prec, 0);
        let mut z_imag = Float::with_val(prec, 0);

        for iter in 0..max_iter {
            let mut real_sq = z_real.clone();
            real_sq.square_mut();
            let mut imag_sq = z_imag.clone();
            imag_sq.square_mut();
            let mut mag_sq = real_sq.clone();
            mag_sq += &imag_sq;

            // Check if point escapes
            if mag_sq > four {
                return (iter, mag_sq.to_f32());
            }

            // Apply the Mandelbrot iteration: z = z² + c
            // z_imag = 2.0 * z_real * z_imag + c_imag;
            z_real <<= 1;
            z_imag.mul_add_mut(&z_real, &c_imag);
            // z_real = real_sq - imag_sq + c_real;
            z_real = real_sq;
            z_real -= &imag_sq;
            z_real += &c_real;
        }

        // Point is in the Mandelbrot set (black)
        return (0, 0.0);
    }

    fn handle_events(&mut self, event_pump: &mut EventPump) -> bool {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. }
                | Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                } => return false,
                Event::Window {
                    win_event: WindowEvent::Resized(w, h),
                    ..
                } => {
                    self.resize(Size32 {
                        w: w as u32,
                        h: h as u32,
                    });
                }
                Event::KeyDown {
                    keycode: Some(Keycode::LShift | Keycode::RShift),
                    ..
                } => {
                    self.input.shift_down = true;
                }
                Event::KeyUp {
                    keycode: Some(Keycode::LShift | Keycode::RShift),
                    ..
                } => {
                    self.input.shift_down = false;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Plus | Keycode::KpPlus | Keycode::Equals),
                    ..
                } => {
                    self.input.mouse_scroll = 10.0;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Minus | Keycode::KpMinus | Keycode::Underscore),
                    ..
                } => {
                    self.input.mouse_scroll = -10.0;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::LeftBracket),
                    ..
                } => {
                    let mut buffer = self.buffer.lock().unwrap();
                    buffer.max_iter = buffer.max_iter.saturating_sub(1000);
                    buffer.progress = 0;
                }
                Event::KeyDown {
                    keycode: Some(Keycode::RightBracket),
                    ..
                } => {
                    let mut buffer = self.buffer.lock().unwrap();
                    buffer.max_iter = buffer.max_iter.saturating_add(1000);
                    buffer.progress = 0;
                }
                Event::MouseButtonDown {
                    x,
                    y,
                    mouse_btn: MouseButton::Left,
                    clicks,
                    ..
                } => {
                    self.input.mouse_down = true;
                    self.input.mouse_position.x = x;
                    self.input.mouse_position.y = y;
                    if clicks == 2 {
                        self.input.mouse_scroll = if self.input.shift_down { -10.0 } else { 10.0 };
                    }
                }
                Event::MouseButtonUp {
                    mouse_btn: MouseButton::Left,
                    ..
                } => {
                    self.input.mouse_down = false;
                }
                Event::MouseMotion { x, y, .. } => {
                    if self.input.mouse_down {
                        self.input.mouse_movement.x = (x - self.input.mouse_position.x) as f32;
                        self.input.mouse_movement.y = (y - self.input.mouse_position.y) as f32;
                        self.input.mouse_moving = true;
                    }
                    self.input.mouse_position.x = x;
                    self.input.mouse_position.y = y;
                }
                Event::MouseWheel {
                    y: mouse_scroll, ..
                } => {
                    self.input.mouse_scroll = mouse_scroll as f32;
                    self.input.mouse_scrolling = true;
                }
                _ => {}
            }
        }
        true
    }

    fn zoom(&mut self, multiplier: f32) {
        let zoom = multiplier * self.config.zoom_factor;
        // Adjust offset to keep mouse position stable
        self.canvas.offset.x += (self.canvas.offset.x
            - self
                .input
                .mouse_position
                .x
                .clamp(0, self.config.window_size.w as i32) as f32)
            * zoom;
        self.canvas.offset.y += (self.canvas.offset.y
            - self
                .input
                .mouse_position
                .y
                .clamp(0, self.config.window_size.h as i32) as f32)
            * zoom;
        // Scale in or out depending on shift key
        self.canvas.scale *= 1.0 + zoom;
    }

    fn scale(&mut self, direction: ScaleDirection) {
        // calculate the pixel offset as integer, so we snap to the nearest pixel
        let factor = match direction {
            ScaleDirection::Up => 0.5,
            ScaleDirection::Down => 2.0,
        };
        let offset = Point32 {
            x: (((self.config.window_size.w as f32 - self.canvas.offset.x * 2.0)
                / self.canvas.scale)
                - self.config.window_size.w as f32 * factor) as i32
                / 2,
            y: (((self.config.window_size.h as f32 - self.canvas.offset.y * 2.0)
                / self.canvas.scale)
                - self.config.window_size.h as f32 * factor) as i32
                / 2,
        };
        let delta = Point32 {
            x: offset.x * self.config.aliasing_factor as i32,
            y: offset.y * self.config.aliasing_factor as i32,
        };

        self.canvas.offset.x += offset.x as f32 * self.canvas.scale;
        self.canvas.offset.y += offset.y as f32 * self.canvas.scale;
        self.canvas.scale *= factor;

        let mut buffer = self.buffer.lock().unwrap();
        buffer.data = scale_rect(
            &buffer.data,
            self.config.buffer_size(),
            self.config.buffer_pitch(),
            delta,
            direction,
        );

        buffer.rect.offset_add(delta);
        buffer.rect.scale_mul(factor);
        buffer.progress = 0;
        buffer.flush = true;
        drop(buffer);

        // Update window title after scale_exp changes
        self.update_title = true;
    }

    fn pan(&mut self, movement: FPoint) {
        self.canvas.offset.x += movement.x;
        self.canvas.offset.y += movement.y;
    }

    fn translate(&mut self) {
        let delta = Point32 {
            x: (self.config.window_size.w as f32 / 2.0
                - (self.canvas.offset.x
                    + self.canvas.scale * self.config.window_size.w as f32 / 2.0))
                as i32,
            y: (self.config.window_size.h as f32 / 2.0
                - (self.canvas.offset.y
                    + self.canvas.scale * self.config.window_size.h as f32 / 2.0))
                as i32,
        };

        self.canvas.offset.x +=
            delta.x as f32 * self.canvas.scale / self.config.aliasing_factor as f32;
        self.canvas.offset.y +=
            delta.y as f32 * self.canvas.scale / self.config.aliasing_factor as f32;

        let mut buffer = self.buffer.lock().unwrap();
        buffer.data = translate_rect(
            &buffer.data,
            self.config.buffer_size(),
            self.config.buffer_pitch(),
            delta,
        );

        buffer.rect.offset_add(delta);
        buffer.progress = 0;
        buffer.flush = true;
    }

    fn resize(&mut self, size: Size32) {
        let buffer_size = self.config.buffer_size();
        let buffer_pitch = self.config.buffer_pitch();
        self.config.window_size = size;

        let mut buffer = self.buffer.lock().unwrap();
        buffer.data = extend_buffer(
            &buffer.data,
            buffer_size,
            buffer_pitch,
            self.config.buffer_size(),
            self.config.buffer_pitch(),
        );
        buffer.size = self.config.buffer_size();

        buffer.progress = 0;
        buffer.flush = true;

        self.canvas.recreate = true; // Signal texture recreation
    }
}

fn main() -> Result<(), AppError> {
    let mut app = App::new(Config::default());
    app.run()?;
    Ok(())
}

/// Set the blend mode of a texture (to be replaced with rust binding when available)
fn set_scale_mode_best(texture: &mut Texture) {
    unsafe {
        let result =
            sys::SDL_SetTextureScaleMode(texture.raw(), sys::SDL_ScaleMode::SDL_ScaleModeBest);
        if result != 0 {
            eprintln!("Failed to set texture scale mode");
        }
    }
}
