use std::sync::Arc;

use crate::EditorStyle;
use crate::button::EditorButton as Button;
use gpui_kit::component::{ActiveTheme, StyledExt};
use gpui_kit::*;

const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 8.;
const ZOOM_STEP: f32 = 1.25;
const CANVAS_INSET: f32 = 24.;
const MAX_TEXTURE_SIDE: i32 = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
struct RasterSize {
    width: i32,
    height: i32,
}

struct ParsedPreview {
    svg: Arc<ParsedSvg>,
    aspect: f32,
    image: Arc<RenderImage>,
}

enum WorkResult {
    Parsed(ParsedPreview),
    Raster(Arc<RenderImage>),
}

/// A local camera over a diagram snapshot. It never holds or edits the document.
pub(crate) struct DiagramViewer {
    source: Option<Arc<Image>>,
    renderer: SvgRenderer,
    parsed: Option<Arc<ParsedSvg>>,
    aspect: Option<f32>,
    raster: Option<Arc<RenderImage>>,
    error: Option<String>,
    source_generation: u64,
    work: Option<Task<()>>,
    raster_size: Option<RasterSize>,
    viewport: Bounds<Pixels>,
    scale_factor: f32,
    zoom: f32,
    pan: Point<Pixels>,
    drag_position: Option<Point<Pixels>>,
    style: Arc<EditorStyle>,
}

impl DiagramViewer {
    pub(crate) fn new(image: Arc<Image>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut viewer = Self {
            source: None,
            renderer: cx.svg_renderer(),
            parsed: None,
            aspect: None,
            raster: None,
            error: None,
            source_generation: 0,
            work: None,
            raster_size: None,
            viewport: Bounds::default(),
            scale_factor: window.scale_factor(),
            zoom: 1.,
            pan: point(px(0.), px(0.)),
            drag_position: None,
            style: Arc::new(EditorStyle::default()),
        };
        viewer.set_preview(Some(image), None, cx);
        viewer
    }

    pub(crate) fn set_style(&mut self, style: Arc<EditorStyle>, cx: &mut Context<Self>) {
        self.style = style;
        // Geometry is measured during prepaint. Only a changed viewport requests
        // another raster; changing colors must not reset the camera or source.
        cx.notify();
    }

    fn style_button(&self, button: Button) -> Button {
        button.appearance(self.style.clone())
    }

    pub(crate) fn set_preview(
        &mut self,
        image: Option<Arc<Image>>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if error.is_none()
            && self.error.is_none()
            && match (&self.source, &image) {
                (Some(previous), Some(next)) => Arc::ptr_eq(previous, next),
                (None, None) => true,
                _ => false,
            }
        {
            return;
        }
        self.source_generation = self.source_generation.wrapping_add(1);
        self.source = if error.is_some() { None } else { image };
        self.error = error;
        self.parsed = None;
        self.aspect = None;
        // Never show a previous source while its replacement is loading or failed.
        self.raster = None;
        self.raster_size = None;
        self.drag_position = None;
        // Keep the in-flight worker until completion: refreshes and wheel events
        // coalesce behind one job rather than queueing unbounded rasterizations.
        self.start_work(cx);
        cx.notify();
    }

    fn fitted_size(&self) -> Option<Size<Pixels>> {
        let aspect = self.aspect?;
        let width = (f32::from(self.viewport.size.width) - 2. * CANVAS_INSET).max(1.);
        let height = (f32::from(self.viewport.size.height) - 2. * CANVAS_INSET).max(1.);
        let width = width.min(height * aspect);
        Some(size(px(width), px(width / aspect)))
    }

    fn desired_raster_size(&self) -> Option<RasterSize> {
        if self.viewport.size.width <= px(0.) || self.viewport.size.height <= px(0.) {
            return None;
        }
        let fitted = self.fitted_size()?;
        let width = f32::from(fitted.width) * self.zoom * self.scale_factor;
        let height = f32::from(fitted.height) * self.zoom * self.scale_factor;
        let scale = (MAX_TEXTURE_SIDE as f32 / width.max(height)).min(1.);
        Some(RasterSize {
            width: (width * scale).ceil().clamp(1., MAX_TEXTURE_SIDE as f32) as i32,
            height: (height * scale).ceil().clamp(1., MAX_TEXTURE_SIDE as f32) as i32,
        })
    }

