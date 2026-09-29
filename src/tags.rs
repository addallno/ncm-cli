//! 音频标签写入（lofty 0.22）：title/artist/album + 嵌入歌词 + 封面
use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};
use std::path::Path;

/// 按魔数判断图片 MIME（封面来自 CDN，不信任 URL 后缀）
fn image_mime(data: &[u8]) -> Option<MimeType> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(MimeType::Jpeg)
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some(MimeType::Png)
    } else {
        None
    }
}

/// 写入标签；lyrics 为 LRC 全文（嵌入为 USLT / Vorbis LYRICS），cover 为封面图片字节
pub fn write_tags(
    path: &Path,
    title: &str,
    artists: &str,
    album: &str,
    lyrics: Option<&str>,
    cover: Option<&[u8]>,
) -> Result<()> {
    let mut tagged = lofty::read_from_path(path)?;
    if tagged.primary_tag().is_none() && tagged.first_tag().is_none() {
        let tag_type = match path
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default()
            .as_str()
        {
            "mp3" => TagType::Id3v2,
            "flac" => TagType::VorbisComments,
            "m4a" | "mp4" => TagType::Mp4Ilst,
            "wav" => TagType::Ape,
            _ => TagType::VorbisComments,
        };
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = if tagged.primary_tag().is_some() {
        tagged.primary_tag_mut()
    } else {
        tagged.first_tag_mut()
    }
    .ok_or_else(|| anyhow::anyhow!("文件无可写标签"))?;

    tag.set_title(title.to_string());
    tag.set_artist(artists.to_string());
    tag.set_album(album.to_string());

    if let Some(text) = lyrics.filter(|t| !t.trim().is_empty()) {
        tag.insert_text(ItemKey::Lyrics, text.to_string());
    }
    if let Some(data) = cover.filter(|d| !d.is_empty()) {
        match image_mime(data) {
            Some(mime) => {
                tag.remove_picture_type(PictureType::CoverFront);
                tag.push_picture(Picture::new_unchecked(
                    PictureType::CoverFront,
                    Some(mime),
                    Some("Cover".to_string()),
                    data.to_vec(),
                ));
            }
            None => anyhow::bail!("封面图片格式无法识别（非 jpeg/png/webp）"),
        }
    }

    tagged.save_to_path(path, WriteOptions::default())?;
    Ok(())
}
