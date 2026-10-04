//! 音频标签写入（lofty 0.22）：title/artist/album + 嵌入歌词 + 封面
use anyhow::Result;
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::{ItemKey, Tag, TagType};
use std::path::Path;

/// 封面统一规格化：最长边 ≤1600（保纵横比）、转 JPEG q85。
/// 网易原图可达 3000px/1.5MB，直接内嵌会让普通播放器加载失败；
/// 解码/编码任一步失败返回 None（调用方跳过嵌入，不中断标签写入）
fn prepare_cover(data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?;
    let rgb = img.thumbnail(1600, 1600).to_rgb8();
    let mut out = std::io::Cursor::new(Vec::new());
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 85)
        .encode_image(&image::DynamicImage::ImageRgb8(rgb))
        .ok()?;
    Some(out.into_inner())
}

/// 自定义字段键：网易云歌曲 ID（FLAC 写为 Vorbis 键，MP3 写为 TXXX 描述帧）
const NCM_ID_KEY: &str = "ncm-id";

/// 写入标签；lyrics 为 LRC 全文（嵌入为 USLT / Vorbis LYRICS），cover 为封面图片字节，
/// ncm_id 为网易云歌曲 ID（写入后可凭此标签直接重新下载/更新）
pub fn write_tags(
    path: &Path,
    title: &str,
    artists: &str,
    album: &str,
    lyrics: Option<&str>,
    cover: Option<&[u8]>,
    ncm_id: Option<u64>,
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
    if let Some(id) = ncm_id {
        // Unknown 键需走 insert_unchecked（insert_text 的 re_map 会拒绝无映射键）
        tag.insert_unchecked(lofty::tag::TagItem::new(
            ItemKey::Unknown(NCM_ID_KEY.to_string()),
            lofty::tag::ItemValue::Text(id.to_string()),
        ));
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

    /// 大图压到 ≤1600px 且输出 JPEG；垃圾输入返回 None
    #[test]
    fn prepare_cover_compresses_and_rejects_garbage() {
        let big = image::DynamicImage::ImageRgb8(image::RgbImage::new(2400, 1200));
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::write_to(&big, &mut png, image::ImageFormat::Png).unwrap();
        let out = prepare_cover(png.get_ref()).expect("应压缩成功");
        assert_eq!(&out[..2], &[0xFF, 0xD8], "输出应为 JPEG");
        let dec = image::load_from_memory(&out).unwrap();
        assert!(
            dec.width() <= 1600 && dec.height() <= 1600,
            "最长边应 ≤1600"
        );
        assert!(prepare_cover(b"not an image").is_none());
    }

    /// 构造最小合法 FLAC：fLaC + STREAMINFO(非 last) + PADDING(last)
    /// （仅 STREAMINFO 作 last 块时 lofty 写 padding 会越界，故补尾部 PADDING）
    fn minimal_flac() -> Vec<u8> {
        let mut v = Vec::from(&b"fLaC"[..]);
        v.push(0x00); // 非 last | 0x00(STREAMINFO)
        v.extend_from_slice(&34u32.to_be_bytes()[1..]);
        let mut si = vec![0u8; 34];
        si[0..2].copy_from_slice(&4096u16.to_be_bytes()); // min block size
        si[2..4].copy_from_slice(&4096u16.to_be_bytes()); // max block size
                                                          // 10..18: 20bit 采样率 | 3bit 声道-1=0(单声道) | 5bit 位深-1=15(16bit) | 36bit 总采样数=0
        let sr = 44100u32; // 0x0AC44
        si[10] = (sr >> 12) as u8;
        si[11] = (sr >> 4) as u8;
        si[12] = ((sr & 0xF) << 4) as u8; // 采样率低4位；声道/位深最高位均为 0
        si[13] = 0xF0; // 位深-1 低4位=0b1111；总采样数高4位=0
        v.extend_from_slice(&si);
        v.push(0x81); // last | 0x01(PADDING)
        v.extend_from_slice(&0u32.to_be_bytes()[1..]); // 空 padding
        v
    }

    /// ncm-id 应写入为 Vorbis 键（FLAC），可原样读回
    #[test]
    fn writes_ncm_id_as_vorbis_key() {
        let dir = std::env::temp_dir().join("ncm_rsid_test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.flac");
        std::fs::write(&p, minimal_flac()).unwrap();

        write_tags(&p, "T", "A", "L", None, None, Some(123456)).unwrap();

        let read = lofty::read_from_path(&p).unwrap();
        let tag = read.primary_tag().or(read.first_tag()).expect("应有标签");
        let got = tag.get_string(&ItemKey::Unknown(NCM_ID_KEY.to_string()));
        assert_eq!(got, Some("123456"), "ncm-id 应可读回");

        // 二次写入应覆盖而非重复
        write_tags(&p, "T2", "A2", "L2", None, None, Some(789)).unwrap();
        let read = lofty::read_from_path(&p).unwrap();
        let tag = read.primary_tag().or(read.first_tag()).unwrap();
        assert_eq!(
            tag.get_string(&ItemKey::Unknown(NCM_ID_KEY.to_string())),
            Some("789")
        );
        assert_eq!(tag.title().as_deref(), Some("T2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 无 ncm_id 时不写入该键
    #[test]
    fn skips_ncm_id_when_absent() {
        let dir = std::env::temp_dir().join("ncm_rsid_test2");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.flac");
        std::fs::write(&p, minimal_flac()).unwrap();

        write_tags(&p, "T", "A", "L", None, None, None).unwrap();

        let read = lofty::read_from_path(&p).unwrap();
        let tag = read.primary_tag().or(read.first_tag()).unwrap();
        assert_eq!(
            tag.get_string(&ItemKey::Unknown(NCM_ID_KEY.to_string())),
            None
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// MP3 侧：Unknown 键经 Tag→Id3v2Tag 转换应落成 TXXX(ncm-id) 并可读回
    #[test]
    fn writes_ncm_id_as_txxx_on_mp3() {
        let mut tag = Tag::new(TagType::Id3v2);
        tag.insert_unchecked(lofty::tag::TagItem::new(
            ItemKey::Unknown(NCM_ID_KEY.to_string()),
            lofty::tag::ItemValue::Text("424242".to_string()),
        ));
        let id3: lofty::id3::v2::Id3v2Tag = tag.into();
        assert_eq!(
            id3.get_user_text(NCM_ID_KEY),
            Some("424242"),
            "ncm-id 应落成 TXXX 描述帧"
        );
    }
}