    fn start_work(&mut self, cx: &mut Context<Self>) {
        if self.work.is_some() || self.error.is_some() {
            return;
        }
        let Some(source) = self.source.clone() else {
            return;
        };
        let parsed = self.parsed.clone();
        let requested_size = self.desired_raster_size();
        if parsed.is_some() && (requested_size.is_none() || requested_size == self.raster_size) {
            return;
        }
        let generation = self.source_generation;
        let renderer = self.renderer.clone();
        let job = cx.background_executor().spawn(async move {
            if let Some(parsed) = parsed {
                let requested = requested_size.expect("parsed SVG has a raster target");
                rasterize(&renderer, &parsed, requested.width).map(WorkResult::Raster)
            } else {
                if source.format != ImageFormat::Svg {
                    return Err("The diagram preview is not an SVG image.".to_owned());
                }
                // Borrow the retained source bytes once, off the UI thread. The
                // native renderer bounds this initial aspect probe to 8192px;
                // rasterize further bounds the retained texture to 4096px.
                let svg = Arc::new(
                    renderer
                        .parse_svg(&source.bytes)
                        .map_err(|error| format!("Could not parse diagram: {error}"))?,
                );
                let image = rasterize(&renderer, &svg, 512)?;
                let dimensions = image.size(0);
                Ok(WorkResult::Parsed(ParsedPreview {
                    svg,
                    aspect: dimensions.width.0 as f32 / dimensions.height.0 as f32,
                    image,
                }))
            }
        });
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = job.await;
            let _ = this.update(cx, |this, cx| {
                this.work = None;
                if this.source_generation == generation {
                    match result {
                        Ok(WorkResult::Parsed(preview)) => {
                            this.parsed = Some(preview.svg);
                            this.aspect = Some(preview.aspect);
                            this.raster = Some(preview.image);
                            this.raster_size = None;
                        }
                        Ok(WorkResult::Raster(image)) => {
                            // A newer camera target must not be overwritten by
                            // work requested at an older zoom or viewport size.
                            if this.desired_raster_size() == requested_size {
                                this.raster = Some(image);
                                this.raster_size = requested_size;
                            }
                        }
                        Err(error) => {
                            if requested_size.is_none()
                                || this.desired_raster_size() == requested_size
                            {
                                this.raster = None;
                                this.error = Some(error);
                            }
                        }
                    }
                }
                this.start_work(cx);
                cx.notify();
            });
        }));
    }

    fn set_viewport(&mut self, bounds: Bounds<Pixels>, scale_factor: f32, cx: &mut Context<Self>) {
        if self.viewport == bounds && self.scale_factor == scale_factor {
            return;
        }
        self.viewport = bounds;
        self.scale_factor = scale_factor;
        self.start_work(cx);
        cx.notify();
    }

    fn zoom_at(&mut self, zoom: f32, anchor: Point<Pixels>, cx: &mut Context<Self>) {
        if !zoom.is_finite() || self.aspect.is_none() {
            return;
        }
        let zoom = zoom.clamp(MIN_ZOOM, MAX_ZOOM);
        if zoom == self.zoom {
            return;
        }
        let center = self.viewport.center();
        let ratio = zoom / self.zoom;
        // Keep the diagram point under the pointer fixed in window coordinates.
        self.pan = point(
            anchor.x - center.x - (anchor.x - center.x - self.pan.x) * ratio,
            anchor.y - center.y - (anchor.y - center.y - self.pan.y) * ratio,
        );
        self.zoom = zoom;
        self.start_work(cx);
        cx.notify();
    }

    fn fit(&mut self, cx: &mut Context<Self>) {
        self.zoom = 1.;
        self.pan = point(px(0.), px(0.));
        self.drag_position = None;
        self.start_work(cx);
        cx.notify();
    }

    fn finish_pan(&mut self, cx: &mut Context<Self>) -> bool {
        if self.drag_position.take().is_some() {
            cx.notify();
            true
        } else {
            false
        }
    }
}

