//! GPU terminal grid renderer: wgpu + glyphon/cosmic-text behind [`Renderer`].
//!
//! The trait is a deliberate seam: a direct Metal backend can be added later
//! without touching the app or the core crates.

mod color;
mod rects;

use core_vt::Vt;
use core_vt::alacritty_terminal::index::Point;
use core_vt::alacritty_terminal::term::cell::Flags;
use core_vt::alacritty_terminal::vte::ansi::CursorShape;
use glyphon::{
    Attrs, Buffer, Cache, Family, FontSystem, Metrics, Resolution, Shaping, SwashCache, TextArea,
    TextAtlas, TextBounds, TextRenderer, Viewport, Weight,
};
use rects::{RectInstance, RectRenderer};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("no suitable GPU adapter found")]
    Adapter(#[from] wgpu::RequestAdapterError),
    #[error("failed to request GPU device")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("failed to create render surface")]
    Surface(#[from] wgpu::CreateSurfaceError),
    #[error("failed to prepare text for rendering")]
    Prepare(#[from] glyphon::PrepareError),
    #[error("failed to draw text")]
    Text(#[from] glyphon::RenderError),
    #[error("render surface failed validation")]
    SurfaceValidation,
}

/// Draws the state of a [`Vt`] into a window surface.
pub trait Renderer {
    /// Handles a window resize, in physical pixels.
    fn resize(&mut self, width: u32, height: u32);

    /// Handles a monitor/DPI change.
    fn set_scale_factor(&mut self, scale: f32);

    /// Size of one character cell in physical pixels (width, height).
    fn cell_size(&self) -> (f32, f32);

    fn draw(&mut self, vt: &Vt) -> Result<(), Error>;
}

const FONT_SIZE: f32 = 14.0;
const LINE_HEIGHT_FACTOR: f32 = 1.25;

pub struct WgpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    surface_config: wgpu::SurfaceConfiguration,

    font_system: FontSystem,
    swash_cache: SwashCache,
    viewport: Viewport,
    atlas: TextAtlas,
    text_renderer: TextRenderer,
    text_buffer: Buffer,
    rect_renderer: RectRenderer,

    scale: f32,
    cell_w: f32,
    cell_h: f32,
}

impl WgpuRenderer {
    /// Creates a renderer for the given window; blocks on GPU setup.
    pub fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        width: u32,
        height: u32,
        scale: f32,
    ) -> Result<Self, Error> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(target)?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        }))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))?;

        let format = wgpu::TextureFormat::Bgra8UnormSrgb;
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: wgpu::CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        surface.configure(&device, &surface_config);

        let mut font_system = FontSystem::new();
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        let viewport = Viewport::new(&device, &cache);
        let mut atlas = TextAtlas::new(&device, &queue, &cache, format);
        let text_renderer =
            TextRenderer::new(&mut atlas, &device, wgpu::MultisampleState::default(), None);

        let metrics = metrics_for(scale);
        let (cell_w, cell_h) = measure_cell(&mut font_system, metrics);
        let mut text_buffer = Buffer::new(&mut font_system, metrics);
        text_buffer.set_size(Some(width as f32), Some(height as f32));

        let rect_renderer = RectRenderer::new(&device, format);
        rect_renderer.set_screen_size(&queue, width, height);

        Ok(Self {
            device,
            queue,
            surface,
            surface_config,
            font_system,
            swash_cache,
            viewport,
            atlas,
            text_renderer,
            text_buffer,
            rect_renderer,
            scale,
            cell_w,
            cell_h,
        })
    }

    /// Builds the text spans and background rects for the current frame.
    fn build_frame(&mut self, vt: &Vt) -> Vec<RectInstance> {
        let content = vt.term().renderable_content();
        let cursor_visible = content.cursor.shape != CursorShape::Hidden;
        let cursor_point = content.cursor.point;

        let mut rects: Vec<RectInstance> = Vec::new();
        // (text, fg sRGB, bold, dim) runs per line; rebuilt every frame.
        let mut spans: Vec<(String, [u8; 3], bool, bool)> = Vec::new();
        let mut current_line = 0i32;

        let push_char = |spans: &mut Vec<(String, [u8; 3], bool, bool)>,
                         c: char,
                         fg: [u8; 3],
                         bold: bool,
                         dim: bool| {
            match spans.last_mut() {
                Some((text, last_fg, last_bold, last_dim))
                    if *last_fg == fg && *last_bold == bold && *last_dim == dim =>
                {
                    text.push(c);
                }
                _ => spans.push((c.to_string(), fg, bold, dim)),
            }
        };

        for indexed in content.display_iter {
            let point: Point = indexed.point;
            if point.line.0 < 0 {
                continue;
            }
            while current_line < point.line.0 {
                push_char(&mut spans, '\n', color::DEFAULT_FG, false, false);
                current_line += 1;
            }

            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }

            let mut fg = color::resolve(cell.fg, content.colors);
            let mut bg = color::resolve(cell.bg, content.colors);
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }

            let is_cursor = cursor_visible && point == cursor_point && content.display_offset == 0;
            if is_cursor {
                // Block cursor: filled cell in fg color, glyph flipped to bg.
                bg = color::DEFAULT_FG;
                fg = color::DEFAULT_BG;
            }

            if bg != color::DEFAULT_BG {
                rects.push(RectInstance {
                    rect: [
                        point.column.0 as f32 * self.cell_w,
                        point.line.0 as f32 * self.cell_h,
                        self.cell_w,
                        self.cell_h,
                    ],
                    color: linear_rgba(bg),
                });
            }

            let c = if cell.c == '\t' { ' ' } else { cell.c };
            push_char(
                &mut spans,
                c,
                fg,
                cell.flags.contains(Flags::BOLD),
                cell.flags.contains(Flags::DIM),
            );
        }

        let default_attrs = Attrs::new().family(Family::Monospace);
        let rich: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|(text, fg, bold, dim)| {
                let mut c = *fg;
                if *dim {
                    c = c.map(|v| (v as f32 * 0.66) as u8);
                }
                let mut attrs = default_attrs
                    .clone()
                    .color(glyphon::Color::rgb(c[0], c[1], c[2]));
                if *bold {
                    attrs = attrs.weight(Weight::BOLD);
                }
                (text.as_str(), attrs)
            })
            .collect();

        self.text_buffer
            .set_rich_text(rich, &default_attrs, Shaping::Advanced, None);
        self.text_buffer
            .shape_until_scroll(&mut self.font_system, false);

        rects
    }
}

