use super::collaboration_tests::{select, setup};
use super::{BlockEditor, DocumentEvent};
use crate::{BlockEditorView, Document, EditorStyle, EditorTextStyle, ExtensionRegistry};
use gpui_kit::test::{TestSupportExt, TestWindowExt};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Context, Entity, Focusable, InteractiveElement,
    IntoElement, ParentElement, Render, SharedString, StyleRefinement, Styled, TestAppContext,
    Window, WindowBounds, WindowOptions, div, point, px, rgb, size,
};
use std::{cell::Cell, rc::Rc};

fn finish(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

struct StyledPair {
    left: Entity<BlockEditor>,
    right: Entity<BlockEditor>,
}
impl Render for StyledPair {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .child(
                div()
                    .id("styled-left")
                    .w(px(450.))
                    .h_full()
                    .test_support()
                    .child(BlockEditorView::new(&self.left).size_full().p_2()),
            )
            .child(
                div()
                    .id("styled-right")
                    .w(px(450.))
                    .h_full()
                    .test_support()
                    .child(BlockEditorView::new(&self.right).size_full()),
            )
    }
}

#[gpui_kit::test]
fn independent_style_overrides_change_layout_and_clear_back_to_defaults(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::init(cx);
    });
    let mut doc = Document::new();
    doc.insert_section(None, "# Heading\n\nA body paragraph")
        .unwrap();
    doc.clear_history();
    let mut peer = Document::new();
    peer.apply_update(&doc.encode_state()).unwrap();
    let before = doc.encode_state();
    let (window, pair) = cx.update(|cx| {
        gpui_kit::open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(1000.), px(900.)),
                ))),
                ..Default::default()
            },
            cx,
            |window, cx| {
                cx.new(|cx| {
                    let left_text = EditorTextStyle {
                        headings: std::array::from_fn(|index| {
                            if index == 0 {
                                StyleRefinement::default().text_size(px(36.))
                            } else {
                                StyleRefinement::default()
                            }
                        }),
                        ..Default::default()
                    };
                    let left_style = EditorStyle {
                        document: StyleRefinement::default().p(px(16.)),
                        content: StyleRefinement::default().max_w(px(320.)).gap(px(12.)),
                        ..Default::default()
                    }
                    .with_text(left_text);
                    let right_style = EditorStyle {
                        document: StyleRefinement::default().p(px(8.)),
                        content: StyleRefinement::default().max_w(px(220.)).gap(px(4.)),
                        ..Default::default()
                    }
                    .with_text(EditorTextStyle {
                        headings: std::array::from_fn(|index| {
                            if index == 0 {
                                StyleRefinement::default().text_size(px(18.))
                            } else {
                                StyleRefinement::default()
                            }
                        }),
                        ..Default::default()
                    });
                    StyledPair {
                        left: cx.new(|cx| {
                            BlockEditor::new(doc, ExtensionRegistry::default(), window, cx)
                                .with_style(left_style)
                        }),
                        right: cx.new(|cx| {
                            BlockEditor::new(peer, ExtensionRegistry::default(), window, cx)
                                .with_style(right_style)
                        }),
                    }
                })
            },
        )
        .unwrap()
    });
    let (left, right) = pair.read_with(cx, |pair, _| (pair.left.clone(), pair.right.clone()));
    finish(window.into(), cx);
    let order = left.read_with(cx, |editor, _| editor.order.clone());
    cx.update_window(window.into(), |_, window, _| {
        let left_heading = window
            .within("styled-left")
            .find(SharedString::from(order[0].clone()))
            .bounds();
        let right_heading = window
            .within("styled-right")
            .find(SharedString::from(order[0].clone()))
            .bounds();
        let left_body = window
            .within("styled-left")
            .find(SharedString::from(order[1].clone()))
            .bounds();
        let right_body = window
            .within("styled-right")
            .find(SharedString::from(order[1].clone()))
            .bounds();
        assert!((left_heading.size.width - px(320.)).abs() < px(1.));
        assert!((right_heading.size.width - px(220.)).abs() < px(1.));
        assert!(left_heading.size.height > right_heading.size.height);
        assert!((left_body.top() - left_heading.bottom() - px(12.)).abs() < px(1.));
        assert!((right_body.top() - right_heading.bottom() - px(4.)).abs() < px(1.));
    })
    .unwrap();
    left.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(window.into(), cx);
    cx.update_window(window.into(), |_, window, _| {
        let left_bounds = window
            .within("styled-left")
            .find(SharedString::from(order[0].clone()))
            .bounds();
        let right_bounds = window
            .within("styled-right")
            .find(SharedString::from(order[0].clone()))
            .bounds();
        assert!((left_bounds.size.width - px(394.)).abs() < px(1.));
        assert!((right_bounds.size.width - px(220.)).abs() < px(1.));
    })
    .unwrap();
    for editor in [&left, &right] {
        editor.read_with(cx, |editor, _| {
            assert_eq!(editor.document.encode_state(), before);
            assert!(!editor.document.can_undo());
        });
    }
}

