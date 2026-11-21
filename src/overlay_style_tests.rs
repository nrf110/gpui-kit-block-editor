use super::collaboration_tests::{replica, select, setup};
use crate::{EditorStyle, ExtensionRegistry, FileMediaStore, MediaKind, MediaStore};
use gpui_kit::test::TestWindowExt;
use gpui_kit::{
    AnyWindowHandle, AppContext, Focusable, SharedString, Styled, TestAppContext,
    VisualTestContext, div, point, px,
};
use std::{path::PathBuf, sync::Arc};

fn finish(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

#[gpui_kit::test]
fn selection_and_link_surfaces_reflow_locally_and_reset_without_touching_selection(
    cx: &mut TestAppContext,
) {
    let (first, editor, id) = setup("Selected words", cx);
    let (second, other, other_id) = setup("Selected words", cx);
    for (handle, editor, id) in [(first, &editor, &id), (second, &other, &other_id)] {
        cx.update_window(handle, |_, window, cx| {
            select(editor, id, 0, 0, 8, window, cx);
            window.render_frame(cx);
            window.click("selection-link", cx);
        })
        .unwrap();
    }
    let baseline = cx
        .update_window(first, |_, window, _| {
            (
                window.find("selection-toolbar").bounds(),
                window.find("selection-link-editor").bounds(),
            )
        })
        .unwrap();
    let before = editor.read_with(cx, |editor, cx| {
        (
            editor.document.encode_state(),
            editor.views[&id].editor.read(cx).selection().clone(),
            editor.scroll.offset(),
        )
    });
    let mut style = EditorStyle::default();
    style.selection_toolbar = div().p(px(16.)).style().clone();
    style.link_editor = div().p(px(24.)).style().clone();
    editor.update(cx, |editor, cx| editor.set_style(style, cx));
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert!(window.find("selection-toolbar").bounds().size.height > baseline.0.size.height);
        assert!(window.find("selection-link-editor").bounds().size.height > baseline.1.size.height);
        let editor = editor.read(cx);
        assert_eq!(editor.document.encode_state(), before.0);
        assert_eq!(editor.views[&id].editor.read(cx).selection(), &before.1);
        assert_eq!(editor.scroll.offset(), before.2);
        assert!(!editor.document.can_undo());
    })
    .unwrap();
    cx.update_window(second, |_, window, cx| {
        window.render_frame(cx);
        assert_eq!(
            window.find("selection-toolbar").bounds().size,
            baseline.0.size
        );
        assert_eq!(
            window.find("selection-link-editor").bounds().size,
            baseline.1.size
        );
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert_eq!(
            window.find("selection-toolbar").bounds().size,
            baseline.0.size
        );
        assert_eq!(
            window.find("selection-link-editor").bounds().size,
            baseline.1.size
        );
        // The same focused input and draft survive the appearance change.
        window.input("https://example.com", cx);
        window.click("selection-link-apply", cx);
        assert!(editor.read(cx).error.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn open_language_menu_restyles_and_resets_without_losing_highlight_or_scroll(
    cx: &mut TestAppContext,
) {
    let source = format!(
        "```rust\nlet x = 1;\n```{}",
        "\n\nScroll sentinel".repeat(50)
    );
    let (first, editor, _) = setup(&source, cx);
    let (second, _, _) = setup(&source, cx);
    for handle in [first, second] {
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            window.click(("code-language", 0usize), cx)
        })
        .unwrap();
        finish(handle, cx);
    }
    let baseline = cx
        .update_window(first, |_, window, cx| {
            for _ in 0..11 {
                window.press("down", cx);
            }
            window.render_frame(cx);
            (
                window.find("popup-menu").bounds().size,
                window
                    .within("popup-menu")
                    .find_by_label("Go")
                    .bounds()
                    .top(),
            )
        })
        .unwrap();
    let before = editor.read_with(cx, |editor, _| {
        (editor.document.encode_state(), editor.scroll.offset())
    });
    let mut style = EditorStyle::default();
    style.popup_menu = div()
        .min_w(px(320.))
        .bg(gpui_kit::rgb(0x203040))
        .style()
        .clone();
    editor.update(cx, |editor, cx| editor.set_style(style, cx));
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert!(window.find("popup-menu").bounds().size.width >= px(320.));
        assert_eq!(
            window
                .within("popup-menu")
                .find_by_label("Go")
                .bounds()
                .top(),
            baseline.1
        );
        assert_eq!(editor.read(cx).document.encode_state(), before.0);
        assert_eq!(editor.read(cx).scroll.offset(), before.1);
    })
    .unwrap();
    cx.update_window(second, |_, window, _| {
        assert_eq!(window.find("popup-menu").bounds().size, baseline.0);
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert_eq!(window.find("popup-menu").bounds().size, baseline.0);
        assert_eq!(
            window
                .within("popup-menu")
                .find_by_label("Go")
                .bounds()
                .top(),
            baseline.1
        );
        window.press("enter", cx);
        assert!(window.try_find("popup-menu").is_none());
        let editor = editor.read(cx);
        let section = &editor.document.sections()[0];
        assert_eq!(section.content.blocks[0].attrs["lang"].as_str(), Some("go"));
        assert!(
            editor.views[&section.id]
                .editor
                .focus_handle(cx)
                .is_focused(window)
        );
    })
    .unwrap();
}

