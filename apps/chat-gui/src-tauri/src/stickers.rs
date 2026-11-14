//! Stickers and sticker packs.
//!
//! The frame they travel in (and every check on a received sticker or pack
//! item) moved to `maya_chat::frame`, so the CLI and this app read one format
//! (ADR-140, build PR 1). This module re-exports it and keeps what only the
//! app does: making a pack from imported files.
//!
//! Anyone who received one sticker can ask the sender for the whole pack
//! ([`Content::PackRequest`]). The sender's app answers with one
//! [`Content::PackItem`] per sticker, but only for a pack the sender owns or
//! installed, and the asker's app checks every item against the pack's id.

pub use maya_chat::frame::{
    Content, Format, MAX_PACK_ITEMS, MAX_TITLE_CHARS, Pack, PackItem, check_file, decode, encode, is_structured,
    pack_id, sticker_id,
};

/// Builds a pack, refusing an empty, oversized or untitled one.
///
/// # Errors
///
/// No title, a title over [`MAX_TITLE_CHARS`], no stickers or more than
/// [`MAX_PACK_ITEMS`], or a sticker without an emoji.
pub fn make_pack(title: &str, items: Vec<PackItem>, mine: bool) -> Result<Pack, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("give the pack a name".into());
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err(format!("a pack name is at most {MAX_TITLE_CHARS} characters"));
    }
    if items.is_empty() || items.len() > MAX_PACK_ITEMS {
        return Err(format!("a pack holds 1 to {MAX_PACK_ITEMS} stickers"));
    }
    if items.iter().any(|i| i.emoji.trim().is_empty()) {
        return Err("give every sticker an emoji, so it can be suggested".into());
    }
    Ok(Pack {
        id: pack_id(title, &items),
        title: title.to_owned(),
        items,
        mine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIF: &[u8] = b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff!\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\x02\x02D\x01\x00;";

    fn item(bytes: &[u8], emoji: &str) -> PackItem {
        let (id, format) = check_file(bytes).unwrap();
        PackItem { id, emoji: emoji.into(), format }
    }

    #[test]
    fn formats_are_read_from_the_bytes() {
        assert_eq!(Format::sniff(GIF), Some(Format::Gif));
        assert_eq!(Format::sniff(b"\x89PNG\r\n\x1a\nrest"), Some(Format::Png));
        assert_eq!(Format::sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::Webp));
        assert_eq!(Format::sniff(&[0x1a, 0x45, 0xdf, 0xa3, 1]), Some(Format::Webm));
        assert_eq!(Format::sniff(&[0x1f, 0x8b, 8, 0]), Some(Format::Tgs));
        assert_eq!(Format::sniff(br#"{"v":"5.7","layers":[]}"#), Some(Format::Lottie));
        assert_eq!(Format::sniff(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#), Some(Format::Svg));
        assert_eq!(Format::sniff(br#"{"not":"lottie"}"#), None, "JSON without layers");
        assert_eq!(Format::sniff(b"MZ\x90\x00 an exe"), None);
    }

    #[test]
    fn pack_ids_are_stable_and_pack_items_must_belong() {
        let items = vec![item(GIF, "😂")];
        let pack = make_pack(" Mine ", items.clone(), true).unwrap();
        assert_eq!(pack.title, "Mine");
        assert_eq!(pack.id, pack_id("Mine", &items), "same pack, same id anywhere");
        assert_ne!(pack.id, pack_id("Yours", &items));

        let ok = encode(&Content::PackItem { pack: pack.clone(), index: 0 }, GIF).unwrap();
        assert!(decode(&ok).is_ok());
        assert!(encode(&Content::PackItem { pack, index: 1 }, GIF).is_err(), "past the end");
    }

    #[test]
    fn packs_need_a_name_stickers_and_emoji() {
        assert!(make_pack("", vec![item(GIF, "😂")], true).is_err());
        assert!(make_pack("x", vec![], true).is_err());
        assert!(make_pack("x", vec![item(GIF, " ")], true).is_err());
        assert!(make_pack(&"x".repeat(MAX_TITLE_CHARS + 1), vec![item(GIF, "😂")], true).is_err());
    }
}