#[gpui_kit::test]
fn code_style_updates_preserve_input_identity_selection_and_shared_undo(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```text\nab\ncd\n```", cx);
    finish(handle, cx);
    let events = Rc::new(Cell::new(0usize));
    let seen = events.clone();
    let subscription = cx.update(|cx| {
        cx.subscribe(&editor, move |_, _: &DocumentEvent, _| {
            seen.set(seen.get() + 1)
        })
    });
    let (input, line_height) = cx
        .update_window(handle, |_, window, cx| {
            select(&editor, &id, 0, 2, 2, window, cx);
            window.render_frame(cx);
            let input = editor.read(cx).views[&id]
                .editor
                .read(cx)
                .code_editor_state()
                .unwrap();
            window.input("X", cx);
            (input.clone(), input.read(cx).cursor_layout().unwrap().1)
        })
        .unwrap();
    finish(handle, cx);
    let before = editor.read_with(cx, |editor, _| editor.document.encode_state());
    let previous_events = events.get();
    let selection = input.read_with(cx, |input, _| input.selected_range());
    editor.update(cx, |editor, cx| {
        editor.set_style(
            EditorStyle::default().with_text(EditorTextStyle {
                code: StyleRefinement::default()
                    .text_size(px(28.))
                    .font_family("Menlo"),
                ..Default::default()
            }),
            cx,
        )
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        let current = editor.read(cx).views[&id]
            .editor
            .read(cx)
            .code_editor_state()
            .unwrap();
        assert_eq!(current, input);
        assert_eq!(input.read(cx).selected_range(), selection);
        assert!(input.read(cx).cursor_layout().unwrap().1 > line_height);
        assert!(input.focus_handle(cx).is_focused(window));
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.encode_state(), before)
    });
    assert_eq!(events.get(), previous_events);
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        assert!((input.read(cx).cursor_layout().unwrap().1 - line_height).abs() < px(1.));
        window.press("cmd-z", cx);
    })
    .unwrap();
    finish(handle, cx);
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections()[0].content.plain_text(), "ab\ncd");
        assert!(!editor.document.can_undo());
        assert!(editor.document.can_redo());
    });
    drop(subscription);
}

#[gpui_kit::test]
fn larger_hover_controls_keep_the_drag_centered_and_menu_attached(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("A block", cx);
    editor.update(cx, |editor, cx| {
        editor.set_style(
            EditorStyle {
                block: StyleRefinement::default().min_h(px(100.)),
                controls: StyleRefinement::default().size(px(44.)),
                drag_handle: StyleRefinement::default().w(px(36.)).h(px(48.)),
                slash_menu: StyleRefinement::default().w(px(300.)),
                slash_menu_heading: StyleRefinement::default().text_size(px(22.)),
                ..Default::default()
            },
            cx,
        )
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(id.clone()), cx);
        let block = window.find(SharedString::from(id.clone())).bounds();
        let grip = window.find(("grip", 0usize)).bounds();
        let button = window.find(("block-menu", 0usize)).bounds();
        assert!((grip.center().y - block.center().y).abs() < px(1.));
        assert!((button.size.height - px(44.)).abs() < px(1.));
        assert!((grip.size.height - px(48.)).abs() < px(1.));
        window.click(("block-menu", 0usize), cx);
        let menu = window.find("slash-menu").bounds();
        assert!((menu.size.width - px(300.)).abs() < px(1.));
        assert!((menu.left() - button.left()).abs() < px(2.));
        assert!(menu.top() >= button.bottom());
        assert!(menu.top() - button.bottom() <= px(8.));
    })
    .unwrap();
}