#[gpui_kit::test]
fn diagram_style_updates_keep_zoom_pan_and_other_viewers_independent(cx: &mut TestAppContext) {
    let source = "```mermaid\nflowchart LR\n A --> B --> C --> D --> E\n```";
    let (first, editor, id) = setup(source, cx);
    let (second, _, second_id) = setup(source, cx);
    cx.update(|cx| cx.set_reduce_motion(true));
    for (handle, id) in [(first, &id), (second, &second_id)] {
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            window.activate_window();
            window.hover(SharedString::from(id.clone()), cx);
            window.click(("expand-preview", 0usize), cx);
        })
        .unwrap();
        finish(handle, cx);
    }
    let before = cx
        .update_window(first, |_, window, cx| {
            window.click("diagram-zoom-in", cx);
            let center = window.find("diagram-viewport").bounds().center();
            window.drag(center, center + point(px(39.), px(27.)), cx);
            window.render_frame(cx);
            (
                window.find("diagram-image").bounds(),
                window.find("diagram-viewport").bounds(),
                window.find("diagram-toolbar").bounds(),
            )
        })
        .unwrap();
    let document = editor.read_with(cx, |editor, _| {
        (editor.document.encode_state(), editor.scroll.offset())
    });
    let mut style = EditorStyle::default();
    style.diagram_canvas = div().bg(gpui_kit::rgb(0x304050)).style().clone();
    style.diagram_toolbar = div().h(px(72.)).style().clone();
    editor.update(cx, |editor, cx| editor.set_style(style, cx));
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        let image = window.find("diagram-image").bounds();
        let canvas = window.find("diagram-viewport").bounds();
        assert_eq!(window.find("diagram-toolbar").bounds().size.height, px(72.));
        assert_eq!(image.size, before.0.size);
        assert_eq!(
            image.center() - canvas.center(),
            before.0.center() - before.1.center()
        );
        assert_eq!(editor.read(cx).document.encode_state(), document.0);
        assert_eq!(editor.read(cx).scroll.offset(), document.1);
    })
    .unwrap();
    cx.update_window(second, |_, window, _| {
        assert_eq!(
            window.find("diagram-toolbar").bounds().size.height,
            before.2.size.height
        );
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert_eq!(window.find("diagram-toolbar").bounds(), before.2);
        assert_eq!(window.find("diagram-image").bounds(), before.0);
        window.click("diagram-fit", cx);
        assert!(window.find("diagram-image").bounds().size.width < before.0.size.width);
    })
    .unwrap();
}

