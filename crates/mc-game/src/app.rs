//! Window, event loop and per-frame driving, shared by the desktop binary and
//! the web build.
//!
//! Desktop: the pack is found on disk and the renderer is created
//! synchronously in `resumed`.
//! Web: the canvas is created in `resumed`; the pack bundle is fetched and the
//! renderer created asynchronously, and the finished game is handed back to
//! the event loop as a user event.

use std::sync::Arc;

use web_time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::game::{Game, Options};
use crate::input_map;
use crate::touch::{Phase, TouchControls};
use mc_render::Renderer;

/// Sent to the event loop when asynchronous start-up (web) has finished.
pub enum UserEvent {
    Ready(Box<(Game, Renderer)>),
    Failed(String),
}

struct App {
    options: Options,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    proxy: EventLoopProxy<UserEvent>,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    game: Option<Game>,
    last_frame: Instant,
    grabbed: bool,
    touch: TouchControls,
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

    /// Browsers only grant pointer lock in response to a click, and release
    /// it themselves when Esc is pressed. Mirror the real lock state.
    #[cfg(target_arch = "wasm32")]
    fn sync_pointer_lock(&mut self) {
        let locked = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.pointer_lock_element())
            .is_some();
        if self.grabbed && !locked {
            // The browser released the lock (Esc): show the pause menu.
            if let Some(game) = &mut self.game {
                if game.wants_cursor_grab() {
                    game.pause();
                }
            }
        }
        self.grabbed = locked;
    }

    fn start(&mut self, game: Game, mut renderer: Renderer) {
        renderer.set_render_distance(self.options.render_distance);
        log::info!("renderer: {}", renderer.adapter_info);
        self.renderer = Some(renderer);
        self.game = Some(game);
        self.last_frame = Instant::now();
        set_status("");
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes().with_title("MCCLONE");
        // Desktop: a fixed starting size. Web: no size, so the page's CSS
        // makes the canvas fill the browser window.
        #[cfg(not(target_arch = "wasm32"))]
        let attrs = attrs.with_inner_size(winit::dpi::LogicalSize::new(1280.0, 720.0));
        #[cfg(target_arch = "wasm32")]
        let attrs = {
            use winit::platform::web::WindowAttributesExtWebSys;
            attrs.with_append(true).with_prevent_default(true)
        };
        let window = Arc::new(el.create_window(attrs).expect("create window"));
        self.window = Some(window.clone());

        #[cfg(not(target_arch = "wasm32"))]
        {
            let game = Game::new(self.options.clone());
            let renderer = Renderer::new(window, game.assets.clone());
            self.start(game, renderer);
        }
        #[cfg(target_arch = "wasm32")]
        {
            let options = self.options.clone();
            let proxy = self.proxy.clone();
            wasm_bindgen_futures::spawn_local(async move {
                let event = match web::start_game(options, window).await {
                    Ok(pair) => UserEvent::Ready(Box::new(pair)),
                    Err(e) => UserEvent::Failed(e),
                };
                let _ = proxy.send_event(event);
            });
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Ready(pair) => {
                let (game, renderer) = *pair;
                self.start(game, renderer);
            }
            UserEvent::Failed(e) => {
                log::error!("start-up failed: {e}");
                set_status(&format!("Failed to start: {e}"));
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let WindowEvent::CloseRequested = event {
            el.exit();
            return;
        }
        let (Some(game), Some(renderer)) = (&mut self.game, &mut self.renderer) else {
            return;
        };
        match event {
            WindowEvent::Resized(size) => renderer.resize(size.width, size.height),
            WindowEvent::KeyboardInput { event, .. } => {
                if let Some(text) = &event.text {
                    if event.state == ElementState::Pressed {
                        game.input.typed.push_str(text);
                    }
                }
                input_map::key_event(&mut game.input, &event);
                if event.state == ElementState::Pressed {
                    if let Some(d) = input_map::render_distance_delta(&event) {
                        game.change_render_distance(d);
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                // On the web, pointer lock may only be requested from a click.
                let want_grab = game.wants_cursor_grab();
                if cfg!(target_arch = "wasm32")
                    && !self.touch.active
                    && state == ElementState::Pressed
                    && want_grab
                    && !self.grabbed
                {
                    if let Some(w) = &self.window {
                        let _ = w.set_cursor_grab(CursorGrabMode::Locked);
                    }
                    // This click only captures the mouse.
                    return;
                }
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
            WindowEvent::Touch(t) => {
                let phase = match t.phase {
                    winit::event::TouchPhase::Started => Phase::Started,
                    winit::event::TouchPhase::Moved => Phase::Moved,
                    winit::event::TouchPhase::Ended => Phase::Ended,
                    winit::event::TouchPhase::Cancelled => Phase::Cancelled,
                };
                let pos = glam::Vec2::new(t.location.x as f32, t.location.y as f32);
                self.touch.event(game, t.id, phase, pos);
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
                #[cfg(target_arch = "wasm32")]
                {
                    // Re-borrow after the lock sync (it may pause the game).
                    let _ = (game, renderer);
                    self.sync_pointer_lock();
                }
                let (Some(game), Some(renderer)) = (&mut self.game, &mut self.renderer) else {
                    return;
                };
                let now = Instant::now();
                let dt = (now - self.last_frame).as_secs_f32().min(0.25);
                self.last_frame = now;
                let size = renderer.size();
                game.input.window_size = size;
                if let Some(w) = &self.window {
                    game.input.scale_factor = w.scale_factor() as f32;
                }
                game.input.cursor_grabbed = self.grabbed;
                self.touch.update(game);
                let mut frame = game.frame(dt);
                self.touch.draw(&mut frame.ui, game.ui.screen_open());
                renderer.render(&game.world, &frame);
                game.after_render(renderer);
                let want_grab = game.wants_cursor_grab();
                if game.quit_requested {
                    el.exit();
                }
                if self.touch.active {
                    // Touch play never captures the pointer.
                } else if cfg!(target_arch = "wasm32") {
                    // Grabbing happens on click; only release here.
                    if !want_grab {
                        self.set_grab(false);
                    }
                } else {
                    self.set_grab(want_grab);
                }
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

/// Show a start-up status line in the page (web only).
fn set_status(_msg: &str) {
    #[cfg(target_arch = "wasm32")]
    web::set_status(_msg);
}

/// Desktop entry point: parse arguments, then run the screenshot tool or the game.
#[cfg(not(target_arch = "wasm32"))]
pub fn run() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info,wgpu_core=warn,wgpu_hal=warn,naga=warn"),
    )
    .init();
    let options = Options::from_args(std::env::args().skip(1).collect());

    if let Some(path) = options.screenshot.clone() {
        crate::game::run_screenshot(&options, &path);
        return;
    }

    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        options,
        proxy: el.create_proxy(),
        window: None,
        renderer: None,
        game: None,
        last_frame: Instant::now(),
        grabbed: false,
        touch: TouchControls::new(),
    };
    el.run_app(&mut app).expect("run app");
}

/// Web entry point (called from `index.html` via wasm-bindgen).
#[cfg(target_arch = "wasm32")]
pub fn run() {
    use winit::platform::web::EventLoopExtWebSys;
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    let options = Options::from_args(web::query_args());
    let el = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    el.set_control_flow(ControlFlow::Poll);
    let app = App {
        options,
        proxy: el.create_proxy(),
        window: None,
        renderer: None,
        game: None,
        last_frame: Instant::now(),
        grabbed: false,
        touch: TouchControls::new(),
    };
    el.spawn_app(app);
}

#[cfg(target_arch = "wasm32")]
mod web {
    use std::sync::Arc;

    use mc_assets::Pack;
    use mc_render::Renderer;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::*;
    use winit::window::Window;

    use crate::game::{Game, Options};

    #[wasm_bindgen(start)]
    pub fn wasm_start() {
        super::run();
    }

    /// `?seed=1&rd=6&survival` → `["--seed", "1", "--rd", "6", "--survival"]`.
    pub fn query_args() -> Vec<String> {
        let search = web_sys::window()
            .and_then(|w| w.location().search().ok())
            .unwrap_or_default();
        let mut args = Vec::new();
        for pair in search
            .trim_start_matches('?')
            .split('&')
            .filter(|p| !p.is_empty())
        {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            args.push(format!("--{k}"));
            if !v.is_empty() {
                args.push(v.to_string());
            }
        }
        // Without worker threads, keep the default view distance modest.
        if !args.iter().any(|a| a == "--rd" || a == "--render-distance") {
            args.push("--rd".into());
            args.push("6".into());
        }
        args
    }

    pub fn set_status(msg: &str) {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("status"))
        {
            el.set_text_content(Some(msg));
        }
    }

    async fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
        let window = web_sys::window().ok_or("no window")?;
        let resp = wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(url))
            .await
            .map_err(|e| format!("fetch {url}: {e:?}"))?;
        let resp: web_sys::Response = resp.dyn_into().map_err(|_| "bad response")?;
        if !resp.ok() {
            return Err(format!("fetch {url}: HTTP {}", resp.status()));
        }
        let buf = wasm_bindgen_futures::JsFuture::from(
            resp.array_buffer().map_err(|e| format!("{e:?}"))?,
        )
        .await
        .map_err(|e| format!("read {url}: {e:?}"))?;
        Ok(js_sys::Uint8Array::new(&buf).to_vec())
    }

    /// Yield to the browser so status text can repaint between slow steps.
    async fn yield_now() {
        let promise = js_sys::Promise::new(&mut |resolve, _| {
            if let Some(w) = web_sys::window() {
                let _ = w.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, 0);
            }
        });
        let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
    }

    pub async fn start_game(
        options: Options,
        window: Arc<Window>,
    ) -> Result<(Game, Renderer), String> {
        set_status("Downloading resource pack…");
        let bytes = fetch_bytes("pack.bin").await?;
        set_status("Loading assets and generating the world…");
        yield_now().await;
        let pack = Pack::from_bundle(&bytes)?;
        drop(bytes);
        let game = Game::with_pack(options, pack);
        set_status("Starting the renderer…");
        yield_now().await;
        let renderer = Renderer::new_async(window, game.assets.clone()).await;
        Ok((game, renderer))
    }
}
