use std::{
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context as _, bail};
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::*;

use crate::media::{MediaAttachment, MediaKind, MediaStore};

enum LocalAsset {
    Loading,
    Ready(Arc<Path>),
    Missing(String),
}

/// A local-only attachment surface. Neither mounting nor rendering opens files in other apps.
pub(super) struct MediaView {
    attachment: Arc<MediaAttachment>,
    store: Arc<dyn MediaStore>,
    local: LocalAsset,
    image_cache: Entity<RetainAllImageCache>,
    open_allowed: bool,
    name_label: SharedString,
    detail_label: SharedString,
    error: Option<String>,
    busy: bool,
    resolve_task: Option<Task<()>>,
    action_task: Option<Task<()>>,
}

impl MediaView {
    pub(super) fn new(
        attachment: MediaAttachment,
        store: Arc<dyn MediaStore>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            open_allowed: permits_system_open(&attachment),
            name_label: attachment.file_name.clone().into(),
            detail_label: format!(
                "{} · {}",
                match attachment.kind {
                    MediaKind::Image => "Image",
                    MediaKind::Video => "Video",
                    MediaKind::Audio => "Audio",
                    MediaKind::File => "File",
                },
                byte_size(attachment.byte_len)
            )
            .into(),
            attachment: Arc::new(attachment),
            store,
            local: LocalAsset::Loading,
            image_cache: RetainAllImageCache::new(cx),
            error: None,
            busy: false,
            resolve_task: None,
            action_task: None,
        };
        view.resolve(window, cx);
        view
    }

    fn replace_path(&mut self, path: Arc<Path>, window: &mut Window, cx: &mut Context<Self>) {
        let unchanged = matches!(&self.local, LocalAsset::Ready(current) if current == &path);
        if !unchanged {
            self.image_cache
                .update(cx, |cache, cx| cache.clear(window, cx));
        }
        self.local = LocalAsset::Ready(path);
    }

    fn resolve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let store = Arc::clone(&self.store);
        let attachment = self.attachment.clone();
        // A retry must also discard failed decodes and same-path files installed by the host.
        self.image_cache
            .update(cx, |cache, cx| cache.clear(window, cx));
        self.local = LocalAsset::Loading;
        self.error = None;
        let work = cx.background_executor().spawn(async move {
            resolve_local(store.as_ref(), &attachment).map_err(|error| format!("{error:#}"))
        });
        self.resolve_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(path) => this.replace_path(path.into(), window, cx),
                    Err(error) => this.local = LocalAsset::Missing(error),
                }
                this.resolve_task = None;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !self.open_allowed || !matches!(self.local, LocalAsset::Ready(_)) {
            return;
        }
        self.busy = true;
        self.error = None;
        let store = Arc::clone(&self.store);
        let attachment = self.attachment.clone();
        let work = cx.background_executor().spawn(async move {
            // Resolve again: a previously displayed asset may have disappeared or been replaced.
            let path =
                resolve_local(store.as_ref(), &attachment).map_err(|error| format!("{error:#}"))?;
            let safety =
                verify_system_open(&attachment, &path).map_err(|error| format!("{error:#}"));
            Ok::<_, String>((path, safety))
        });
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                this.action_task = None;
                match result {
                    Ok((path, safety)) => {
                        let path: Arc<Path> = path.into();
                        this.replace_path(Arc::clone(&path), window, cx);
                        match safety {
                            Ok(()) => cx.open_with_system(&path),
                            Err(error) => this.error = Some(error),
                        }
                    }
                    Err(error) => this.local = LocalAsset::Missing(error),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || !matches!(self.local, LocalAsset::Ready(_)) {
            return;
        }
        // No filesystem access here. The platform chooses its default location for an empty path.
        let directory = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_default();
        let prompt = cx.prompt_for_new_path(&directory, Some(&self.attachment.file_name));
        self.busy = true;
        self.error = None;
        let store = Arc::clone(&self.store);
        let attachment = self.attachment.clone();
        self.action_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = match prompt.await {
                Ok(Ok(Some(destination))) => {
                    // A dismissed/dropped view must not start a copy after its dialog completes.
                    if this.upgrade().is_none() {
                        return;
                    }
                    cx.background_executor()
                        .spawn(async move {
                            store
                                .export_file(&attachment, &destination)
                                .map_err(|error| format!("{error:#}"))
                        })
                        .await
                }
                Ok(Ok(None)) => Ok(()),
                Ok(Err(error)) => Err(format!("{error:#}")),
                Err(error) => Err(format!("Save dialog closed: {error}")),
            };
            let _ = this.update_in(cx, |this, _, cx| {
                this.busy = false;
                this.action_task = None;
                this.error = result.err();
                cx.notify();
            });
        }));
        cx.notify();
    }
}

