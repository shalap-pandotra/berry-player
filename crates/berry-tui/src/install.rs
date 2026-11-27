//! `berry --install` and `berry --uninstall`: copy berry somewhere permanent and put it on your
//! PATH (just for you, no admin needed), or undo both. Also `--version` and `--help`.
//!
//! PATH is edited straight in the registry, the way Windows' own settings dialog does it, keeping
//! every existing entry byte for byte (the quick PowerShell way can flatten `%USERPROFILE%`-style
//! entries into fixed paths and break other programs).

use std::path::{Path, PathBuf};

/// %LOCALAPPDATA%\Programs\berry, where installed programs for one user conventionally live.
fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join("berry")
}

/// Handle berry's command-line flags. True if one was handled (berry should exit).
pub fn handle_flags() -> bool {
    match std::env::args().nth(1).as_deref() {
        Some("--install") => install(),
        Some("--uninstall") => uninstall(),
        Some("--version" | "-V") => println!("berry {}", env!("CARGO_PKG_VERSION")),
        Some("--help" | "-h") => println!(
            "berry {}: a terminal music player\n\n\
             usage:\n  berry               play from your Music folder (or the last one opened with :folder)\n  \
             berry <folder>      play from that folder\n  berry --install     copy berry somewhere permanent and add it to your PATH\n  \
             berry --uninstall   undo --install (your library cache and settings stay)\n  berry --version\n\n\
             inside berry, ? shows the keys and :help the commands",
            env!("CARGO_PKG_VERSION")
        ),
        _ => return false,
    }
    true
}

/// Whether running `berry` from a new terminal would find this copy: its folder is on the PATH.
/// Used to offer `berry --install` to people who run it from their Downloads folder.
pub fn on_path() -> bool {
    let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) else { return true };
    let dir = normalise(&dir);
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| normalise(&d) == dir))
}

fn normalise(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

fn install() {
    if !cfg!(windows) { return println!("--install only works on Windows; elsewhere, put berry in a folder on your PATH"); }
    let dir = install_dir();
    let target = dir.join("berry.exe");
    let Ok(me) = std::env::current_exe() else { return println!("couldn't work out where berry is running from") };
    if normalise(&me) != normalise(&target) {
        if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::copy(&me, &target)) {
            println!("couldn't copy berry to {}: {e}", dir.display());
            if target.exists() { println!("(if berry is open somewhere, close it and try again)"); }
            return;
        }
        println!("copied berry to {}", target.display());
    }
    match path::add(&dir) {
        Ok(true) => {
            println!("added {} to your PATH", dir.display());
            println!("\nopen a new terminal and type berry to run it from anywhere.");
            if !on_path() { println!("you can delete the copy you just ran"); }
            println!("to undo all this, run berry --uninstall");
        }
        Ok(false) => println!("{} is already on your PATH, so berry is ready to go", dir.display()),
        Err(e) => println!("couldn't update your PATH: {e}"),
    }
}

fn uninstall() {
    let dir = install_dir();
    match path::remove(&dir) {
        Ok(true) => println!("removed {} from your PATH", dir.display()),
        Ok(false) => println!("berry's install folder wasn't on your PATH"),
        Err(e) => println!("couldn't update your PATH: {e}"),
    }
    if !dir.exists() {
        println!("nothing installed at {}", dir.display());
    } else if std::env::current_exe().is_ok_and(|me| normalise(&me).starts_with(&normalise(&dir))) {
        // a running program can't delete itself, so a little helper waits a moment and does it
        path::delete_after_exit(&dir);
        println!("berry will finish removing {} as soon as it closes", dir.display());
    } else {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => println!("deleted {}", dir.display()),
            Err(e) => println!("couldn't delete {}: {e} (if berry is open somewhere, close it and try again)", dir.display()),
        }
    }
    let data = install_dir().parent().and_then(Path::parent).map(|p| p.join("berry"));
    if let Some(data) = data.filter(|d| d.exists()) {
        println!("\nyour library cache and settings are still in {}; delete that folder too for a clean slate", data.display());
    }
}

