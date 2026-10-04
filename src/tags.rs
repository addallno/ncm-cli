//! 音频标签写入（lofty 0.22）：title/artist/album + 嵌入歌词 + 封面
use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};
use std::path::Path;

/// 封面统一规格化：最长边 ≤1000（保纵横比）、转 JPEG q85。
/// 网易原图可达 3000px/1.5MB，直接内嵌会让普通播放器加载失败；
/// 解码/编码任一步失败返回 None（调用方跳过嵌入，不中断标签写入）
fn prepare_cover(data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let rgb = img.thumbnail(1000, 1000).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&image::DynamicImage::ImageRgb8(rgb))
        .ok()?;
    Some(out.into_inner())
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
        // 规格化失败只跳过封面，标题/歌词等已设字段照常落盘
        if let Some(prepared) = prepare_cover(data) {
            tag.remove_picture_type(PictureType::CoverFront);
            tag.push_picture(Picture::new_unchecked(
                PictureType::CoverFront,
                Some(MimeType::Jpeg),
                Some("Cover".to_string()),
                prepared,
            ));
        }
    }

    tagged.save_to_path(path, WriteOptions::default())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 大图压到 ≤1000px 且输出 JPEG；垃圾输入返回 None
    #[test]
    fn prepare_cover_compresses_and_rejects_garbage() {
        let big = image::DynamicImage::ImageRgb8(image::RgbImage::new(2400, 1200));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::write_to(&big, &mut png, image::ImageFormat::Png).unwrap();
        let out = prepare_cover(png.get_ref()).expect("应压缩成功");
        assert_eq!(&out[..2], &[0xFF, 0xD8], "输出应为 JPEG");
        let dec = image::load_from_memory(&out).unwrap();
        assert!(
            dec.width() <= 1000 && dec.height() <= 1000,
            "最长边应 ≤1000"
        );
        assert!(prepare_cover(b"not an image").is_none());
    }
}