struct Assets(PathBuf);
impl Drop for Assets {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[gpui_kit::test]
fn retained_image_cards_reflow_per_instance_and_reset_without_replacing_media(
    cx: &mut TestAppContext,
) {
    let files =
        Assets(std::env::temp_dir().join(format!("overlay-style-{}", uuid::Uuid::new_v4())));
    std::fs::create_dir_all(&files.0).unwrap();
    let path = files.0.join("image.svg");
    std::fs::write(&path, b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"320\" height=\"160\"><rect width=\"320\" height=\"160\" fill=\"blue\"/></svg>").unwrap();
    let store = Arc::new(FileMediaStore::new(files.0.join("assets")));
    let attachment = store.import_file(&path, Some(MediaKind::Image)).unwrap();
    let (first, editor, id) = setup("seed", cx);
    let (second, other, other_id) = setup("seed", cx);
    for (handle, editor, id) in [(first, &editor, &id), (second, &other, &other_id)] {
        editor.update(cx, |editor, _| {
            editor.extensions =
                Arc::new(ExtensionRegistry::default().with_media_store(store.clone()));
        });
        let mut remote = replica(editor, cx);
        remote
            .set_content(id, &attachment.to_content().unwrap())
            .unwrap();
        let update = remote.encode_state();
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.apply_update(&update, window, cx).unwrap()
            });
        })
        .unwrap();
        finish(handle, cx);
    }
    let baseline = cx
        .update_window(first, |_, window, _| {
            window.find("media-attachment").bounds()
        })
        .unwrap();
    let image_before = VisualTestContext::from_window(first, cx)
        .debug_bounds("media-image")
        .unwrap();
    let before = editor.read_with(cx, |editor, _| {
        (
            editor.document.encode_state(),
            editor.views[&id].media.as_ref().unwrap().1.clone(),
        )
    });
    let mut style = EditorStyle::default();
    style.media = div().max_w(px(360.)).p(px(12.)).style().clone();
    editor.update(cx, |editor, cx| editor.set_style(style, cx));
    finish(first, cx);
    let image_after = VisualTestContext::from_window(first, cx)
        .debug_bounds("media-image")
        .unwrap();
    assert!(image_after.size.width < image_before.size.width);
    assert!(
        (f32::from(image_after.size.width) / f32::from(image_after.size.height) - 2.).abs() < 0.01
    );
    cx.update_window(first, |_, window, cx| {
        assert!(window.find("media-attachment").bounds().size.width <= px(360.));
        let editor = editor.read(cx);
        assert_eq!(editor.document.encode_state(), before.0);
        assert_eq!(editor.views[&id].media.as_ref().unwrap().1, before.1);
    })
    .unwrap();
    cx.update_window(second, |_, window, _| {
        assert_eq!(window.find("media-attachment").bounds().size, baseline.size);
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(first, cx);
    cx.update_window(first, |_, window, _| {
        assert_eq!(window.find("media-attachment").bounds(), baseline);
    })
    .unwrap();
    assert_eq!(
        VisualTestContext::from_window(first, cx)
            .debug_bounds("media-image")
            .unwrap(),
        image_before
    );
}

#[gpui_kit::test]
fn style_and_list_popups_reset_dimensions_and_contain_scroll(cx: &mut TestAppContext) {
    let source = format!("Selected words{}", "\n\nScroll sentinel".repeat(50));
    let (handle, editor, id) = setup(&source, cx);
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &id, 0, 0, 8, window, cx);
        window.render_frame(cx);
        cx.set_reduce_motion(true);
    })
    .unwrap();
    let before = editor.read_with(cx, |editor, _| {
        (editor.document.encode_state(), editor.scroll.offset())
    });
    for trigger in ["selection-style", "selection-list"] {
        cx.update_window(handle, |_, window, cx| window.click(trigger, cx))
            .unwrap();
        finish(handle, cx);
        let baseline = cx
            .update_window(handle, |_, window, _| {
                window.find("popup-menu").bounds().size
            })
            .unwrap();
        let mut style = EditorStyle::default();
        style.popup_menu = div().min_w(px(300.)).max_h(px(80.)).style().clone();
        editor.update(cx, |editor, cx| editor.set_style(style, cx));
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            let menu = window.find("popup-menu").bounds();
            assert!(menu.size.width >= px(300.));
            assert!(menu.size.height <= px(80.));
            window.scroll(
                "selection-dropdown-menu",
                gpui_kit::ScrollDelta::Pixels(point(px(0.), px(-90.))),
                cx,
            );
            assert_eq!(editor.read(cx).scroll.offset(), before.1);
        })
        .unwrap();
        editor.update(cx, |editor, cx| {
            editor.set_style(EditorStyle::default(), cx)
        });
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            assert_eq!(window.find("popup-menu").bounds().size, baseline);
            window.press("escape", cx);
        })
        .unwrap();
        finish(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            assert!(window.try_find("popup-menu").is_none());
            assert!(window.find("selection-toolbar").visible());
            assert!(
                editor.read(cx).views[&id]
                    .editor
                    .focus_handle(cx)
                    .is_focused(window)
            );
            assert_eq!(editor.read(cx).document.encode_state(), before.0);
        })
        .unwrap();
    }
}

#[gpui_kit::test]
fn toolbar_buttons_resolve_one_local_hover_style_and_clear_it_at_runtime(cx: &mut TestAppContext) {
    let (first, editor, id) = setup("Selected words", cx);
    let (second, other, other_id) = setup("Selected words", cx);
    for (handle, editor, id) in [(first, &editor, &id), (second, &other, &other_id)] {
        cx.update_window(handle, |_, window, cx| {
            select(editor, id, 0, 0, 8, window, cx);
            window.render_frame(cx);
        })
        .unwrap();
    }
    let baseline = cx
        .update_window(first, |_, window, _| {
            window.find("selection-bold").bounds().size
        })
        .unwrap();
    let before = editor.read_with(cx, |editor, cx| {
        (
            editor.document.encode_state(),
            editor.views[&id].editor.read(cx).selection().clone(),
        )
    });
    let mut style = EditorStyle::default();
    style.controls = div().w(px(40.)).h(px(32.)).style().clone();
    style.controls_hover = div().w(px(64.)).style().clone();
    editor.update(cx, |editor, cx| editor.set_style(style, cx));
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert_eq!(window.find("selection-bold").bounds().size.width, px(40.));
        window.hover("selection-bold", cx);
        assert_eq!(window.find("selection-bold").bounds().size.width, px(64.));
        assert_eq!(window.find("selection-bold").bounds().size.height, px(32.));
        assert_eq!(editor.read(cx).document.encode_state(), before.0);
        assert_eq!(
            editor.read(cx).views[&id].editor.read(cx).selection(),
            &before.1
        );
    })
    .unwrap();
    cx.update_window(second, |_, window, _| {
        assert_eq!(window.find("selection-bold").bounds().size, baseline);
    })
    .unwrap();
    editor.update(cx, |editor, cx| {
        editor.set_style(EditorStyle::default(), cx)
    });
    finish(first, cx);
    cx.update_window(first, |_, window, cx| {
        assert_eq!(window.find("selection-bold").bounds().size, baseline);
        assert_eq!(editor.read(cx).document.encode_state(), before.0);
        assert_eq!(
            editor.read(cx).views[&id].editor.read(cx).selection(),
            &before.1
        );
    })
    .unwrap();
}
