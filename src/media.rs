//! Replicated media references and a persistent, host-local asset store.
use crate::native_editor::rich::{RichAtom, RichDocument, RichKind, RichMarks, RichNode, RichRun};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};
use uuid::Uuid;

const MEDIA_NAME: &str = "gpui-kit-media";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

impl MediaKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "Image",
            Self::Video => "Video",
            Self::Audio => "Audio",
            Self::File => "File",
        }
    }
}

/// Portable metadata only: asset bytes and machine-local paths never enter CRDT state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaAttachment {
    pub asset_id: String,
    pub kind: MediaKind,
    pub file_name: String,
    pub mime_type: String,
    pub byte_len: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaPayload {
    version: u32,
    attachment: MediaAttachment,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MediaEnvelope<'a> {
    codec: &'a str,
    data: MediaPayload,
}

impl MediaAttachment {
    fn validate(&self) -> Result<()> {
        let id = Uuid::parse_str(&self.asset_id).context("invalid media asset ID")?;
        ensure!(
            self.asset_id.len() == 36
                && !self.asset_id.bytes().any(|byte| byte.is_ascii_uppercase())
                && !id.is_nil(),
            "unsafe media asset ID"
        );
        validate_file_name(&self.file_name)?;
        let (major, minor) = self
            .mime_type
            .split_once('/')
            .context("invalid media MIME type")?;
        ensure!(
            !major.is_empty()
                && !minor.is_empty()
                && self.mime_type.bytes().all(|byte| byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || b"/!#$&^_.+-".contains(&byte))
                && !minor.contains('/'),
            "invalid media MIME type"
        );
        let expected = match self.kind {
            MediaKind::Image => Some("image"),
            MediaKind::Video => Some("video"),
            MediaKind::Audio => Some("audio"),
            MediaKind::File => None,
        };
        ensure!(
            expected.is_none_or(|expected| major == expected),
            "media kind and MIME type disagree"
        );
        Ok(())
    }

    fn markdown(&self) -> String {
        let mut name = String::with_capacity(self.file_name.len());
        for character in self.file_name.chars() {
            if matches!(character, '\\' | '[' | ']') {
                name.push('\\');
            }
            name.push(character);
        }
        format!("[{name}](asset://{})", self.asset_id)
    }

    fn accessibility_label(&self) -> String {
        format!("{}: {}", self.kind.label(), self.file_name)
    }

    pub fn to_content(&self) -> Result<RichDocument> {
        self.validate()?;
        let mut node = RichNode::paragraph("");
        node.kind = RichKind::Custom;
        node.runs.push(RichRun {
            text: "\u{FFFC}".into(),
            marks: RichMarks::default(),
            atom: Some(RichAtom::Custom {
                name: MEDIA_NAME.into(),
                text: self.file_name.clone(),
                markdown: self.markdown(),
                accessibility_label: self.accessibility_label(),
                payload: serde_json::json!({
                    "codec": "json",
                    "data": { "version": 1, "attachment": self },
                }),
            }),
        });
        Ok(RichDocument {
            blocks: vec![node],
            ..RichDocument::default()
        })
    }

    /// Only a standalone atomic media block is rendered as a media card.
    pub fn from_content(content: &RichDocument) -> Result<Option<Self>> {
        content
            .validate()
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        let mut attachment = None;
        for node in &content.blocks {
            let media = validate_node(node)?;
            if content.blocks.len() == 1 {
                attachment = media;
            }
        }
        Ok(attachment)
    }
}

fn validate_file_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.ends_with(['.', ' '])
            && !name
                .chars()
                .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c)),
        "unsafe media file name"
    );
    let stem = name.split('.').next().unwrap_or_default();
    ensure!(
        !["CON", "PRN", "AUX", "NUL"]
            .iter()
            .any(|reserved| stem.eq_ignore_ascii_case(reserved))
            && !(stem.len() == 4
                && (stem.as_bytes()[..3].eq_ignore_ascii_case(b"COM")
                    || stem.as_bytes()[..3].eq_ignore_ascii_case(b"LPT"))
                && matches!(stem.as_bytes()[3], b'1'..=b'9')),
        "reserved media file name"
    );
    Ok(())
}

