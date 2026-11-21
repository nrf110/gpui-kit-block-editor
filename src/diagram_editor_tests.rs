use super::BlockEditor;
use super::collaboration_tests::{replica, select, setup};
use gpui_kit::test::{TestEventExt, TestWindowExt};
use gpui_kit::{
    AnyWindowHandle, AppContext, Bounds, Entity, InputEvent, MouseButton, Pixels, Point,
    ScrollDelta, ScrollEvent, SharedString, TestAppContext, point, px,
};

const HORIZONTAL: &str = "```mermaid\nflowchart LR\n A --> B --> C --> D --> E --> F\n```";
const VERTICAL: &str = "```mermaid\nflowchart TD\n A --> B --> C --> D --> E --> F\n```";

// Match native image tests: drain the real renderer, paint the new viewport, then
// drain its raster request. Reduced motion removes dialog entrance geometry from
// these camera assertions, as in GPUI Kit's overlay integration tests.
fn finish(handle: AnyWindowHandle, cx: &mut TestAppContext) {
    for _ in 0..3 {
        cx.run_until_parked();
        cx.update_window(handle, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
}

fn open(handle: AnyWindowHandle, section: &str, cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_reduce_motion(true));
    cx.update_window(handle, |_, window, _| window.activate_window())
        .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        window.hover(SharedString::from(section.to_owned()), cx);
        assert!(window.find(("expand-preview", 0usize)).visible());
        window.click(("expand-preview", 0usize), cx);
    })
    .unwrap();
    finish(handle, cx);
}

fn image(handle: AnyWindowHandle, cx: &mut TestAppContext) -> Bounds<Pixels> {
    cx.update_window(handle, |_, window, _| {
        let image = window.find("diagram-image");
        assert!(image.visible());
        image.bounds()
    })
    .unwrap()
}

fn close_to(actual: Point<Pixels>, expected: Point<Pixels>) {
    assert!(
        (actual.x - expected.x).abs() <= px(2.),
        "{actual:?} != {expected:?}"
    );
    assert!(
        (actual.y - expected.y).abs() <= px(2.),
        "{actual:?} != {expected:?}"
    );
}

fn same_camera(actual: Bounds<Pixels>, expected: Bounds<Pixels>) {
    close_to(actual.origin, expected.origin);
    assert!((actual.size.width - expected.size.width).abs() <= px(2.));
    assert!((actual.size.height - expected.size.height).abs() <= px(2.));
}

fn apply_remote(
    handle: AnyWindowHandle,
    editor: &Entity<BlockEditor>,
    update: &[u8],
    cx: &mut TestAppContext,
) {
    cx.update_window(handle, |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.apply_update(update, window, cx).unwrap();
        });
    })
    .unwrap();
    finish(handle, cx);
}

#[gpui_kit::test]
fn hover_expand_opens_a_window_sized_diagram_and_camera_controls_do_not_edit(
    cx: &mut TestAppContext,
) {
    let (handle, editor, section) = setup(HORIZONTAL, cx);
    let original = editor.read_with(cx, |editor, _| editor.document.sections());
    let sentinel = cx
        .update_window(handle, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor
                    .insert_section(None, "Undo sentinel", window, cx)
                    .unwrap()
            })
        })
        .unwrap();
    finish(handle, cx);
    let before = editor.read_with(cx, |editor, _| editor.document.encode_state());
    cx.update_window(handle, |_, window, cx| {
        window.hover("add-section", cx);
        assert!(
            !window
                .try_find(("expand-preview", 0usize))
                .is_some_and(|button| button.visible())
        );
        window.hover(SharedString::from(section.clone()), cx);
        let expand = window.find(("expand-preview", 0usize)).bounds();
        let toggle = window.find(("toggle-preview", 0usize)).bounds();
        assert!(expand.right() <= toggle.left());
    })
    .unwrap();
    open(handle, &section, cx);
    let fitted = image(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        let surface = window.within("dialog").find("surface").bounds();
        let viewer = window.find("diagram-viewer").bounds();
        let viewport = window.find("diagram-viewport").bounds();
        let size = window.viewport_size();
        assert!(surface.left() >= px(0.) && surface.top() >= px(0.));
        assert!(surface.right() <= size.width && surface.bottom() <= size.height);
        assert!(surface.size.width >= size.width - px(64.));
        assert!(surface.size.height >= size.height - px(64.));
        assert!(viewer.size.width >= surface.size.width - px(4.));
        assert!(viewport.size.height >= surface.size.height - px(96.));
        assert!(fitted.left() >= viewport.left() - px(2.));
        assert!(fitted.right() <= viewport.right() + px(2.));
        assert!(fitted.top() >= viewport.top() - px(2.));
        assert!(fitted.bottom() <= viewport.bottom() + px(2.));
        window.click("diagram-zoom-in", cx);
    })
    .unwrap();
    finish(handle, cx);
    let enlarged = image(handle, cx);
    assert!(enlarged.size.width > fitted.size.width);
    assert!(enlarged.size.height > fitted.size.height);
    close_to(enlarged.center(), fitted.center());
    cx.update_window(handle, |_, window, cx| window.click("diagram-zoom-out", cx))
        .unwrap();
    finish(handle, cx);
    let reduced = image(handle, cx);
    assert!(reduced.size.width < enlarged.size.width);
    close_to(reduced.center(), enlarged.center());
    cx.update_window(handle, |_, window, cx| {
        window.click("diagram-zoom-in", cx);
        let start = window.find("diagram-viewport").bounds().center();
        window.drag(start, start + point(px(45.), px(30.)), cx);
        window.click("diagram-fit", cx);
    })
    .unwrap();
    finish(handle, cx);
    same_camera(image(handle, cx), fitted);
    cx.update_window(handle, |_, window, cx| window.press("escape", cx))
        .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("diagram-viewer").is_none());
        assert!(window.try_find("dialog").is_none());
    })
    .unwrap();
    // The ordinary close icon must remain usable beside the viewer's controls.
    open(handle, &section, cx);
    cx.update_window(handle, |_, window, cx| {
        window.within("dialog").click("close", cx);
    })
    .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("diagram-viewer").is_none());
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.encode_state(), before);
        assert!(editor.document.can_undo());
        assert!(!editor.document.can_redo());
    });
    cx.update_window(handle, |_, window, cx| {
        select(&editor, &sentinel, 0, 0, 0, window, cx);
        window.press("cmd-z", cx);
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.document.sections(), original);
        assert!(!editor.document.can_undo());
        assert!(editor.document.can_redo());
    });
}