#[gpui_kit::test]
fn diagram_dialog_dimensions_and_padding_apply_once_and_reset_live(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```mermaid\nflowchart LR\n A --> B --> C\n```", cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    finish(handle, cx);
    let initial = cx
        .update_window(handle, |_, window, cx| {
            window.activate_window();
            window.hover(SharedString::from(id), cx);
            window.click(("expand-preview", 0usize), cx);
            window.within("dialog").find("surface").bounds()
        })
        .unwrap();
    finish(handle, cx);
    let color = rgb(0x15283b);
    editor.update(cx, |editor, cx| {
        editor.set_style(
            EditorStyle {
                diagram_viewer: StyleRefinement::default()
                    .w(px(640.))
                    .h(px(500.))
                    .p(px(12.))
                    .bg(color),
                ..Default::default()
            },
            cx,
        )
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        let surface = window.within("dialog").find("surface").bounds();
        let viewer = window.find("diagram-viewer").bounds();
        assert!((surface.size.width - px(640.)).abs() < px(1.));
        assert!((surface.size.height - px(500.)).abs() < px(1.));
        assert!((viewer.left() - surface.left() - px(13.)).abs() < px(1.));
        assert!((viewer.right() - surface.right() + px(13.)).abs() < px(1.));
        assert!(
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background.as_solid() == Some(color.into()))
        );
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        assert_eq!(window.within("dialog").find("surface").bounds(), initial);
    })
    .unwrap();
}

#[gpui_kit::test]
fn disabled_code_formatting_controls_do_not_paint_enabled_hover_styles(cx: &mut TestAppContext) {
    let (handle, editor, id) = setup("```rust\nlet value = 1;\n```", cx);
    let hover_color = rgb(0xe53691);
    editor.update(cx, |editor, cx| {
        editor.set_style(
            EditorStyle {
                controls_hover: StyleRefinement::default().bg(hover_color),
                ..Default::default()
            },
            cx,
        )
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 3, window, cx);
        window.render_frame(cx);
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        window.hover("selection-bold", cx);
        assert!(
            !window
                .painted_quads()
                .iter()
                .any(|quad| quad.background.as_solid() == Some(hover_color.into()))
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn structured_quotes_apply_specific_typography_and_surface_without_changing_content(
    cx: &mut TestAppContext,
) {
    let (handle, editor, _) = setup("> Quoted words\n\nOutside words", cx);
    let order = editor.read_with(cx, |editor, _| editor.order.clone());
    let before = editor.read_with(cx, |editor, _| editor.document.encode_state());
    let quote_color = rgb(0x405060);
    editor.update(cx, |editor, cx| {
        editor.set_style(
            EditorStyle::default().with_text(EditorTextStyle {
                body: StyleRefinement::default().text_size(px(16.)),
                quote: StyleRefinement::default()
                    .text_size(px(28.))
                    .bg(quote_color),
                ..Default::default()
            }),
            cx,
        )
    });
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &order[0], 0, 0, 6, window, cx);
    })
    .unwrap();
    finish(handle, cx);
    let quote_height = editor.read_with(cx, |editor, cx| {
        editor.views[&order[0]]
            .editor
            .read(cx)
            .selection_bounds()
            .unwrap()
            .size
            .height
    });
    cx.update_window(handle, |_, window, cx| {
        assert!(
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.background.as_solid() == Some(quote_color.into()))
        );
        select(&editor, &order[1], 0, 0, 6, window, cx);
    })
    .unwrap();
    finish(handle, cx);
    editor.read_with(cx, |editor, cx| {
        let outside_height = editor.views[&order[1]]
            .editor
            .read(cx)
            .selection_bounds()
            .unwrap()
            .size
            .height;
        assert!(quote_height > outside_height);
        assert_eq!(editor.document.encode_state(), before);
        assert!(!editor.document.can_undo());
    });
}