fn media_atom(atom: &RichAtom) -> Result<Option<MediaAttachment>> {
    let RichAtom::Custom {
        name,
        text,
        markdown,
        accessibility_label,
        payload,
    } = atom
    else {
        return Ok(None);
    };
    if name != MEDIA_NAME {
        return Ok(None);
    }
    let envelope = MediaEnvelope::deserialize(payload).context("invalid media payload")?;
    ensure!(
        envelope.codec == "json" && envelope.data.version == 1,
        "unsupported media payload version or codec"
    );
    let media = envelope.data.attachment;
    media.validate()?;
    ensure!(
        text == &media.file_name
            && markdown == &media.markdown()
            && accessibility_label == &media.accessibility_label(),
        "media presentation does not match its metadata"
    );
    Ok(Some(media))
}

/// Validate reserved payloads before storage projections can hide their original runs.
pub(crate) fn validate_media_atom(atom: &RichAtom) -> Result<()> {
    media_atom(atom)?;
    Ok(())
}

fn validate_node(node: &RichNode) -> Result<Option<MediaAttachment>> {
    let mut attachment = None;
    for run in &node.runs {
        let Some(atom) = &run.atom else { continue };
        let Some(media) = media_atom(atom)? else {
            continue;
        };
        ensure!(
            node.kind == RichKind::Custom
                && node.runs.len() == 1
                && node.children.is_empty()
                && run.text == "\u{FFFC}"
                && run.marks == RichMarks::default(),
            "media requires a single unmarked custom block"
        );
        attachment = Some(media);
    }
    for child in &node.children {
        validate_node(child)?;
    }
    Ok(attachment)
}

/// Reserved-name validation is additive; other extensions retain their own payload schemas.
pub(crate) fn validate_media_content(content: &RichDocument) -> Result<()> {
    for node in &content.blocks {
        validate_node(node)?;
    }
    Ok(())
}

/// Blocking filesystem operations. Callers must dispatch them off the UI thread.
pub trait MediaStore: Send + Sync {
    fn import_file(&self, path: &Path, requested: Option<MediaKind>) -> Result<MediaAttachment>;
    fn local_path(&self, attachment: &MediaAttachment) -> Result<PathBuf>;
    fn export_file(&self, attachment: &MediaAttachment, destination: &Path) -> Result<()>;
}

/// Immutable UUID-named files; filenames are display metadata, never storage paths.
#[derive(Clone, Debug)]
pub struct FileMediaStore {
    root: Option<PathBuf>,
}

impl Default for FileMediaStore {
    fn default() -> Self {
        Self {
            root: default_root(),
        }
    }
}

impl FileMediaStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: Some(root.into()),
        }
    }

    fn root(&self, create: bool) -> Result<PathBuf> {
        let root = self
            .root
            .as_ref()
            .context("persistent user data location is unavailable")?;
        if create {
            fs::create_dir_all(root).context("creating media asset directory")?;
        }
        ensure!(
            fs::symlink_metadata(root)?.file_type().is_dir(),
            "media root must be a real directory"
        );
        fs::canonicalize(root).context("resolving media asset directory")
    }

    /// Fulfil a peer's reference without changing its replicated asset ID.
    pub fn install_file(&self, attachment: &MediaAttachment, source: &Path) -> Result<()> {
        attachment.validate()?;
        let mut input = open_regular(source)?;
        ensure!(
            input.metadata()?.len() == attachment.byte_len,
            "media asset size mismatch"
        );
        let (kind, mime) = detect_media(&mut input, &attachment.file_name)?;
        ensure!(
            (attachment.kind == MediaKind::File || attachment.kind == kind)
                && attachment.mime_type == mime,
            "media asset type mismatch"
        );
        let root = self.root(true)?;
        let destination = asset_path(&root, attachment);
        if fs::symlink_metadata(&destination).is_ok() {
            let mut existing = open_managed(&root, &destination, attachment.byte_len)?;
            ensure!(
                same_bytes(&mut existing, &mut input)?,
                "media asset ID already contains different bytes"
            );
            return Ok(());
        }
        let (temporary, mut output) = temporary_file(&root)?;
        ensure!(
            io::copy(&mut input, &mut output)? == attachment.byte_len,
            "media asset changed during import"
        );
        output.sync_all()?;
        drop(output);
        fs::hard_link(&temporary.0, &destination).context("installing immutable media asset")?;
        Ok(())
    }
}