#[gpui_kit::test]
fn wheel_zooms_at_the_pointer_and_pan_ends_on_inside_or_outside_release(cx: &mut TestAppContext) {
    // A genuinely scrollable document makes escaped wheel events observable.
    let source = format!("{HORIZONTAL}{}", "\n\nDocument scroll sentinel".repeat(60));
    let (handle, editor, section) = setup(&source, cx);
    finish(handle, cx);
    cx.update_window(handle, |_, window, cx| {
        let position = window
            .find(SharedString::from(section.clone()))
            .bounds()
            .center();
        window.pointer_move(position, None, cx);
        window.dispatch_event(
            ScrollEvent {
                position,
                delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                ..Default::default()
            }
            .to_platform_input(),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    assert!(editor.read_with(cx, |editor, _| editor.scroll.offset().y) < px(0.));
    open(handle, &section, cx);
    let before = editor.read_with(cx, |editor, _| editor.document.encode_state());
    let page = editor.read_with(cx, |editor, _| editor.scroll.offset());
    let fitted = image(handle, cx);
    let pointer = cx
        .update_window(handle, |_, window, cx| {
            let viewport = window.find("diagram-viewport").bounds();
            let pointer = viewport.center() + point(px(73.), px(29.));
            window.pointer_move(pointer, None, cx);
            window.dispatch_event(
                ScrollEvent {
                    position: pointer,
                    delta: ScrollDelta::Pixels(point(px(0.), px(90.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            pointer
        })
        .unwrap();
    finish(handle, cx);
    let zoomed = image(handle, cx);
    assert!((zoomed.size.width - fitted.size.width).abs() > px(2.));
    let ratio = zoomed.size.width / fitted.size.width;
    let expected_origin = pointer - (pointer - fitted.origin) * ratio;
    close_to(zoomed.origin, expected_origin);
    let ratio_y = zoomed.size.height / fitted.size.height;
    assert!((ratio - ratio_y).abs() < 0.01);
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.scroll.offset()),
        page
    );

    for outside in [false, true] {
        let before_pan = image(handle, cx);
        cx.update_window(handle, |_, window, cx| {
            let viewport = window.find("diagram-viewport").bounds();
            let start = viewport.center();
            let delta = point(px(47.), px(-31.));
            window.pointer_down(start, MouseButton::Left, cx);
            window.pointer_move(start + delta, Some(MouseButton::Left), cx);
            close_to(
                window.find("diagram-image").bounds().origin,
                before_pan.origin + delta,
            );
            let release = if outside {
                // Header is inside the dialog but outside the clipped canvas.
                window.find("diagram-fit").bounds().center()
            } else {
                start + delta
            };
            if outside {
                window.pointer_move(release, Some(MouseButton::Left), cx);
            }
            window.pointer_up(release, MouseButton::Left, cx);
            let released = window.find("diagram-image").bounds();
            window.pointer_move(viewport.center() + point(px(-55.), px(19.)), None, cx);
            same_camera(window.find("diagram-image").bounds(), released);
            assert!(window.find("diagram-viewer").visible());
        })
        .unwrap();
    }
    cx.update_window(handle, |_, window, cx| window.click("diagram-fit", cx))
        .unwrap();
    finish(handle, cx);
    same_camera(image(handle, cx), fitted);
    editor.read_with(cx, |editor, _| {
        assert_eq!(editor.scroll.offset(), page);
        assert_eq!(editor.document.encode_state(), before);
        assert!(!editor.document.can_undo());
        assert!(!editor.document.can_redo());
    });
}

#[gpui_kit::test]
fn remote_mermaid_refresh_changes_the_expanded_image_without_resetting_camera(
    cx: &mut TestAppContext,
) {
    let (handle, editor, section) = setup(HORIZONTAL, cx);
    open(handle, &section, cx);
    let initial = image(handle, cx);
    assert!(initial.size.width > initial.size.height);
    cx.update_window(handle, |_, window, cx| {
        window.click("diagram-zoom-in", cx);
        let start = window.find("diagram-viewport").bounds().center();
        window.drag(start, start + point(px(37.), px(23.)), cx);
    })
    .unwrap();
    finish(handle, cx);
    let prior = image(handle, cx);
    let mut remote = replica(&editor, cx);
    let vector = remote.state_vector();
    remote.set_markdown(&section, VERTICAL).unwrap();
    let update = remote.encode_update(&vector).unwrap();
    apply_remote(handle, &editor, &update, cx);
    let refreshed = image(handle, cx);
    assert!(refreshed.size.height > refreshed.size.width);
    close_to(refreshed.center(), prior.center());
    cx.update_window(handle, |_, window, cx| window.click("diagram-fit", cx))
        .unwrap();
    finish(handle, cx);
    let fitted = image(handle, cx);
    assert!(refreshed.size.height > fitted.size.height);
    close_to(
        refreshed.center() - fitted.center(),
        point(px(37.), px(23.)),
    );
    assert_eq!(
        editor.read_with(cx, |editor, _| editor.document.markdown()),
        remote.markdown()
    );
    editor.read_with(cx, |editor, _| {
        assert!(!editor.document.can_undo());
        assert!(!editor.document.can_redo());
    });
}

#[gpui_kit::test]
fn remote_invalid_non_diagram_and_deleted_sources_replace_the_image_with_an_error(
    cx: &mut TestAppContext,
) {
    let (handle, editor, section) = setup(HORIZONTAL, cx);
    open(handle, &section, cx);
    let mut remote = replica(&editor, cx);
    for replacement in [
        Some("```mermaid\nnot a diagram\n```"),
        Some("Ordinary text now"),
        None,
    ] {
        let vector = remote.state_vector();
        if let Some(source) = replacement {
            remote.set_markdown(&section, source).unwrap();
        } else {
            remote.remove_section(&section).unwrap();
        }
        let update = remote.encode_update(&vector).unwrap();
        apply_remote(handle, &editor, &update, cx);
        cx.update_window(handle, |_, window, _| {
            assert!(window.find("diagram-viewer").visible());
            assert!(
                !window
                    .try_find("diagram-image")
                    .is_some_and(|image| image.visible()),
                "an unavailable source must not leave its previous diagram on screen"
            );
            let error = window.find("diagram-error");
            assert!(error.visible());
            assert!(
                error
                    .label()
                    .or(error.value())
                    .is_some_and(|text| !text.trim().is_empty())
            );
        })
        .unwrap();
        assert_eq!(
            editor.read_with(cx, |editor, _| editor.document.markdown()),
            remote.markdown()
        );
        if replacement.is_some() {
            // Recovery must work in the same open dialog, not only after reopening.
            let vector = remote.state_vector();
            remote.set_markdown(&section, HORIZONTAL).unwrap();
            let update = remote.encode_update(&vector).unwrap();
            apply_remote(handle, &editor, &update, cx);
            assert!(image(handle, cx).size.width > px(0.));
            cx.update_window(handle, |_, window, _| {
                assert!(
                    !window
                        .try_find("diagram-error")
                        .is_some_and(|error| error.visible())
                );
            })
            .unwrap();
        }
    }
    cx.update_window(handle, |_, window, cx| window.press("escape", cx))
        .unwrap();
    finish(handle, cx);
    cx.update_window(handle, |_, window, _| {
        assert!(window.try_find("dialog").is_none());
    })
    .unwrap();
    editor.read_with(cx, |editor, _| {
        assert!(!editor.document.can_undo());
        assert!(!editor.document.can_redo());
    });
}
