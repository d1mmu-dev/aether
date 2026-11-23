//! Playlists: the list (with create) and the detail page (rename, delete, remove tracks, play).

pub mod detail;
pub mod list;

use crate::i18n::Msg;

/// The name as it is sent: trimmed, 1 to 200 characters (the backend's rule), else a catalogue message.
pub fn validate_name(name: &str) -> Result<String, Msg> {
    let name = name.trim();
    match name.chars().count() {
        0 => Err(Msg::EnterPlaylistName),
        1..=200 => Ok(name.to_owned()),
        _ => Err(Msg::PlaylistNameTooLong),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_trimmed_and_bounded() {
        assert_eq!(validate_name(""), Err(Msg::EnterPlaylistName));
        assert_eq!(validate_name("   "), Err(Msg::EnterPlaylistName));
        assert_eq!(validate_name("  Road trip ").unwrap(), "Road trip");
        assert!(validate_name(&"x".repeat(200)).is_ok());
        assert_eq!(
            validate_name(&"x".repeat(201)),
            Err(Msg::PlaylistNameTooLong)
        );
        // Characters, not bytes.
        assert!(validate_name(&"ż".repeat(200)).is_ok());
    }
}