impl Render for MediaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = match self.attachment.kind {
            MediaKind::Image => IconName::Frame,
            MediaKind::Video => IconName::Play,
            MediaKind::Audio => IconName::Mic,
            MediaKind::File => IconName::File,
        };
        let ready = matches!(self.local, LocalAsset::Ready(_));
        let loading = matches!(self.local, LocalAsset::Loading);
        let muted = cx.theme().muted_foreground;
        let danger = cx.theme().danger;
        let mut card = div()
            .id("media-attachment")
            .w_full()
            .min_w_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .rounded_md()
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background);
        if self.attachment.kind == MediaKind::Image
            && let LocalAsset::Ready(path) = &self.local
        {
            let decoded = self.image_cache.update(cx, |cache, cx| {
                cache.load(&Resource::Path(path.clone()), window, cx)
            });
            card = match decoded {
                Some(Ok(image)) => {
                    let size = image.size(0);
                    // GPUI gives a percentage-width image its intrinsic pixel
                    // height. Let a layout box own the responsive aspect ratio
                    // instead, with the decoded image contained inside it.
                    card.child(
                        div()
                            .relative()
                            .w_full()
                            .max_h(px(420.))
                            .aspect_ratio(size.width.0 as f32 / size.height.0 as f32)
                            .flex_shrink_0()
                            .child(
                                img(ImageSource::Render(image))
                                    .absolute()
                                    .inset_0()
                                    .size_full()
                                    .object_fit(ObjectFit::Contain)
                                    .id("media-image")
                                    .debug_selector(|| "media-image".into()),
                            ),
                    )
                }
                Some(Err(error)) => {
                    card.child(div().p_4().text_sm().text_color(danger).child(format!(
                        "Image preview unavailable: {error}. Retry, or save the original file."
                    )))
                }
                None => card.child(
                    div()
                        .p_4()
                        .text_sm()
                        .text_color(muted)
                        .child("Loading image…"),
                ),
            };
        }
        let open_tooltip = if self.open_allowed {
            "Open attachment in the system default app"
        } else {
            "Opening this file type is disabled for safety; save a copy instead"
        };
        card = card.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_2()
                .p_3()
                .child(Icon::new(icon).text_color(muted).flex_shrink_0())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(div().text_sm().truncate().child(self.name_label.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(self.detail_label.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_1()
                        .child(
                            Button::new("media-open")
                                .ghost()
                                .small()
                                .icon(IconName::ExternalLink)
                                .label("Open")
                                .accessibility_label("Open attachment in system app")
                                .tooltip(open_tooltip)
                                .disabled(!ready || !self.open_allowed || self.busy)
                                .on_click(cx.listener(|this, _, window, cx| this.open(window, cx))),
                        )
                        .child(
                            Button::new("media-save")
                                .ghost()
                                .small()
                                .icon(IconName::ArrowDown)
                                .label("Save")
                                .accessibility_label("Save attachment as a file")
                                .tooltip("Save a copy…")
                                .disabled(!ready || self.busy)
                                .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                        )
                        .child(
                            Button::new("media-retry")
                                .ghost()
                                .small()
                                .icon(IconName::RefreshCw)
                                .accessibility_label("Retry loading attachment")
                                .tooltip("Retry loading attachment")
                                .disabled(loading || self.busy)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.resolve(window, cx)),
                                ),
                        ),
                ),
        );
        if loading {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_sm()
                    .text_color(muted)
                    .child("Locating attachment…"),
            );
        }
        if let LocalAsset::Missing(error) = &self.local {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_sm()
                    .text_color(danger)
                    .child("Missing attachment. Ask the host to provide this asset, then Retry.")
                    .child(div().text_xs().child(error.clone())),
            );
        }
        if !self.open_allowed && ready {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_xs()
                    .text_color(muted)
                    .child("Open disabled for this file type. You can still save a copy."),
            );
        }
        if let Some(error) = &self.error {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_sm()
                    .text_color(danger)
                    .child(error.clone()),
            );
        }
        if self.busy {
            card = card.child(
                div()
                    .px_3()
                    .pb_3()
                    .text_xs()
                    .text_color(muted)
                    .child("Working…"),
            );
        }
        #[cfg(test)]
        let card = {
            use gpui_kit::test::TestSupportExt;
            card.test_support()
        };
        card
    }
}

