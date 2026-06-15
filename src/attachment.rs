use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use base64::Engine;

use crate::llm::{ProviderContentBlock, ProviderMessage, ProviderRole};

const MAX_TEXT_ATTACHMENT_BYTES: u64 = 256 * 1024;
const MAX_IMAGE_ATTACHMENT_BYTES: u64 = 5 * 1024 * 1024;
const SUPPORTED_TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "rs", "toml", "json", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "go",
    "java", "sql", "html", "css",
];
const SUPPORTED_IMAGE_EXTENSIONS: &[(&str, &str)] = &[
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("webp", "image/webp"),
];

pub fn parse_attachment_input(input: &str) -> Result<Vec<String>> {
    Ok(input
        .split(';')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub fn load_attachment_blocks(paths: &[PathBuf]) -> Result<Vec<ProviderContentBlock>> {
    paths.iter().map(|path| load_single_attachment(path)).collect()
}

pub fn build_user_message(
    text: impl Into<String>,
    attachments: Vec<ProviderContentBlock>,
) -> ProviderMessage {
    let mut content = Vec::with_capacity(1 + attachments.len());
    content.push(ProviderContentBlock::Text { text: text.into() });
    content.extend(attachments);
    ProviderMessage::new_blocks(ProviderRole::User, content)
}

pub fn load_text_attachment(path: &Path) -> Result<ProviderContentBlock> {
    let metadata = validate_attachment_path(path)?;
    let extension = file_extension(path)?;

    if !is_supported_text_extension(&extension) {
        bail!(
            "unsupported text attachment type: {} (supported: {})",
            path.display(),
            SUPPORTED_TEXT_EXTENSIONS.join(", ")
        );
    }

    ensure_size_within_limit(path, metadata.len(), MAX_TEXT_ATTACHMENT_BYTES, "text")?;

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read text attachment: {}", path.display()))?;
    let content = String::from_utf8(bytes)
        .with_context(|| format!("text attachment is not valid UTF-8: {}", path.display()))?;

    Ok(ProviderContentBlock::File {
        filename: file_name(path)?,
        content,
    })
}

pub fn load_image_attachment(path: &Path) -> Result<ProviderContentBlock> {
    let metadata = validate_attachment_path(path)?;
    let extension = file_extension(path)?;
    let media_type = media_type_for_extension(&extension).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported image attachment type: {} (supported: {})",
            path.display(),
            supported_image_extensions()
        )
    })?;

    ensure_size_within_limit(path, metadata.len(), MAX_IMAGE_ATTACHMENT_BYTES, "image")?;

    let bytes = std::fs::read(path)
        .with_context(|| format!("failed to read image attachment: {}", path.display()))?;
    let data_base64 = base64::engine::general_purpose::STANDARD.encode(bytes);

    Ok(ProviderContentBlock::Image {
        source_name: file_name(path)?,
        media_type: media_type.to_string(),
        data_base64,
    })
}

fn load_single_attachment(path: &Path) -> Result<ProviderContentBlock> {
    let extension = file_extension(path)?;

    if is_supported_image_extension(&extension) {
        return load_image_attachment(path);
    }
    if is_supported_text_extension(&extension) {
        return load_text_attachment(path);
    }

    bail!(
        "unsupported attachment type: {} (supported text: {}; supported image: {})",
        path.display(),
        SUPPORTED_TEXT_EXTENSIONS.join(", "),
        supported_image_extensions()
    );
}

fn validate_attachment_path(path: &Path) -> Result<std::fs::Metadata> {
    if !path.exists() {
        bail!("attachment file does not exist: {}", path.display());
    }

    let metadata = std::fs::metadata(path)
        .with_context(|| format!("failed to read attachment metadata: {}", path.display()))?;
    if !metadata.is_file() {
        bail!("attachment path is not a file: {}", path.display());
    }

    Ok(metadata)
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow::anyhow!("attachment path has no valid file name: {}", path.display()))
}

fn file_extension(path: &Path) -> Result<String> {
    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("attachment file has no supported extension: {}", path.display()))
}

