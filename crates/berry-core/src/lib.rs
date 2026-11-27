//! berry's guts: the music library (and its cache), playlists and lyrics. No terminal, no audio
//! output: the terminal player and, later, the Android app both build on this.

pub mod library;
pub mod lyrics;
pub mod playlist;

/// Background programs shouldn't flash a console window up on Windows.
pub fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd;
}