fn resolve_local(store: &dyn MediaStore, attachment: &MediaAttachment) -> anyhow::Result<PathBuf> {
    let path = store.local_path(attachment)?;
    if !path.is_absolute() {
        bail!("The media store returned a non-local path");
    }
    let metadata =
        std::fs::symlink_metadata(&path).context("Attachment is not available locally")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("The attachment is not a regular local file");
    }
    Ok(path)
}

// Only passive, commonly associated formats are launched. Unknown formats can always be saved.
fn permits_system_open(attachment: &MediaAttachment) -> bool {
    let extension = Path::new(&attachment.file_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("");
    let extension = extension.to_ascii_lowercase();
    let mime = attachment.mime_type.to_ascii_lowercase();
    if mime.contains("javascript")
        || mime.contains("executable")
        || mime.contains("shellscript")
        || matches!(
            mime.as_str(),
            "text/html" | "application/xhtml+xml" | "image/svg+xml"
        )
    {
        return false;
    }
    matches!(
        extension.as_str(),
        "png"
            | "jpg"
            | "jpeg"
            | "gif"
            | "webp"
            | "bmp"
            | "tif"
            | "tiff"
            | "ico"
            | "avif"
            | "mp4"
            | "m4v"
            | "mov"
            | "webm"
            | "mkv"
            | "avi"
            | "mpeg"
            | "mpg"
            | "mp3"
            | "m4a"
            | "aac"
            | "wav"
            | "flac"
            | "ogg"
            | "opus"
            | "aiff"
            | "aif"
            | "pdf"
            | "txt"
            | "md"
            | "csv"
            | "tsv"
            | "rtf"
            | "doc"
            | "docx"
            | "xlsx"
            | "pptx"
            | "odt"
            | "ods"
            | "odp"
    )
}

fn verify_system_open(attachment: &MediaAttachment, path: &Path) -> anyhow::Result<()> {
    if !permits_system_open(attachment) {
        bail!("Opening executable or active content is disabled. Save a copy instead.");
    }
    // The actual launch path must retain the safe association, not merely its display name.
    let actual_extension = path.extension().and_then(|extension| extension.to_str());
    let named_extension = Path::new(&attachment.file_name)
        .extension()
        .and_then(|extension| extension.to_str());
    if !actual_extension
        .zip(named_extension)
        .is_some_and(|(actual, named)| actual.eq_ignore_ascii_case(named))
    {
        bail!("The local file type differs from the attachment name. Save a copy instead.");
    }
    let mut file = File::open(path).context("Cannot read attachment")?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 != 0 {
            bail!("Opening files with executable permissions is disabled. Save a copy instead.");
        }
    }
    if !metadata.is_file() {
        bail!("The attachment is no longer a regular file");
    }
    let mut header = [0u8; 4];
    let len = metadata.len().min(header.len() as u64) as usize;
    file.read_exact(&mut header[..len])?;
    if header[..len].starts_with(b"#!")
        || header[..len].starts_with(b"MZ")
        || header == *b"\x7fELF"
        || matches!(
            u32::from_be_bytes(header),
            0xfeedface
                | 0xfeedfacf
                | 0xcefaedfe
                | 0xcffaedfe
                | 0xcafebabe
                | 0xbebafeca
                | 0xcafebabf
                | 0xbfbafeca
        )
    {
        bail!("This attachment contains executable content. Save a copy instead.");
    }
    Ok(())
}

