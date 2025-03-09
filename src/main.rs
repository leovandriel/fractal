use sdl2::event::Event;
use sdl2::keyboard::Keycode;
use sdl2::mouse::MouseButton;
use sdl2::pixels::Color;
use sdl2::rect::{FPoint, FRect, Point};
use sdl2::render::{BlendMode, Canvas, Texture};
use sdl2::sys;
use sdl2::video::Window;
use std::time::Duration;

const WINDOW_WIDTH: u32 = 800;
const WINDOW_HEIGHT: u32 = 600;

const ALIASING_FACTOR: u32 = 2;
const BUFFER_WIDTH: u32 = WINDOW_WIDTH * ALIASING_FACTOR;
const BUFFER_HEIGHT: u32 = WINDOW_HEIGHT * ALIASING_FACTOR;
const MAX_UPDATE_PROGRESS: u32 = 20;

struct App {
    sdl_context: Option<sdl2::Sdl>,
    video_subsystem: Option<sdl2::VideoSubsystem>,
    canvas: Option<Canvas<Window>>,
    pixel_buffer: Vec<u8>,
    src_rect: FRect,
    dest_rect: FRect,
    dirty: bool,
    update_progress: u32,
    mouse_pos: Point,
    mouse_down: bool,
    shift_pressed: bool,
}

impl App {
    fn new() -> Self {
        App {
            sdl_context: None,
            video_subsystem: None,
            canvas: None,
            pixel_buffer: vec![0; (BUFFER_WIDTH * BUFFER_HEIGHT * 4) as usize],
            src_rect: FRect::new(0.0, 0.0, WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
            dest_rect: FRect::new(0.0, 0.0, WINDOW_WIDTH as f32, WINDOW_HEIGHT as f32),
            dirty: true,
            update_progress: 0,
            mouse_pos: Point::new(0, 0),
            mouse_down: false,
            shift_pressed: false,
        }
    }

    fn init(&mut self) -> Result<(), String> {
        let sdl_context = sdl2::init()?;
        let video_subsystem = sdl_context.video()?;

        let window = video_subsystem
            .window("Render", WINDOW_WIDTH, WINDOW_HEIGHT)
            .position_centered()
            .build()
            .unwrap();

        let canvas = window.into_canvas().build().unwrap();

        self.sdl_context = Some(sdl_context);
        self.video_subsystem = Some(video_subsystem);
        self.canvas = Some(canvas);

        Ok(())
    }

    fn handle_event(&mut self, event: Event) -> bool {
        match event {
            Event::Quit { .. }
            | Event::KeyDown {
                keycode: Some(Keycode::Escape),
                ..
            } => return true,

            Event::KeyDown {
                keycode: Some(Keycode::LShift | Keycode::RShift),
                ..
            } => {
                self.shift_pressed = true;
            }

            Event::KeyUp {
                keycode: Some(Keycode::LShift | Keycode::RShift),
                ..
            } => {
                self.shift_pressed = false;
            }

            Event::MouseButtonDown {
                x,
                y,
                mouse_btn: MouseButton::Left,
                ..
            } => {
                self.mouse_down = true;
                self.mouse_pos.x = x;
                self.mouse_pos.y = y;
            }

            Event::MouseButtonUp {
                mouse_btn: MouseButton::Left,
                ..
            } => {
                self.mouse_down = false;
            }

            Event::MouseMotion { x, y, .. } => {
                self.mouse_pos.x = x;
                self.mouse_pos.y = y;
            }

            _ => {}
        }
        false
    }

    fn fill_pixel_buffer(&mut self, texture: &mut Texture) {
        let rect = self.src_rect;
        let window_size = WINDOW_WIDTH.min(WINDOW_HEIGHT) as f32;
        let scale = FPoint::new(
            rect.w / BUFFER_WIDTH as f32 / window_size,
            rect.h / BUFFER_HEIGHT as f32 / window_size,
        );
        let offset = FPoint::new(
            (rect.x - WINDOW_WIDTH as f32 * 0.5) / window_size,
            (rect.y - WINDOW_HEIGHT as f32 * 0.5) / window_size,
        );
        let y_max = (self.update_progress + MAX_UPDATE_PROGRESS).min(BUFFER_HEIGHT);
        for y in self.update_progress..y_max {
            for x in 0..BUFFER_WIDTH {
                let index = ((y * BUFFER_WIDTH + x) * 4) as usize;
                let p = FPoint::new(x as f32 * scale.x + offset.x, y as f32 * scale.y + offset.y);
                let color = get_pixel_color(p);
                self.pixel_buffer[index] = color.r; // R
                self.pixel_buffer[index + 1] = color.g; // G
                self.pixel_buffer[index + 2] = color.b; // B
                self.pixel_buffer[index + 3] = color.a; // A
            }
        }
        self.update_progress = y_max;
        texture
            .update(None, &self.pixel_buffer, (BUFFER_WIDTH * 4) as usize)
            .unwrap();
    }

    fn run(&mut self) -> Result<(), String> {
        self.init()?;

        let mut canvas = self.canvas.take().unwrap();
        let texture_creator = canvas.texture_creator();
        let mut texture = texture_creator
            .create_texture_streaming(None, BUFFER_WIDTH, BUFFER_HEIGHT)
            .unwrap();
        texture.set_blend_mode(BlendMode::Blend);
        // TODO: ScaleMode added in next SDL2 release
        unsafe {
            sys::SDL_SetTextureScaleMode(texture.raw(), sys::SDL_ScaleMode::SDL_ScaleModeBest)
        };

        let mut event_pump = self.sdl_context.as_ref().unwrap().event_pump()?;

        'running: loop {
            for event in event_pump.poll_iter() {
                if self.handle_event(event) {
                    break 'running;
                }
            }

            // Only scale if mouse is down
            if self.mouse_down {
                let factor = if self.shift_pressed { 0.99 } else { 1.01 };
                // Adjust offset to keep mouse position stable
                self.dest_rect.x += (self.dest_rect.x - self.mouse_pos.x as f32) * (factor - 1.0);
                self.dest_rect.y += (self.dest_rect.y - self.mouse_pos.y as f32) * (factor - 1.0);
                // Scale in or out depending on shift key state
                self.dest_rect.w *= factor;
                self.dest_rect.h *= factor;

                self.dirty = true;
            }

            if self.dest_rect.w > WINDOW_WIDTH as f32 * 2.0
                || self.dest_rect.w < WINDOW_WIDTH as f32
            {
                let isup = self.dest_rect.w > WINDOW_WIDTH as f32;
                let factor = if isup { 0.5 } else { 2.0 };

                let scale = FPoint::new(
                    BUFFER_WIDTH as f32 / self.dest_rect.w,
                    BUFFER_HEIGHT as f32 / self.dest_rect.h,
                );

                // calculate the pixel offset as integer, so we snap to the nearest pixel
                let dx = ((WINDOW_WIDTH as f32 * 0.5
                    - self.dest_rect.w * 0.5 * factor
                    - self.dest_rect.x)
                    * scale.x) as i32;
                let dy = ((WINDOW_HEIGHT as f32 * 0.5
                    - self.dest_rect.h * 0.5 * factor
                    - self.dest_rect.y)
                    * scale.y) as i32;

                let mut new_pixel_buffer = vec![0; (BUFFER_WIDTH * BUFFER_HEIGHT * 4) as usize];

                for y in 0..BUFFER_HEIGHT as i32 {
                    for x in 0..BUFFER_WIDTH as i32 {
                        let xn = if isup { x / 2 } else { x * 2 } + dx;
                        let yn = if isup { y / 2 } else { y * 2 } + dy;
                        if xn >= 0
                            && (xn as u32) < BUFFER_WIDTH
                            && yn >= 0
                            && (yn as u32) < BUFFER_HEIGHT
                        {
                            let from = ((yn * BUFFER_WIDTH as i32 + xn) * 4) as usize;
                            let to = ((y * BUFFER_WIDTH as i32 + x) * 4) as usize;
                            new_pixel_buffer[to] = self.pixel_buffer[from];
                            new_pixel_buffer[to + 1] = self.pixel_buffer[from + 1];
                            new_pixel_buffer[to + 2] = self.pixel_buffer[from + 2];
                            new_pixel_buffer[to + 3] = self.pixel_buffer[from + 3];
                        }
                    }
                }

                self.src_rect.x += dx as f32 / scale.x / self.dest_rect.w * self.src_rect.w;
                self.src_rect.y += dy as f32 / scale.y / self.dest_rect.h * self.src_rect.h;
                self.src_rect.w *= factor;
                self.src_rect.h *= factor;

                self.dest_rect.x += dx as f32 / scale.x;
                self.dest_rect.y += dy as f32 / scale.y;
                self.dest_rect.w *= factor;
                self.dest_rect.h *= factor;

                self.update_progress = 0;
                self.pixel_buffer = new_pixel_buffer;
                self.dirty = true;
            }

            if self.update_progress < BUFFER_HEIGHT {
                self.fill_pixel_buffer(&mut texture);
                self.dirty = true;
            }

            if self.dirty {
                // Use the floating-point copy method
                canvas.clear();
                canvas.copy_f(&texture, None, self.dest_rect)?;
                canvas.present();
                self.dirty = false;
            }

            std::thread::sleep(Duration::new(0, 1_000_000_000u32 / 60));
        }

        self.canvas = Some(canvas);
        Ok(())
    }
}