impl MediaStore for FileMediaStore {
    fn import_file(&self, path: &Path, requested: Option<MediaKind>) -> Result<MediaAttachment> {
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("media filename is not Unicode")?
            .to_owned();
        validate_file_name(&file_name)?;
        let mut input = open_regular(path)?;
        let byte_len = input.metadata()?.len();
        let (inferred, mime_type) = detect_media(&mut input, &file_name)?;
        let kind = requested.unwrap_or(inferred);
        ensure!(
            kind == MediaKind::File || kind == inferred,
            "selected file is not supported {} media",
            kind.label()
        );
        let attachment = MediaAttachment {
            asset_id: Uuid::new_v4().to_string(),
            kind,
            file_name,
            mime_type: mime_type.into(),
            byte_len,
        };
        let root = self.root(true)?;
        let (temporary, mut output) = temporary_file(&root)?;
        ensure!(
            io::copy(&mut input, &mut output)? == byte_len,
            "media asset changed during import"
        );
        output.sync_all()?;
        drop(output);
        fs::hard_link(&temporary.0, asset_path(&root, &attachment))
            .context("installing imported media asset")?;
        Ok(attachment)
    }
    fn local_path(&self, attachment: &MediaAttachment) -> Result<PathBuf> {
        attachment.validate()?;
        let root = self.root(false)?;
        let path = asset_path(&root, attachment);
        open_managed(&root, &path, attachment.byte_len)?;
        Ok(path)
    }

    fn export_file(&self, attachment: &MediaAttachment, destination: &Path) -> Result<()> {
        attachment.validate()?;
        let root = self.root(false)?;
        let source = asset_path(&root, attachment);
        let mut input = open_managed(&root, &source, attachment.byte_len)?;
        let parent = destination
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).context("resolving export directory")?;
        ensure!(
            parent != root,
            "cannot export into the managed asset directory"
        );
        let destination = parent.join(
            destination
                .file_name()
                .context("export requires a filename")?,
        );
        match fs::symlink_metadata(&destination) {
            Ok(metadata) => {
                ensure!(
                    metadata.file_type().is_file(),
                    "export destination must be a regular file"
                );
                ensure!(
                    fs::canonicalize(&destination)? != source,
                    "cannot export over the managed asset"
                );
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let (temporary, mut output) = temporary_file(&parent)?;
        ensure!(
            io::copy(&mut input, &mut output)? == attachment.byte_len,
            "media asset size mismatch"
        );
        output.sync_all()?;
        drop(output);
        fs::rename(&temporary.0, &destination).context("saving media asset")?;
        Ok(())
    }
}

fn asset_path(root: &Path, attachment: &MediaAttachment) -> PathBuf {
    match Path::new(&attachment.file_name).extension() {
        Some(extension) => {
            let mut name = std::ffi::OsString::from(&attachment.asset_id);
            name.push(".");
            name.push(extension);
            root.join(name)
        }
        None => root.join(&attachment.asset_id),
    }
}

fn default_root() -> Option<PathBuf> {
    fn absolute_env(name: &str) -> Option<PathBuf> {
        env::var_os(name)
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
    }
    if cfg!(target_os = "windows") {
        absolute_env("LOCALAPPDATA").map(|root| root.join("gpui-kit-block-editor").join("media"))
    } else if cfg!(target_os = "macos") {
        absolute_env("HOME")
            .map(|root| root.join("Library/Application Support/gpui-kit-block-editor/media"))
    } else {
        absolute_env("XDG_DATA_HOME")
            .or_else(|| absolute_env("HOME").map(|root| root.join(".local/share")))
            .map(|root| root.join("gpui-kit-block-editor/media"))
    }
}

fn open_regular(path: &Path) -> Result<File> {
    let metadata = fs::symlink_metadata(path).context("reading media file metadata")?;
    ensure!(
        metadata.file_type().is_file(),
        "media source must be a regular file, not a symlink"
    );
    let file = File::open(path).context("opening media file")?;
    let opened = file.metadata()?;
    ensure!(opened.is_file(), "media source must be a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.dev() == opened.dev() && metadata.ino() == opened.ino(),
            "media file changed while opening"
        );
    }
    Ok(file)
}

fn open_managed(root: &Path, path: &Path, byte_len: u64) -> Result<File> {
    let file = open_regular(path)?;
    ensure!(
        fs::canonicalize(path)?.parent() == Some(root),
        "media asset is outside its managed directory"
    );
    ensure!(
        file.metadata()?.len() == byte_len,
        "media asset size mismatch"
    );
    Ok(file)
}

