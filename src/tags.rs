//! 音频标签写入（lofty 0.22，title/artist/album 三键）
use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::{Tag, TagType};
use std::path::Path;

pub fn write_tags(path: &Path, title: &str, artists: &str, album: &str) -> Result<()> {
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
    tagged.save_to_path(path, WriteOptions::default())?;
    Ok(())
}
