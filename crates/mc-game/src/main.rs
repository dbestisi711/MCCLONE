//! MCCLONE entry point: window, input mapping, fixed-tick game loop, and the
//! glue between worldgen, entities, UI and rendering.
//!
//! Run:          cargo run --release
//! Screenshot:   cargo run --release -- --screenshot out.png [--size 1280x720] [--seed N]
//!               [--time 6000] [--yaw DEG] [--pitch DEG] [--pos X,Y,Z] [--ui inventory]

mod game;
mod input_map;

use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorGrabMode, Window, WindowId};

use game::{Game, Options};
use mc_render::Renderer;

struct App {
    options: Options,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    game: Option<Game>,
    last_frame: Instant,
    grabbed: bool,
}

impl App {
    fn set_grab(&mut self, grab: bool) {
        let Some(w) = &self.window else { return };
        if grab == self.grabbed {
            return;
        }
        if grab {
            let ok = w
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| w.set_cursor_grab(CursorGrabMode::Confined))
                .is_ok();
            w.set_cursor_visible(!ok);
        } else {
            let _ = w.set_cursor_grab(CursorGrabMode::None);
            w.set_cursor_visible(true);
        }
        self.grabbed = grab;
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("MCCLONE")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        let game = Game::new(self.options.clone());
        let renderer = Renderer::new(window.clone(), game.assets.clone());
        log::info!("renderer: {}", renderer.adapter_info);
        self.window = Some(window);
        self.renderer = Some(renderer);
        self.game = Some(game);
        self.last_frame = Instant::now();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (Some(game), Some(renderer)) = (&mut self.game, &mut self.renderer) else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(text) = &event.text {
                    if event.state == ElementState::Pressed {
                        game.input.typed.push_str(text);
                    }
                }
                input_map::key_event(&mut game.input, &event);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(b) = input_map::mouse_button(button) {
                    match state {
                        ElementState::Pressed => {
                            game.input.mouse_held.insert(b);
                            game.input.mouse_pressed.insert(b);
                        }
                        ElementState::Released => {
                            game.input.mouse_held.remove(&b);
                            game.input.mouse_released.insert(b);
                        }
                    }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                game.input.scroll += match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
            }
            WindowEvent::CursorMoved { position, .. } => {
                game.input.cursor_pos = (position.x as f32, position.y as f32);
            }
            WindowEvent::Focused(false) => {
                game.input.held.clear();
                game.input.mouse_held.clear();
                game.pause();
            }
            WindowEvent::RedrawRequested => {
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.25);
                self.last_frame = now;
                let size = renderer.size();
                game.input.window_size = size;
                if let Some(w) = &self.window {
                    game.input.scale_factor = w.scale_factor() as f32;
                }
                game.input.cursor_grabbed = self.grabbed;
                let frame = game.frame(dt);
                renderer.render(&game.world, &frame);
                game.after_render(renderer);
                let want_grab = game.wants_cursor_grab();
                if game.quit_requested {
                    el.exit();
                }
                self.set_grab(want_grab);
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let (DeviceEvent::MouseMotion { delta }, Some(game)) = (event, &mut self.game) {
            if self.grabbed {
                game.input.mouse_delta.0 += delta.0 as f32;
                game.input.mouse_delta.1 += delta.1 as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let options = Options::from_args(std::env::args().skip(1).collect());

    if let Some(path) = options.screenshot.clone() {
        game::run_screenshot(&options, &path);
        return;
    }

    let el = EventLoop::new().expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        options,
        window: None,
        renderer: None,
        game: None,
        last_frame: Instant::now(),
        grabbed: false,
    };
    el.run_app(&mut app).expect("run app");
}