fn ensure_size_within_limit(path: &Path, actual_size: u64, limit: u64, kind: &str) -> Result<()> {
    if actual_size > limit {
        bail!(
            "{kind} attachment exceeds size limit ({actual_size} bytes > {limit} bytes): {}",
            path.display()
        );
    }

    Ok(())
}

fn is_supported_text_extension(extension: &str) -> bool {
    SUPPORTED_TEXT_EXTENSIONS.contains(&extension)
}

fn is_supported_image_extension(extension: &str) -> bool {
    SUPPORTED_IMAGE_EXTENSIONS
        .iter()
        .any(|(candidate, _)| *candidate == extension)
}

fn media_type_for_extension(extension: &str) -> Option<&'static str> {
    SUPPORTED_IMAGE_EXTENSIONS
        .iter()
        .find_map(|(candidate, media_type)| (*candidate == extension).then_some(*media_type))
}

fn supported_image_extensions() -> String {
    SUPPORTED_IMAGE_EXTENSIONS
        .iter()
        .map(|(extension, _)| *extension)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{build_user_message, load_attachment_blocks, parse_attachment_input};
    use crate::llm::{ProviderContentBlock, ProviderMessage, ProviderRole};

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new(name: &str) -> Self {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let sequence = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "little-agent-{name}-{}-{timestamp}-{sequence}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn parses_semicolon_separated_attachment_paths() {
        let parsed = parse_attachment_input(r"C:\a.png; C:\b.toml ;").unwrap();
        assert_eq!(parsed, vec!["C:\\a.png", "C:\\b.toml"]);
    }

    #[test]
    fn rejects_unsupported_attachment_extension() {
        let dir = TestDir::new("unsupported");
        let path = dir.path.join("archive.zip");
        fs::write(&path, b"fake").unwrap();

        let error = load_attachment_blocks(&[path]).unwrap_err().to_string();

        assert!(error.contains("unsupported attachment type"));
    }

    #[test]
    fn loads_text_file_attachment() {
        let dir = TestDir::new("text");
        let path = dir.path.join("notes.md");
        fs::write(&path, "# Notes\nhello").unwrap();

        let blocks = load_attachment_blocks(&[path]).unwrap();

        assert_eq!(
            blocks,
            vec![ProviderContentBlock::File {
                filename: "notes.md".to_string(),
                content: "# Notes\nhello".to_string(),
            }]
        );
    }

    #[test]
    fn loads_image_file_attachment() {
        let dir = TestDir::new("image");
        let path = dir.path.join("diagram.png");
        fs::write(&path, [0_u8, 1, 2, 3]).unwrap();

        let blocks = load_attachment_blocks(&[path]).unwrap();

        assert_eq!(
            blocks,
            vec![ProviderContentBlock::Image {
                source_name: "diagram.png".to_string(),
                media_type: "image/png".to_string(),
                data_base64: "AAECAw==".to_string(),
            }]
        );
    }

    #[test]
    fn builds_user_message_with_text_and_attachments() {
        let message = build_user_message(
            "summarize these files",
            vec![
                ProviderContentBlock::File {
                    filename: "notes.md".to_string(),
                    content: "# Notes".to_string(),
                },
                ProviderContentBlock::Image {
                    source_name: "diagram.png".to_string(),
                    media_type: "image/png".to_string(),
                    data_base64: "AAECAw==".to_string(),
                },
            ],
        );

        assert_eq!(
            message,
            ProviderMessage::new_blocks(
                ProviderRole::User,
                vec![
                    ProviderContentBlock::Text {
                        text: "summarize these files".to_string(),
                    },
                    ProviderContentBlock::File {
                        filename: "notes.md".to_string(),
                        content: "# Notes".to_string(),
                    },
                    ProviderContentBlock::Image {
                        source_name: "diagram.png".to_string(),
                        media_type: "image/png".to_string(),
                        data_base64: "AAECAw==".to_string(),
                    },
                ],
            )
        );
    }

    #[test]
    fn builds_user_message_with_text_only() {
        let message = build_user_message("hello", Vec::new());

        assert_eq!(message, ProviderMessage::new_text(ProviderRole::User, "hello"));
    }
}
