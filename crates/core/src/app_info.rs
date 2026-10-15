//! Application identity. Renaming the app means changing these constants, the
//! `.desktop` and icon files named after [`APP_ID`] in `crates/app/data`, and the
//! names in `install.sh`.

/// Human readable name shown in window titles, dialogs and media controls.
pub const APP_NAME: &str = "Penguin Player";

/// Reverse-DNS id used by GTK, D-Bus (MPRIS) and desktop integration, made from the
/// repository's address (github.com/01Cindyy/Penguin-player) the way Flathub expects:
/// lowercase owner, `_` before a leading digit, and `_` in place of `-`. It must start
/// with [`APP_QUALIFIER`] and [`APP_ORGANIZATION`].
pub const APP_ID: &str = "io.github._01cindyy.Penguin_player";

/// The parts of [`APP_ID`] before the app's own name. macOS and Windows also name the
/// app's folders after them (`~/Library/Application Support/io.github._01cindyy.penguin-player`,
/// `%APPDATA%\_01cindyy\penguin-player`).
pub const APP_QUALIFIER: &str = "io.github";
pub const APP_ORGANIZATION: &str = "_01cindyy";

/// Folder name used for the config, data and cache directories.
pub const APP_DIR_NAME: &str = "penguin-player";

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Where the app is developed. Web requests from plugins name it, so sites know who asks.
pub const APP_WEBSITE: &str = "https://github.com/01Cindyy/Penguin-player";

/// The app's application in Discord's developer portal. Discord shows its name
/// ("Listening to Penguin Player"), and plugins can show the images uploaded to it
/// (the app's icon is `penguin`). An empty id leaves Discord out.
pub const DISCORD_APPLICATION_ID: &str = "1559915885395976192";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_id_starts_with_the_qualifier_and_organization() {
        let prefix = format!("{APP_QUALIFIER}.{APP_ORGANIZATION}.");
        assert!(
            APP_ID.starts_with(&prefix),
            "{APP_ID} should start with {prefix}"
        );
    }
}
