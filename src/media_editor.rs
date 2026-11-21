use super::*;
use crate::media::MediaKind;
use anyhow::ensure;
use std::path::PathBuf;

struct MediaSlash {
    anchor: RelativePosition,
    head: RelativePosition,
    expected: String,
}

enum MediaTarget {
    Command {
        section: SectionId,
        slash: Option<MediaSlash>,
    },
    Drop(DropTarget),
}

fn empty_text(content: &RichDocument) -> bool {
    !content.blocks.is_empty()
        && content.blocks.iter().all(|node| {
            matches!(node.kind, RichKind::Paragraph | RichKind::Heading)
                && node.children.is_empty()
                && node
                    .runs
                    .iter()
                    .all(|run| run.text.is_empty() && run.atom.is_none())
        })
}

impl BlockEditor {
    pub(super) fn sync_media_view(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.views.get_mut(id) else {
            return;
        };
        let attachment = match MediaAttachment::from_content(&view.content) {
            Ok(attachment) => attachment,
            Err(error) => {
                self.error = Some(error.to_string());
                return;
            }
        };
        if view.media.as_ref().map(|(attachment, _)| attachment) == attachment.as_ref() {
            return;
        }
        view.media = attachment.map(|attachment| {
            let store = self.extensions.media_store.clone();
            let style = self.style.clone();
            let card = cx.new(|cx| {
                let mut card = MediaView::new(attachment.clone(), store, window, cx);
                card.set_style(style, cx);
                card
            });
            (attachment, card)
        });
    }