fn byte_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LocalAsset, MediaAttachment, MediaKind, MediaStore, MediaView, permits_system_open,
        verify_system_open,
    };
    use crate::media::FileMediaStore;
    use gpui_kit::test::TestWindowExt;
    use gpui_kit::{AnyWindowHandle, AppContext, Entity, TestAppContext, WindowOptions};
    use std::{path::PathBuf, sync::Arc};

    struct Files(PathBuf);

    impl Files {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("media-view-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn import(&self) -> (Arc<FileMediaStore>, MediaAttachment) {
            let source = self.0.join("notes.txt");
            std::fs::write(&source, b"Shared attachment\n").unwrap();
            let store = Arc::new(FileMediaStore::new(self.0.join("assets")));
            let attachment = store.import_file(&source, Some(MediaKind::File)).unwrap();
            (store, attachment)
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn setup(
        attachment: MediaAttachment,
        store: Arc<dyn MediaStore>,
        cx: &mut TestAppContext,
    ) -> (AnyWindowHandle, Entity<MediaView>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| MediaView::new(attachment, store, window, cx))
            })
            .unwrap()
        })
    }

    #[gpui_kit::test]
    fn local_attachment_resolves_and_missing_asset_recovers_on_retry(cx: &mut TestAppContext) {
        let files = Files::new();
        let (store, attachment) = files.import();
        let path = store.local_path(&attachment).unwrap();
        let (handle, view) = setup(attachment.clone(), store.clone(), cx);
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(matches!(view.local, LocalAsset::Ready(_)))
        });
        std::fs::remove_file(&path).unwrap();
        cx.update_window(handle, |_, window, cx| {
            view.update(cx, |view, cx| view.resolve(window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(matches!(view.local, LocalAsset::Missing(_)))
        });
        store
            .install_file(&attachment, &files.0.join("notes.txt"))
            .unwrap();
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("media-retry", cx);
        })
        .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(matches!(view.local, LocalAsset::Ready(_)));
            assert!(view.error.is_none());
        });
    }

    #[gpui_kit::test]
    fn save_uses_platform_dialog_and_cancel_does_not_export(cx: &mut TestAppContext) {
        let files = Files::new();
        let (store, attachment) = files.import();
        let (handle, view) = setup(attachment, store, cx);
        cx.run_until_parked();
        let destination = files.0.join("saved.txt");
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("media-save", cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| None);
        cx.run_until_parked();
        assert!(!destination.exists());
        view.read_with(cx, |view, _| {
            assert!(!view.busy);
            assert!(view.error.is_none());
        });
        cx.update_window(handle, |_, window, cx| {
            window.render_frame(cx);
            window.click("media-save", cx);
        })
        .unwrap();
        assert!(cx.did_prompt_for_new_path());
        cx.simulate_new_path_selection(|_| Some(destination.clone()));
        cx.run_until_parked();
        assert_eq!(std::fs::read(&destination).unwrap(), b"Shared attachment\n");
        view.read_with(cx, |view, _| {
            assert!(!view.busy);
            assert!(view.error.is_none());
        });
    }

    #[test]
    fn open_rejects_active_extensions_and_disguised_executables() {
        let files = Files::new();
        let (_, mut attachment) = files.import();
        for name in [
            "script.js",
            "page.html",
            "image.svg",
            "macro.docm",
            "launcher.desktop",
            "program.exe",
        ] {
            attachment.file_name = name.into();
            assert!(!permits_system_open(&attachment), "{name}");
        }
        attachment.file_name = "notes.txt".into();
        let path = files.0.join("notes.txt");
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        assert!(verify_system_open(&attachment, &path).is_err());
        std::fs::write(&path, b"MZ executable").unwrap();
        assert!(verify_system_open(&attachment, &path).is_err());
        std::fs::write(&path, b"ordinary text").unwrap();
        assert!(verify_system_open(&attachment, &path).is_ok());
    }
}