struct TemporaryFile(PathBuf);
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn temporary_file(parent: &Path) -> Result<(TemporaryFile, File)> {
    let path = parent.join(format!(".{}.partial", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&path).context("creating media staging file")?;
    Ok((TemporaryFile(path), file))
}

fn same_bytes(left: &mut File, right: &mut File) -> Result<bool> {
    let mut left_buffer = [0; 8192];
    let mut right_buffer = [0; 8192];
    loop {
        let length = left.read(&mut left_buffer)?;
        if length == 0 {
            return Ok(right.read(&mut right_buffer[..1])? == 0);
        }
        if let Err(error) = right.read_exact(&mut right_buffer[..length]) {
            if error.kind() == io::ErrorKind::UnexpectedEof {
                return Ok(false);
            }
            return Err(error.into());
        }
        if left_buffer[..length] != right_buffer[..length] {
            return Ok(false);
        }
    }
}

fn detect_media(file: &mut File, name: &str) -> Result<(MediaKind, &'static str)> {
    let mut prefix = [0; 512];
    let mut length = 0;
    while length < prefix.len() {
        let count = file.read(&mut prefix[length..])?;
        if count == 0 {
            break;
        }
        length += count;
    }
    file.seek(SeekFrom::Start(0))?;
    let bytes = &prefix[..length];
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let extension_is = |expected: &str| extension.eq_ignore_ascii_case(expected);
    use MediaKind::{Audio, File as Other, Image, Video};
    let detected = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        (Image, "image/png")
    } else if bytes.starts_with(b"\xff\xd8\xff") {
        (Image, "image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (Image, "image/gif")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        (Image, "image/webp")
    } else if bytes.starts_with(b"BM") {
        (Image, "image/bmp")
    } else if bytes.starts_with(b"II*\0") || bytes.starts_with(b"MM\0*") {
        (Image, "image/tiff")
    } else if extension_is("svg")
        && std::str::from_utf8(bytes).is_ok_and(|text| {
            text.trim_start_matches('\u{feff}')
                .trim_start()
                .starts_with("<svg")
                || text.trim_start().starts_with("<?xml") && text.contains("<svg")
        })
    {
        (Image, "image/svg+xml")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE") {
        (Audio, "audio/wav")
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"AVI ") {
        (Video, "video/x-msvideo")
    } else if bytes.starts_with(b"fLaC") {
        (Audio, "audio/flac")
    } else if bytes.starts_with(b"ID3")
        || bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe6 == 0xe2
    {
        (Audio, "audio/mpeg")
    } else if bytes.starts_with(b"OggS") {
        if bytes.windows(7).any(|window| window == b"\x80theora") {
            (Video, "video/ogg")
        } else {
            (Audio, "audio/ogg")
        }
    } else if bytes.get(4..8) == Some(b"ftyp") {
        if extension_is("m4a") || bytes.get(8..12) == Some(b"M4A ") {
            (Audio, "audio/mp4")
        } else if extension_is("mov") || bytes.get(8..12) == Some(b"qt  ") {
            (Video, "video/quicktime")
        } else {
            (Video, "video/mp4")
        }
    } else if bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        if extension_is("mka") {
            (Audio, "audio/x-matroska")
        } else if extension_is("mkv") {
            (Video, "video/x-matroska")
        } else {
            (Video, "video/webm")
        }
    } else if bytes.starts_with(b"%PDF-") {
        (Other, "application/pdf")
    } else if extension_is("mp3") {
        (Audio, "audio/mpeg")
    } else if extension_is("aac") {
        (Audio, "audio/aac")
    } else if extension_is("aif") || extension_is("aiff") {
        (Audio, "audio/aiff")
    } else if extension_is("mp4") || extension_is("m4v") {
        (Video, "video/mp4")
    } else if extension_is("mov") {
        (Video, "video/quicktime")
    } else if extension_is("webm") {
        (Video, "video/webm")
    } else if extension_is("txt") || extension_is("md") {
        (Other, "text/plain")
    } else if bytes.starts_with(b"PK\x03\x04") && extension_is("docx") {
        (
            Other,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        )
    } else if bytes.starts_with(b"PK\x03\x04") {
        (Other, "application/zip")
    } else {
        (Other, "application/octet-stream")
    };
    Ok(detected)
}
