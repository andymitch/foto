use std::path::{Path, PathBuf};

use foto_core::Platform;

pub struct Desktop;

impl Platform for Desktop {
    fn thumbnail(&self, path: String, video: bool, max_px: u32) -> Option<Vec<u8>> {
        #[cfg(target_os = "macos")]
        {
            crate::macos::thumbnail(&path, video, max_px)
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (path, video, max_px);
            None
        }
    }

    fn trash(&self, _: Vec<String>) -> bool {
        false
    }

    fn is_metered(&self) -> bool {
        false
    }
}

pub fn full_image(path: &Path) -> Option<Vec<u8>> {
    if let Some(b) = Desktop.thumbnail(path.to_string_lossy().into_owned(), false, 4096) {
        return Some(b);
    }
    let img = image::open(path).ok()?;
    let img = if img.width().max(img.height()) > 4096 {
        img.thumbnail(4096, 4096)
    } else {
        img
    };
    Some(foto_core::media::jpeg(&img))
}

const LABEL: &str = "com.andymitch.foto";

fn autostart_file() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var("HOME").ok()?);
    Some(if cfg!(target_os = "macos") {
        home.join("Library/LaunchAgents")
            .join(format!("{LABEL}.plist"))
    } else {
        home.join(".config/autostart/foto.desktop")
    })
}

pub fn autostart_enabled() -> bool {
    autostart_file().is_some_and(|f| f.exists())
}

pub fn set_autostart(on: bool) -> std::io::Result<()> {
    let Some(file) = autostart_file() else {
        return Ok(());
    };
    if !on {
        return match std::fs::remove_file(&file) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let exe = std::env::current_exe()?;
    let exe = exe.to_string_lossy();
    let body = if cfg!(target_os = "macos") {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>{exe}</string><string>--background</string></array>
<key>RunAtLoad</key><true/>
</dict></plist>
"#
        )
    } else {
        format!(
            "[Desktop Entry]\nType=Application\nName=foto\nExec=\"{exe}\" --background\nX-GNOME-Autostart-enabled=true\n"
        )
    };
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, body)
}