    pub(super) fn prompt_media(
        &mut self,
        id: &str,
        kind: MediaKind,
        range: Option<CapturedRange>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let view = self.views.get(id).context("media target was deleted")?;
        let state = view.editor.read(cx);
        ensure!(
            !state.is_readonly(),
            "cannot insert media into a read-only block"
        );
        let slash = if let Some(range) = range {
            ensure!(
                state.model().is_capture_current(&range),
                "media command is no longer current"
            );
            let caret = state
                .model()
                .caret()
                .context("media command has no text caret")?;
            let prefix = state
                .caret_prefix()
                .context("media command has no text prefix")?;
            let start = prefix.rfind('/').context("media command has no slash")?;
            let map = view
                .rich_map
                .as_ref()
                .context("media command has no semantic mapping")?;
            let anchor = map
                .point(markdown_editor::BlockPosition::new(caret.block, start))
                .context("media command start is not mapped")?;
            let head = map
                .point(caret)
                .context("media command end is not mapped")?;
            Some(MediaSlash {
                anchor: self.document.relative_position(
                    id,
                    &anchor.node,
                    anchor.offset,
                    Assoc::After,
                )?,
                head: self.document.relative_position(
                    id,
                    &head.node,
                    head.offset,
                    Assoc::Before,
                )?,
                expected: prefix[start..].to_owned(),
            })
        } else {
            None
        };
        let target = MediaTarget::Command {
            section: id.to_owned(),
            slash,
        };
        let selection = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(format!("Choose {} files", kind_label(kind)).into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = selection.await;
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(Ok(Some(paths))) => {
                    this.start_media_import(paths, Some(kind), target, window, cx)
                }
                Ok(Ok(None)) => {}
                Ok(Err(error)) => {
                    this.error = Some(error.to_string());
                    cx.notify();
                }
                Err(error) => {
                    this.error = Some(format!("File picker: {error}"));
                    cx.notify();
                }
            });
        })
        .detach();
        Ok(())
    }

    pub(super) fn import_dropped_media(
        &mut self,
        paths: Vec<PathBuf>,
        target: DropTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.start_media_import(paths, None, MediaTarget::Drop(target), window, cx);
    }

    fn start_media_import(
        &mut self,
        paths: Vec<PathBuf>,
        kind: Option<MediaKind>,
        target: MediaTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if paths.is_empty() {
            return;
        }
        self.media_imports += 1;
        self.error = None;
        let store = self.extensions.media_store.clone();
        let imported = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|path| store.import_file(path, kind))
                .collect::<anyhow::Result<Vec<_>>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let imported = imported.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.media_imports -= 1;
                let result = imported.and_then(|attachments| {
                    this.finish_media_import(attachments, target, window, cx)
                });
                if let Err(error) = result {
                    this.error = Some(format!("Media import: {error:#}"));
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn finish_media_import(
        &mut self,
        attachments: Vec<MediaAttachment>,
        target: MediaTarget,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> anyhow::Result<()> {
        self.flush_local_edits(window, cx)?;
        self.capture_selections(cx)?;
        let contents = attachments
            .iter()
            .map(MediaAttachment::to_content)
            .collect::<anyhow::Result<Vec<_>>>()?;
        ensure!(!contents.is_empty(), "no media files were imported");
        let (placement, command) = match target {
            MediaTarget::Drop(target) => (target, None),
            MediaTarget::Command { section, slash } => {
                let view = self
                    .views
                    .get(&section)
                    .context("media target was deleted")?;
                let siblings = self.document.children(view.parent.as_deref());
                let index = siblings
                    .iter()
                    .position(|candidate| candidate.id == section)
                    .context("media target is no longer in its group")?;
                let placement = DropTarget {
                    parent: view.parent.clone(),
                    before: siblings
                        .get(index + 1)
                        .map(|candidate| candidate.id.clone()),
                };
                let cleanup = slash
                    .map(|slash| self.resolve_media_slash(&section, slash, cx))
                    .transpose()?;
                (placement, Some((section, cleanup)))
            }
        };
        let siblings = self.document.children(placement.parent.as_deref());
        ensure!(
            placement
                .before
                .as_ref()
                .is_none_or(|id| siblings.iter().any(|section| section.id == *id)),
            "media drop target was moved or deleted"
        );
        let in_list = if let Some(parent) = &placement.parent {
            let sections = self.document.sections();
            let parent = sections
                .iter()
                .find(|section| section.id == *parent)
                .context("media parent was deleted")?;
            matches!(parent.kind, SectionKind::List { .. })
        } else {
            false
        };
        let role = if in_list {
            SectionKind::ListItem { checked: None }
        } else {
            SectionKind::Text
        };
        let vector = self.document.state_vector();
        self.document.begin_history_group();
        let result = (|| -> anyhow::Result<()> {
            let mut replacement = None;
            if let Some((section, cleanup)) = command {
                if let Some(selection) = cleanup {
                    let editor = self.views[&section].editor.clone();
                    editor.update(cx, |state, cx| -> anyhow::Result<()> {
                        state
                            .set_selection(selection, cx)
                            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                        let mut capture = state
                            .capture_selection()
                            .map_err(|error| anyhow::anyhow!("{error:?}"))?;
                        state
                            .replace_captured(&mut capture, "", cx)
                            .map_err(|error| anyhow::anyhow!("{error:?}"))
                    })?;
                    self.sync_native_change(
                        &section,
                        Some(EditOrigin::Command("insert_media")),
                        window,
                        cx,
                    )?;
                }
                let view = &self.views[&section];
                if view.kind.default_collapsed().is_none() && empty_text(&view.content) {
                    replacement = Some(section);
                }
            }
            let replaced = replacement.is_some();
            for content in contents {
                if let Some(id) = replacement.take() {
                    self.document.set_content(&id, &content)?;
                } else {
                    self.document.insert_block(
                        placement.parent.as_deref(),
                        placement.before.as_deref(),
                        &BlockSpec {
                            kind: role.clone(),
                            content,
                            children: Vec::new(),
                        },
                    )?;
                }
            }
            // Replacing the sole typing block must leave an editable continuation.
            let continuation = if replaced {
                Some(self.document.insert_block(
                    placement.parent.as_deref(),
                    placement.before.as_deref(),
                    &BlockSpec {
                        kind: role,
                        content: RichDocument::paragraph(""),
                        children: Vec::new(),
                    },
                )?)
            } else {
                None
            };
            self.completion = None;
            self.block_selection.clear();
            self.block_anchor = None;
            self.reconcile(window, cx);
            if let Some(id) = continuation {
                self.focus_block_at(&id, None, window, cx)?;
            }
            self.publish(&vector, cx);
            Ok(())
        })();
        self.document.end_history_group();
        result
    }

    fn resolve_media_slash(
        &self,
        id: &str,
        slash: MediaSlash,
        cx: &App,
    ) -> anyhow::Result<BlockSelection> {
        let (section, node, start) = self
            .document
            .resolve_position(&slash.anchor)
            .context("media command text was deleted")?;
        let (end_section, end_node, end) = self
            .document
            .resolve_position(&slash.head)
            .context("media command text was deleted")?;
        ensure!(
            section == id && end_section == id && node == end_node && start <= end,
            "media command target changed"
        );
        let view = self.views.get(id).context("media target was deleted")?;
        fn find<'a>(nodes: &'a [RichNode], id: &str) -> Option<&'a RichNode> {
            nodes.iter().find_map(|node| {
                if node.id == id {
                    Some(node)
                } else {
                    find(&node.children, id)
                }
            })
        }
        let text = find(&view.content.blocks, &node)
            .context("media command node was deleted")?
            .text();
        ensure!(
            text.get(start..end) == Some(slash.expected.as_str()),
            "media command text changed while choosing a file"
        );
        let map = view
            .rich_map
            .as_ref()
            .context("media command has no semantic mapping")?;
        let point = |offset| RichPoint {
            node: node.clone(),
            offset,
            after_atom: false,
        };
        let anchor = map
            .position(&point(start), Assoc::After)
            .context("media command start was deleted")?;
        let head = map
            .position(&point(end), Assoc::Before)
            .context("media command end was deleted")?;
        ensure!(
            view.editor
                .read(cx)
                .model()
                .document()
                .block(anchor.block)
                .is_some(),
            "media command is no longer editable"
        );
        Ok(BlockSelection::Text { anchor, head })
    }
}

fn kind_label(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Audio => "audio",
        MediaKind::File => "attachment",
    }
}
