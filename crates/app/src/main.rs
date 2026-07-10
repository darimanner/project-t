//! Thin binary: winit window + wiring of PTY -> VT -> renderer.

mod keys;

use std::io::Read;
use std::sync::{Arc, Mutex};

use core_pty::{PtySession, Size, SpawnConfig};
use core_vt::{GridSize, Vt};
use render::{Renderer, WgpuRenderer};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event::{ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};

/// Events sent from the PTY reader thread to the UI event loop.
#[derive(Debug)]
enum UserEvent {
    /// New terminal output was parsed; the window needs a redraw.
    Output,
    /// The shell exited.
    Exited,
}

fn main() {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("create event loop");
    let proxy = event_loop.create_proxy();
    let mut app = App {
        proxy,
        state: None,
        modifiers: ModifiersState::empty(),
    };
    event_loop.run_app(&mut app).expect("run event loop");
}

struct App {
    proxy: EventLoopProxy<UserEvent>,
    state: Option<State>,
    modifiers: ModifiersState,
}

struct State {
    renderer: WgpuRenderer,
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<PtySession>>,
    window: Arc<Window>,
}

impl State {
    fn new(event_loop: &ActiveEventLoop, proxy: EventLoopProxy<UserEvent>) -> Self {
        let attrs = Window::default_attributes()
            .with_title("project-t")
            .with_inner_size(LogicalSize::new(960.0, 640.0));
        let window = Arc::new(event_loop.create_window(attrs).expect("create window"));

        let size = window.inner_size();
        let scale = window.scale_factor() as f32;
        let renderer = WgpuRenderer::new(window.clone(), size.width, size.height, scale)
            .expect("create renderer");

        let (cols, rows) = grid_dimensions(&renderer, size);
        let vt = Arc::new(Mutex::new(Vt::new(
            GridSize {
                columns: cols as usize,
                screen_lines: rows as usize,
            },
            10_000,
        )));

        let pty =
            PtySession::spawn(&SpawnConfig::user_shell(Size { rows, cols })).expect("spawn shell");
        let reader = pty.reader().expect("clone pty reader");
        let pty = Arc::new(Mutex::new(pty));

        spawn_reader_thread(reader, vt.clone(), pty.clone(), proxy);

        Self {
            renderer,
            vt,
            pty,
            window,
        }
    }

    fn resize(&mut self, size: PhysicalSize<u32>) {
        self.renderer.resize(size.width, size.height);
        let (cols, rows) = grid_dimensions(&self.renderer, size);
        self.vt.lock().expect("vt lock").resize(GridSize {
            columns: cols as usize,
            screen_lines: rows as usize,
        });
        if let Err(err) = self
            .pty
            .lock()
            .expect("pty lock")
            .resize(Size { rows, cols })
        {
            eprintln!("pty resize failed: {err}");
        }
        self.window.request_redraw();
    }

    fn write_input(&self, bytes: &[u8]) {
        if let Err(err) = self.pty.lock().expect("pty lock").write_all(bytes) {
            eprintln!("pty write failed: {err}");
        }
    }
}

/// Pumps PTY output into the VT state machine and wakes the UI thread.
fn spawn_reader_thread(
    mut reader: Box<dyn Read + Send>,
    vt: Arc<Mutex<Vt>>,
    pty: Arc<Mutex<PtySession>>,
    proxy: EventLoopProxy<UserEvent>,
) {
    std::thread::spawn(move || {
        let mut buf = [0u8; 65536];
        loop {
            match reader.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let responses = {
                        let mut vt = vt.lock().expect("vt lock");
                        vt.advance(&buf[..n]);
                        vt.drain_pty_writes()
                    };
                    // Terminal replies (e.g. cursor position reports) go back
                    // to the application running inside the PTY.
                    for response in responses {
                        let _ = pty.lock().expect("pty lock").write_all(response.as_bytes());
                    }
                    if proxy.send_event(UserEvent::Output).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = proxy.send_event(UserEvent::Exited);
    });
}

fn grid_dimensions(renderer: &WgpuRenderer, size: PhysicalSize<u32>) -> (u16, u16) {
    let (cell_w, cell_h) = renderer.cell_size();
    let cols = ((size.width as f32 / cell_w).floor() as u16).max(2);
    let rows = ((size.height as f32 / cell_h).floor() as u16).max(2);
    (cols, rows)
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            self.state = Some(State::new(event_loop, self.proxy.clone()));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Output => {
                if let Some(state) = &self.state {
                    state.window.request_redraw();
                }
            }
            UserEvent::Exited => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        match event {
            WindowEvent::CloseRequested => {
                let _ = state.pty.lock().expect("pty lock").kill();
                event_loop.exit();
            }
            WindowEvent::Resized(size) => state.resize(size),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                state.renderer.set_scale_factor(scale_factor as f32);
                state.resize(state.window.inner_size());
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state == ElementState::Pressed
                    && let Some(bytes) = keys::encode(&event, self.modifiers)
                {
                    state.write_input(&bytes);
                }
            }
            WindowEvent::RedrawRequested => {
                let vt = state.vt.lock().expect("vt lock");
                if let Err(err) = state.renderer.draw(&vt) {
                    eprintln!("draw failed: {err}");
                }
            }
            _ => {}
        }
    }
}