#[cfg(windows)]
mod path {
    use std::path::Path;
    use winreg::enums::{RegType, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
    use winreg::{RegKey, RegValue};

    /// The user PATH as stored: its entries, and the registry type to write it back as.
    fn read() -> std::io::Result<(Vec<String>, RegType)> {
        let env = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags("Environment", KEY_READ)?;
        match env.get_raw_value("Path") {
            Ok(v) => {
                let units: Vec<u16> = v.bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let text = String::from_utf16_lossy(&units);
                let text = text.trim_end_matches('\0');
                let entries = text.split(';').filter(|e| !e.is_empty()).map(str::to_string).collect();
                Ok((entries, v.vtype))
            }
            // no user PATH yet: the usual type is REG_EXPAND_SZ
            Err(_) => Ok((Vec::new(), RegType::REG_EXPAND_SZ)),
        }
    }

    fn write(entries: &[String], vtype: RegType) -> std::io::Result<()> {
        let env = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags("Environment", KEY_READ | KEY_WRITE)?;
        let mut bytes: Vec<u8> = entries.join(";").encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
        bytes.shrink_to_fit();
        env.set_raw_value("Path", &RegValue { bytes, vtype })?;
        announce();
        Ok(())
    }

    /// Tell running programs (Explorer, so new terminals) that the environment changed.
    fn announce() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        let what: Vec<u16> = "Environment".encode_utf16().chain([0]).collect();
        unsafe {
            SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, what.as_ptr() as isize, SMTO_ABORTIFHUNG, 3000, std::ptr::null_mut());
        }
    }

    fn same(entry: &str, dir: &Path) -> bool {
        // entries may use %VARIABLES%: compare them expanded
        let expanded = expand(entry);
        super::normalise(Path::new(&expanded)) == super::normalise(dir)
    }

    fn expand(s: &str) -> String {
        let mut out = String::new();
        let mut rest = s;
        while let Some(start) = rest.find('%') {
            out.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            match after.find('%') {
                Some(end) => {
                    let name = &after[..end];
                    out.push_str(&std::env::var(name).unwrap_or_else(|_| format!("%{name}%")));
                    rest = &after[end + 1..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// Add a folder to the user PATH. False if it was already there.
    pub fn add(dir: &Path) -> std::io::Result<bool> {
        let (mut entries, vtype) = read()?;
        if entries.iter().any(|e| same(e, dir)) { return Ok(false); }
        entries.push(dir.to_string_lossy().into_owned());
        write(&entries, vtype)?;
        Ok(true)
    }

    /// Take a folder off the user PATH. False if it wasn't there.
    pub fn remove(dir: &Path) -> std::io::Result<bool> {
        let (mut entries, vtype) = read()?;
        let before = entries.len();
        entries.retain(|e| !same(e, dir));
        if entries.len() == before { return Ok(false); }
        write(&entries, vtype)?;
        Ok(true)
    }

    /// Delete a folder a couple of seconds from now, once this program has exited.
    pub fn delete_after_exit(dir: &Path) {
        use std::os::windows::process::CommandExt;
        const DETACHED: u32 = 0x0000_0008 | 0x0800_0000; // DETACHED_PROCESS | CREATE_NO_WINDOW
        // ping is the old trick for "wait a couple of seconds" that works without a console window
        // (timeout doesn't). raw_arg passes the line through exactly, so cmd sees the quotes as written.
        let _ = std::process::Command::new("cmd")
            .raw_arg(format!("/C ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()))
            .creation_flags(DETACHED)
            .spawn();
    }
}

#[cfg(not(windows))]
mod path {
    use std::path::Path;
    fn unsupported() -> std::io::Error {
        std::io::Error::other("--install only works on Windows; elsewhere, put berry in a folder on your PATH")
    }
    pub fn add(_: &Path) -> std::io::Result<bool> { Err(unsupported()) }
    pub fn remove(_: &Path) -> std::io::Result<bool> { Err(unsupported()) }
    pub fn delete_after_exit(_: &Path) {}
}