impl Renderer for WgpuRenderer {
    fn resize(&mut self, width: u32, height: u32) {
        self.surface_config.width = width.max(1);
        self.surface_config.height = height.max(1);
        self.surface.configure(&self.device, &self.surface_config);
        self.text_buffer
            .set_size(Some(width as f32), Some(height as f32));
        self.rect_renderer
            .set_screen_size(&self.queue, width, height);
    }

    fn set_scale_factor(&mut self, scale: f32) {
        self.scale = scale;
        let metrics = metrics_for(scale);
        let (w, h) = measure_cell(&mut self.font_system, metrics);
        self.cell_w = w;
        self.cell_h = h;
        self.text_buffer.set_metrics(metrics);
    }

    fn cell_size(&self) -> (f32, f32) {
        (self.cell_w, self.cell_h)
    }

    fn draw(&mut self, vt: &Vt) -> Result<(), Error> {
        let rects = self.build_frame(vt);
        self.rect_renderer.prepare(&self.device, &rects);

        self.viewport.update(
            &self.queue,
            Resolution {
                width: self.surface_config.width,
                height: self.surface_config.height,
            },
        );
        self.text_renderer.prepare(
            &self.device,
            &self.queue,
            &mut self.font_system,
            &mut self.atlas,
            &self.viewport,
            [TextArea {
                buffer: &self.text_buffer,
                left: 0.0,
                top: 0.0,
                scale: 1.0,
                bounds: TextBounds {
                    left: 0,
                    top: 0,
                    right: self.surface_config.width as i32,
                    bottom: self.surface_config.height as i32,
                },
                default_color: glyphon::Color::rgb(
                    color::DEFAULT_FG[0],
                    color::DEFAULT_FG[1],
                    color::DEFAULT_FG[2],
                ),
                custom_glyphs: &[],
            }],
            &mut self.swash_cache,
        )?;

        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(frame) => frame,
            wgpu::CurrentSurfaceTexture::Timeout | wgpu::CurrentSurfaceTexture::Occluded => {
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Suboptimal(_) => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Lost => {
                self.surface_config.width = self.surface_config.width.max(1);
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => return Err(Error::SurfaceValidation),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let bg = linear_rgba(color::DEFAULT_BG);
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: bg[0] as f64,
                            g: bg[1] as f64,
                            b: bg[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.rect_renderer.render(&mut pass);
            self.text_renderer
                .render(&self.atlas, &self.viewport, &mut pass)?;
        }

        self.queue.submit(Some(encoder.finish()));
        self.queue.present(frame);
        self.atlas.trim();
        Ok(())
    }
}

fn metrics_for(scale: f32) -> Metrics {
    let size = FONT_SIZE * scale;
    Metrics::new(size, (size * LINE_HEIGHT_FACTOR).ceil())
}

/// Measures the advance of one monospace glyph to derive the cell size.
fn measure_cell(font_system: &mut FontSystem, metrics: Metrics) -> (f32, f32) {
    let mut buffer = Buffer::new(font_system, metrics);
    buffer.set_text(
        "M",
        &Attrs::new().family(Family::Monospace),
        Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);
    let width = buffer
        .layout_runs()
        .next()
        .map(|run| run.line_w)
        .unwrap_or(metrics.font_size * 0.6);
    (width, metrics.line_height)
}

fn linear_rgba(srgb: [u8; 3]) -> [f32; 4] {
    [
        color::srgb8_to_linear(srgb[0]),
        color::srgb8_to_linear(srgb[1]),
        color::srgb8_to_linear(srgb[2]),
        1.0,
    ]
}