fn get_pixel_color(point: FPoint) -> Color {
    // Map the point to the complex plane
    // Scale and translate to view the interesting part of the Mandelbrot set
    let c_real = point.x * 3.0 - 0.5;
    let c_imag = point.y * 3.0;

    // Mandelbrot iteration
    let max_iter = 100;
    let mut z_real = 0.0;
    let mut z_imag = 0.0;
    let mut iter = 0;

    while z_real * z_real + z_imag * z_imag < 4.0 && iter < max_iter {
        let temp = z_real * z_real - z_imag * z_imag + c_real;
        z_imag = 2.0 * z_real * z_imag + c_imag;
        z_real = temp;
        iter += 1;
    }

    if iter == max_iter {
        // Point is in the Mandelbrot set
        Color::RGB(0, 0, 0)
    } else {
        // Point is outside, color based on escape iterations
        let hue = (iter as f32 / max_iter as f32) * 360.0;
        let saturation = 0.8;
        let value = if iter < max_iter { 1.0 } else { 0.0 };

        // Simple HSV to RGB conversion
        let c = value * saturation;
        let x = c * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
        let m = value - c;

        let (r, g, b) = if hue < 60.0 {
            (c, x, 0.0)
        } else if hue < 120.0 {
            (x, c, 0.0)
        } else if hue < 180.0 {
            (0.0, c, x)
        } else if hue < 240.0 {
            (0.0, x, c)
        } else if hue < 300.0 {
            (x, 0.0, c)
        } else {
            (c, 0.0, x)
        };

        Color::RGB(
            ((r + m) * 255.0) as u8,
            ((g + m) * 255.0) as u8,
            ((b + m) * 255.0) as u8,
        )
    }
}

fn main() -> Result<(), String> {
    let mut app = App::new();
    app.run()?;
    Ok(())
}