impl Render for DiagramViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted = cx.theme().muted_foreground;
        let ready = self.raster.is_some() && self.error.is_none();
        let percentage = format!("{:.0}%", self.zoom * 100.);
        let zoom_label = div()
            .id("diagram-zoom-label")
            .aria_label(percentage.clone())
            .w(px(52.))
            .text_center()
            .text_xs()
            .text_color(muted)
            .child(percentage);
        #[cfg(test)]
        let zoom_label = {
            use gpui_kit::test::TestSupportExt;
            zoom_label.test_support()
        };
        let header = div()
            .id("diagram-toolbar")
            .h(px(48.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_1()
            .px_3()
            // The dialog owns an absolute close button in the upper right.
            .pr(px(52.))
            .border_b_1()
            .border_color(cx.theme().border)
            .refine_style(&self.style.diagram_toolbar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_medium()
                    .child("Diagram"),
            )
            .child(
                self.style_button(
                    Button::new("diagram-zoom-out")
                        .ghost()
                        .small()
                        .label("−")
                        .tooltip("Zoom out")
                        .disabled(!ready || self.zoom <= MIN_ZOOM)
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.zoom_at(this.zoom / ZOOM_STEP, this.viewport.center(), cx);
                        })),
                ),
            )
            .child(zoom_label)
            .child(
                self.style_button(
                    Button::new("diagram-zoom-in")
                        .ghost()
                        .small()
                        .label("+")
                        .tooltip("Zoom in")
                        .disabled(!ready || self.zoom >= MAX_ZOOM)
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.zoom_at(this.zoom * ZOOM_STEP, this.viewport.center(), cx);
                        })),
                ),
            )
            .child(
                self.style_button(
                    Button::new("diagram-fit")
                        .ghost()
                        .small()
                        .label("Fit")
                        .tooltip("Fit diagram to window")
                        .disabled(!ready)
                        .on_click(cx.listener(|this, _, _, cx| {
                            cx.stop_propagation();
                            this.fit(cx);
                        })),
                ),
            );
        #[cfg(test)]
        let header = {
            use gpui_kit::test::TestSupportExt;
            header.test_support()
        };
        let geometry_owner = cx.weak_entity();
        let pointer_owner = cx.weak_entity();
        let mut viewport = div()
            .id("diagram-viewport")
            .relative()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_hidden()
            .bg(cx.theme().muted)
            .refine_style(&self.style.diagram_canvas)
            .cursor(if self.drag_position.is_some() {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    window.prevent_default();
                    if this.raster.is_some() && this.error.is_none() {
                        this.drag_position = Some(event.position);
                        cx.notify();
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.finish_pan(cx);
                    cx.stop_propagation();
                }),
            )
            .on_scroll(cx.listener(|this, event: &ScrollEvent, window, cx| {
                cx.stop_propagation();
                window.prevent_default();
                let delta = f32::from(event.delta.pixel_delta(px(24.)).y);
                if delta.is_finite() {
                    let factor = (delta * 0.0025).clamp(-1., 1.).exp();
                    this.zoom_at(this.zoom * factor, event.position, cx);
                }
            }))
            .child(
                canvas(
                    move |bounds, window, cx| {
                        let _ = geometry_owner.update(cx, |this, cx| {
                            this.set_viewport(bounds, window.scale_factor(), cx);
                        });
                    },
                    move |_, _, window, _| {
                        // Window-level capture keeps panning through an edge and
                        // releases the camera even when mouse-up is outside us.
                        let move_owner = pointer_owner.clone();
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, _, cx| {
                            if phase != DispatchPhase::Capture {
                                return;
                            }
                            let _ = move_owner.update(cx, |this, cx| {
                                let Some(previous) = this.drag_position else {
                                    return;
                                };
                                if !event.dragging() {
                                    this.finish_pan(cx);
                                    return;
                                }
                                this.pan += event.position - previous;
                                this.drag_position = Some(event.position);
                                cx.stop_propagation();
                                cx.notify();
                            });
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Capture && event.button == MouseButton::Left
                            {
                                let _ = pointer_owner.update(cx, |this, cx| {
                                    if this.finish_pan(cx) {
                                        cx.stop_propagation();
                                    }
                                });
                            }
                        });
                    },
                )
                .absolute()
                .inset_0(),
            );
        if let Some(error) = &self.error {
            let error = div()
                .id("diagram-error")
                .aria_label(error.clone())
                .p_6()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(error.clone());
            #[cfg(test)]
            let error = {
                use gpui_kit::test::TestSupportExt;
                error.test_support()
            };
            viewport = viewport.child(error);
        } else if let Some(image) = &self.raster {
            if let Some(fitted) = self.fitted_size() {
                let image_size = size(fitted.width * self.zoom, fitted.height * self.zoom);
                let origin = point(
                    (self.viewport.size.width - image_size.width) / 2. + self.pan.x,
                    (self.viewport.size.height - image_size.height) / 2. + self.pan.y,
                );
                let image = img(ImageSource::Render(image.clone()))
                    .object_fit(ObjectFit::Contain)
                    .id("diagram-image")
                    .aria_label("Expanded diagram")
                    .absolute()
                    .left(origin.x)
                    .top(origin.y)
                    .w(image_size.width)
                    .h(image_size.height);
                #[cfg(test)]
                let image = {
                    use gpui_kit::test::TestSupportExt;
                    image.test_support()
                };
                viewport = viewport.child(image);
            }
        } else {
            viewport = viewport.child(
                div()
                    .p_6()
                    .text_sm()
                    .text_color(muted)
                    .child("Rendering diagram…"),
            );
        }
        #[cfg(test)]
        let viewport = {
            use gpui_kit::test::TestSupportExt;
            viewport.test_support()
        };
        let body = div()
            .id("diagram-viewer")
            .size_full()
            .min_h_0()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll(|_, window, cx| {
                window.prevent_default();
                cx.stop_propagation();
            })
            .child(header)
            .child(viewport);
        #[cfg(test)]
        let body = {
            use gpui_kit::test::TestSupportExt;
            body.test_support()
        };
        body
    }
}

fn rasterize(
    renderer: &SvgRenderer,
    parsed: &ParsedSvg,
    width: i32,
) -> Result<Arc<RenderImage>, String> {
    let render = |width| {
        renderer
            .render_parsed(
                parsed,
                SvgSize::Size(size(DevicePixels(width), DevicePixels(1))),
            )
            .map_err(|error| format!("Could not render diagram: {error}"))
    };
    let image = render(width)?;
    let dimensions = image.size(0);
    let longest = dimensions.width.0.max(dimensions.height.0);
    if longest > MAX_TEXTURE_SIDE {
        // Size preserves the SVG's true aspect ratio, not the rounded probe's.
        // A tall SVG can exceed our limit on the probe or at a rounding edge.
        let width = ((dimensions.width.0 as f64 * MAX_TEXTURE_SIDE as f64 / longest as f64).floor()
            as i32)
            .max(1);
        return render(width);
    }
    Ok(image)
}
