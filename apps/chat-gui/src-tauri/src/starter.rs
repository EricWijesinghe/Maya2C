//! The starter pack, built into the app: twelve animated stickers of one
//! character, drawn by `stickers/make_starter.py` (original art, Forged Light
//! palette). Built in so a new user can say something with a sticker before
//! they have added or made any pack.

use crate::stickers::{Pack, PackItem, check_file, make_pack};

/// The pack's name in the picker.
const TITLE: &str = "Orbi";

/// (emoji, file), in picker order.
const FILES: [(&str, &[u8]); 12] = [
    ("😊", include_bytes!("../stickers/starter/01-happy.svg")),
    ("😂", include_bytes!("../stickers/starter/02-laugh.svg")),
    ("😍", include_bytes!("../stickers/starter/03-love.svg")),
    ("😢", include_bytes!("../stickers/starter/04-sad.svg")),
    ("😠", include_bytes!("../stickers/starter/05-angry.svg")),
    ("😮", include_bytes!("../stickers/starter/06-wow.svg")),
    ("🤔", include_bytes!("../stickers/starter/07-thinking.svg")),
    ("👍", include_bytes!("../stickers/starter/08-thumbs-up.svg")),
    ("🎉", include_bytes!("../stickers/starter/09-party.svg")),
    ("😴", include_bytes!("../stickers/starter/10-sleepy.svg")),
    ("😎", include_bytes!("../stickers/starter/11-cool.svg")),
    ("👋", include_bytes!("../stickers/starter/12-wave.svg")),
];

/// The starter pack.
///
/// # Panics
///
/// Never in a build that passed its tests: the files are fixed at compile
/// time and `the_starter_pack_is_valid` checks every one.
#[must_use]
pub fn pack() -> Pack {
    let items = FILES
        .iter()
        .map(|(emoji, bytes)| {
            let (id, format) = check_file(bytes).expect("a starter sticker is valid");
            PackItem {
                id,
                emoji: (*emoji).to_owned(),
                format,
            }
        })
        .collect();
    make_pack(TITLE, items, true).expect("the starter pack is valid")
}

/// A starter sticker's file, by id.
#[must_use]
pub fn file(id: &str) -> Option<&'static [u8]> {
    FILES
        .iter()
        .map(|(_, bytes)| *bytes)
        .find(|bytes| crate::stickers::sticker_id(bytes) == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_starter_pack_is_valid() {
        let pack = pack();
        assert_eq!(pack.items.len(), FILES.len());
        for item in &pack.items {
            assert_eq!(item.format, crate::stickers::Format::Svg);
            let bytes = file(&item.id).expect("each item has its file");
            let text = std::str::from_utf8(bytes).expect("utf-8");
            assert!(!text.contains("<script") && !text.contains("href"), "no script or link");
        }
    }
}
