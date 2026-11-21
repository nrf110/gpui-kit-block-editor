//! Presentation tests inspect the native frame and GPUI input's shaped layout.
use std::{cell::Cell, rc::Rc, sync::Arc};

use crate::{
    BlockEditor, BlockEditorEvent, BlockEditorState, BlockPosition, BlockSelection, EditorTextStyle,
};
use gpui::{
    AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _, Pixels, Render,
    StyleRefinement, Styled as _, TestAppContext, VisualTestContext, Window, div, px, size,
};

struct Root {
    editors: [Entity<BlockEditorState>; 2],
    styles: [Arc<EditorTextStyle>; 2],
    sizes: [f32; 2],
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(700.))
            .flex()
            .flex_col()
            .children((0..2).map(|ix| {
                div()
                    .w_full()
                    .font_family("Helvetica")
                    .text_size(px(self.sizes[ix]))
                    .child(
                        BlockEditor::new(&self.editors[ix]).presentation(self.styles[ix].clone()),
                    )
            }))
    }
}

fn open<'a>(
    cx: &'a mut TestAppContext,
    source: &str,
    code: bool,
) -> (
    Entity<Root>,
    [Entity<BlockEditorState>; 2],
    &'a mut VisualTestContext,
) {
    cx.update(crate::init_for_tests);
    let source = source.to_owned();
    let (root, cx) = cx.add_window_view(move |window, cx| {
        window.resize(size(px(900.), px(1800.)));
        let editors = std::array::from_fn(|_| {
            cx.new(|cx| {
                let state = BlockEditorState::new(window, cx).with_source(&source);
                if code {
                    state.with_code_editor()
                } else {
                    state
                }
            })
        });
        Root {
            editors,
            styles: std::array::from_fn(|_| Arc::default()),
            sizes: [16.; 2],
        }
    });
    let editors = root.read_with(cx, |root, _| root.editors.clone());
    draw(cx);
    (root, editors, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
}

fn apply(root: &Entity<Root>, style: EditorTextStyle, cx: &mut VisualTestContext) {
    root.update(cx, |root, cx| {
        root.styles[0] = Arc::new(style);
        cx.notify();
    });
    draw(cx);
}

fn metrics(
    editor: &Entity<BlockEditorState>,
    cx: &VisualTestContext,
) -> Vec<(Pixels, Pixels, Pixels)> {
    editor.read_with(cx, |editor, _| {
        editor
            .frame
            .borrow()
            .leaves
            .iter()
            .map(|leaf| {
                (
                    leaf.line_height(),
                    leaf.caret_height(),
                    leaf.caret_point(4).x - leaf.caret_point(0).x,
                )
            })
            .collect()
    })
}

fn refinement(size: f32, height: f32) -> StyleRefinement {
    let mut style = StyleRefinement::default();
    style.text.font_size = Some(px(size).into());
    style.text.line_height = Some(px(height).into());
    style
}

#[gpui::test]
fn body_typography_reflows_only_its_instance_without_editing(cx: &mut TestAppContext) {
    let (root, editors, cx) = open(cx, "iiii WWWW", false);
    let before = metrics(&editors[0], cx);
    let untouched = metrics(&editors[1], cx);
    let saved = editors[0].read_with(cx, |editor, _| {
        (
            editor.model.rich_document().unwrap(),
            editor.revision(),
            editor.selection().clone(),
        )
    });
    let changes = Rc::new(Cell::new(0));
    let count = changes.clone();
    let _subscription = cx.update(|_, cx| {
        cx.subscribe(&editors[0], move |_, event, _| {
            if matches!(event, BlockEditorEvent::Changed { .. }) {
                count.set(count.get() + 1);
            }
        })
    });
    let mut style = EditorTextStyle::default();
    style.body = refinement(32., 48.);
    style.body.text.font_family = Some("Courier".into());
    apply(&root, style, cx);
    let after = metrics(&editors[0], cx);
    assert_eq!(after[0].0, px(48.));
    assert!(after[0].1 > before[0].1 * 1.5);
    assert!(
        after[0].2 > before[0].2 * 1.5,
        "font family and size reach actual glyphs"
    );
    assert_eq!(metrics(&editors[1], cx), untouched);
    assert_eq!(
        editors[0].read_with(cx, |editor, _| {
            (
                editor.model.rich_document().unwrap(),
                editor.revision(),
                editor.selection().clone(),
            )
        }),
        saved
    );
    assert_eq!(changes.get(), 0);

    apply(&root, EditorTextStyle::default(), cx);
    assert_eq!(metrics(&editors[0], cx), before);
    root.update(cx, |root, cx| {
        root.sizes[0] = 24.;
        cx.notify();
    });
    draw(cx);
    let inherited = metrics(&editors[0], cx);
    assert!(
        inherited[0].1 > before[0].1 * 1.4,
        "unset body keeps Styled inheritance"
    );
    assert_eq!(metrics(&editors[1], cx), untouched);
}

#[gpui::test]
fn partial_heading_overrides_keep_and_refresh_app_defaults(cx: &mut TestAppContext) {
    use gpui_kit::base::text::{TextViewDefaults, TextViewStyle};
    let (root, editors, cx) = open(cx, "# MMMM\n\nbody", false);
    cx.update(|_, cx| {
        TextViewDefaults::new()
            .with_style(TextViewStyle::default().with_heading(|_| refinement(30., 42.)))
            .install(cx);
    });
    let mut style = EditorTextStyle::default();
    style.headings[0].text.color = Some(gpui::red());
    apply(&root, style.clone(), cx);
    let before = metrics(&editors[0], cx);
    assert_eq!(before[0].0, px(42.));
    assert_eq!(before, metrics(&editors[1], cx));
    cx.update(|_, cx| {
        TextViewDefaults::new()
            .with_style(TextViewStyle::default().with_heading(|_| refinement(45., 60.)))
            .install(cx);
    });
    root.update(cx, |_, cx| cx.notify());
    draw(cx);
    let after = metrics(&editors[0], cx);
    assert_eq!(after[0].0, px(60.));
    assert!(after[0].2 > before[0].2 * 1.4);
    assert_eq!(after[1], before[1]);
    style.headings[0] = refinement(20., 28.);
    apply(&root, style, cx);
    assert_eq!(metrics(&editors[0], cx)[0].0, px(28.));
    assert_eq!(metrics(&editors[1], cx)[0].0, px(60.));
}

#[gpui::test]
fn quote_code_and_table_refinements_reach_native_leaf_geometry(cx: &mut TestAppContext) {
    let (root, editors, cx) = open(
        cx,
        "> MMMM\n\n```plain\nMMMM\n```\n\n| Head |\n| --- |\n| MMMM |",
        false,
    );
    let before = metrics(&editors[0], cx);
    assert_eq!(before.len(), 4);
    let mut style = EditorTextStyle::default();
    style.quote = refinement(26., 39.);
    style.code = refinement(24., 38.);
    style.table = refinement(22., 34.);
    style.table_cell.padding.left = Some(px(19.).into());
    style.table_cell.padding.right = Some(px(31.).into());
    style.table_cell.padding.top = Some(px(13.).into());
    style.table_cell.padding.bottom = Some(px(17.).into());
    style.table_head.text.font_size = Some(px(28.).into());
    apply(&root, style, cx);
    let after = metrics(&editors[0], cx);
    assert_eq!(after[0].0, px(39.));
    assert_eq!(after[1].0, px(38.));
    assert_eq!(after[2].0, px(34.));
    assert_eq!(after[3].0, px(34.));
    assert!(after[0].2 > before[0].2);
    assert!(after[1].2 > before[1].2);
    assert!(
        after[2].1 > after[3].1,
        "table header typography refines the table's base typography"
    );
    editors[0].read_with(cx, |editor, _| {
        let frame = editor.frame.borrow();
        for leaf in &frame.leaves[2..] {
            let cell = frame.cell_boxes[&leaf.info.block];
            assert!((leaf.bounds().top() - cell.top() - px(13.)).abs() < px(1.));
            assert!((cell.bottom() - leaf.bounds().bottom() - px(17.)).abs() <= px(1.));
        }
    });
    assert_eq!(metrics(&editors[1], cx), before);
}

fn code_metrics(
    editor: &Entity<BlockEditorState>,
    cx: &VisualTestContext,
) -> (Pixels, Bounds<Pixels>, Pixels) {
    editor.read_with(cx, |editor, cx| {
        let input = editor.code_editor_state().unwrap();
        let input = input.read(cx);
        (
            input.line_height().unwrap(),
            input.cursor_layout().unwrap().0,
            input.range_to_bounds(&(0..4)).unwrap().size.width,
        )
    })
}

#[gpui::test]
fn source_editor_styles_shape_real_input_and_keep_identity_selection_and_undo(
    cx: &mut TestAppContext,
) {
    let (root, editors, cx) = open(cx, "```plain\niiii WWWW\nsecond\n```", true);
    let input = editors[0].read_with(cx, |editor, _| editor.code_editor_state().unwrap());
    // Focus alone does not activate a headless window. GPUI deliberately
    // suppresses caret painting in inactive windows, even with valid geometry.
    cx.update(|window, _| window.activate_window());
    cx.run_until_parked();
    cx.update(|window, cx| {
        use gpui::Focusable as _;
        window.focus(&input.focus_handle(cx), cx);
    });
    cx.simulate_input("!");
    draw(cx);
    let before = code_metrics(&editors[0], cx);
    let untouched = code_metrics(&editors[1], cx);
    let saved = editors[0].read_with(cx, |editor, cx| {
        (
            editor.model.rich_document().unwrap(),
            editor.revision(),
            editor.selection().clone(),
            input.read(cx).selected_range(),
        )
    });
    apply(
        &root,
        EditorTextStyle {
            body: StyleRefinement::default()
                .font_family("Georgia")
                .text_size(px(24.)),
            ..Default::default()
        },
        cx,
    );
    let prose_only = code_metrics(&editors[0], cx);
    assert_eq!(prose_only.0, before.0);
    assert_eq!(
        prose_only.2, before.2,
        "body typography must not replace the code font"
    );
    let caret = gpui::hsla(0.27, 0.91, 0.47, 1.);
    let background = gpui::hsla(0.73, 0.82, 0.19, 1.);
    let mut style = EditorTextStyle::default();
    style.body = refinement(18., 25.);
    style.code = refinement(32., 46.);
    style.code.text.font_family = Some("Courier".into());
    style.code.background = Some(background.into());
    style.caret = Some(caret);
    style.selection = Some(gpui::hsla(0.13, 0.72, 0.51, 0.6));
    apply(&root, style, cx);
    let after = code_metrics(&editors[0], cx);
    assert_eq!(after.0, px(46.));
    assert!(after.1.size.height > before.1.size.height * 1.3);
    assert!(after.2 > before.2 * 1.3);
    assert_eq!(code_metrics(&editors[1], cx).0, untouched.0);
    assert_eq!(code_metrics(&editors[1], cx).2, untouched.2);
    assert_eq!(
        editors[0].read_with(cx, |editor, cx| {
            assert_eq!(editor.code_editor_state().unwrap(), input);
            (
                editor.model.rich_document().unwrap(),
                editor.revision(),
                editor.selection().clone(),
                input.read(cx).selected_range(),
            )
        }),
        saved
    );
    cx.update(|window, cx| {
        use gpui::Focusable as _;
        assert!(
            window.is_window_active(),
            "caret paint requires an active window"
        );
        assert!(input.focus_handle(cx).is_focused(window));
        assert!(input.read(cx).selected_range().is_empty());
        let quads = window.painted_quads();
        assert!(
            quads
                .iter()
                .any(|quad| quad.background.as_solid() == Some(caret)),
            "the real input paints the local caret color"
        );
        assert!(
            quads
                .iter()
                .any(|quad| quad.background.as_solid() == Some(background)),
            "the real input paints the local code surface"
        );
    });
    apply(&root, EditorTextStyle::default(), cx);
    let restored = code_metrics(&editors[0], cx);
    assert_eq!(restored.0, before.0);
    assert_eq!(restored.2, before.2);
    editors[0].update(cx, |editor, cx| editor.undo(cx));
    draw(cx);
    assert_eq!(
        editors[0].read_with(cx, |editor, _| editor.source()),
        "```plain\niiii WWWW\nsecond\n```"
    );
}
