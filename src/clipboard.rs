//! A small UI adapter for the cliphist history already used by d77 sessions.

use std::{io::Cursor, process::Stdio, time::Duration};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Debug)]
pub struct Entry {
    pub id: u64,
    pub mime: String,
    pub format: String,
    pub preview: String,
}

impl Entry {
    pub fn is_image(&self) -> bool {
        !self.format.is_empty() || self.mime.starts_with("image/")
    }

    pub fn content_type(&self) -> Option<&str> {
        if !self.mime.is_empty() {
            Some(&self.mime)
        } else {
            match self.format.as_str() {
                "png" => Some("image/png"),
                "jpeg" | "jpg" => Some("image/jpeg"),
                "gif" => Some("image/gif"),
                "webp" => Some("image/webp"),
                "bmp" => Some("image/bmp"),
                "tiff" => Some("image/tiff"),
                _ => None,
            }
        }
    }
}

fn image_format(preview: &str) -> String {
    if !preview.starts_with("[[ binary data ") {
        return String::new();
    }
    preview
        .split_whitespace()
        .find(|part| {
            matches!(
                *part,
                "png" | "jpeg" | "jpg" | "gif" | "webp" | "bmp" | "tiff"
            )
        })
        .unwrap_or_default()
        .to_owned()
}

fn parse_list(output: &str, fields: bool) -> Vec<Entry> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(if fields { 4 } else { 2 }, '\t');
            let id = parts.next()?.parse().ok()?;
            let (mime, format, preview) = if fields {
                (parts.next()?, parts.next()?, parts.next()?)
            } else {
                ("", "", parts.next()?)
            };
            let format = if format.is_empty() {
                image_format(preview)
            } else {
                format.to_owned()
            };
            Some(Entry {
                id,
                mime: mime.to_owned(),
                format,
                preview: preview.to_owned(),
            })
        })
        .collect()
}

pub async fn list() -> Result<Vec<Entry>, String> {
    let modern =
        crate::services::command("cliphist", &["list", "-fields", "id,mime,format,preview"]).await;
    match modern {
        Ok(output) => Ok(parse_list(&output, true)),
        Err(_) => {
            let output = crate::services::command("cliphist", &["list"]).await?;
            Ok(parse_list(&output, false))
        }
    }
}

async fn decode(id: u64) -> Result<Vec<u8>, String> {
    let output = tokio::time::timeout(
        Duration::from_secs(4),
        tokio::process::Command::new("cliphist")
            .args(["decode", &id.to_string()])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| "cliphist decode timed out".to_owned())?
    .map_err(|err| format!("cliphist decode: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "cliphist decode: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

pub async fn restore(entry: Entry) -> Result<String, String> {
    let bytes = decode(entry.id).await?;
    let mut command = tokio::process::Command::new("wl-copy");
    if let Some(mime) = entry.content_type() {
        command.args(["--type", mime]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|err| format!("wl-copy: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&bytes)
            .await
            .map_err(|err| format!("wl-copy: {err}"))?;
    }
    let result = tokio::time::timeout(Duration::from_secs(4), child.wait_with_output())
        .await
        .map_err(|_| "wl-copy timed out".to_owned())?
        .map_err(|err| format!("wl-copy: {err}"))?;
    if !result.status.success() {
        return Err(format!(
            "wl-copy: {}",
            String::from_utf8_lossy(&result.stderr).trim()
        ));
    }
    Ok(String::new())
}

pub async fn thumbnail(id: u64) -> Option<iced::widget::image::Handle> {
    let bytes = decode(id).await.ok()?;
    tokio::task::spawn_blocking(move || {
        let mut reader = ::image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = ::image::Limits::default();
        limits.max_image_width = Some(12_288);
        limits.max_image_height = Some(12_288);
        limits.max_alloc = Some(128 * 1024 * 1024);
        reader.limits(limits);
        let thumb = reader.decode().ok()?.thumbnail(72, 72).into_rgba8();
        Some(iced::widget::image::Handle::from_rgba(
            thumb.width(),
            thumb.height(),
            thumb.into_raw(),
        ))
    })
    .await
    .ok()
    .flatten()
}

pub async fn wipe() -> Result<String, String> {
    crate::services::command("cliphist", &["wipe"]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_text_and_image_metadata() {
        let entries = parse_list(
            "17\ttext/plain\t\tSome text\n16\timage/png\tpng\t[[ binary data 12 KiB png 80x40 ]]",
            true,
        );
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].is_image());
        assert!(entries[1].is_image());
        assert_eq!(entries[1].content_type(), Some("image/png"));

        let older = parse_list("15\t[[ binary data 2 KiB jpeg 20x20 ]]", false);
        assert_eq!(older[0].content_type(), Some("image/jpeg"));
    }
}
