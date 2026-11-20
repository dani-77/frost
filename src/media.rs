use iced::widget::image;
use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::OnceLock,
};

pub const SYMBOLS: &[u8] = include_bytes!("../assets/FrostSymbols.ttf");
pub const ICON_FONT: iced::Font = iced::Font::with_name("Symbols Nerd Font");

pub fn ice_icon() -> image::Handle {
    static ICON: OnceLock<image::Handle> = OnceLock::new();
    ICON.get_or_init(|| image::Handle::from_bytes(include_bytes!("../assets/ice.png").as_slice()))
        .clone()
}

pub fn owl_icon() -> image::Handle {
    static ICON: OnceLock<image::Handle> = OnceLock::new();
    ICON.get_or_init(|| image::Handle::from_bytes(include_bytes!("../assets/owl.png").as_slice()))
        .clone()
}

pub fn icon_theme() -> String {
    if let Ok(theme) = env::var("FROST_ICON_THEME")
        && !theme.is_empty()
    {
        return theme;
    }
    let config = env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")));
    for gtk in ["gtk-4.0", "gtk-3.0"] {
        if let Some(data) = config
            .as_ref()
            .and_then(|path| fs::read_to_string(path.join(gtk).join("settings.ini")).ok())
            && let Some(theme) = data.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "gtk-icon-theme-name" && !value.trim().is_empty())
                    .then(|| value.trim().trim_matches('"').to_owned())
            })
        {
            return theme;
        }
    }
    "Adwaita".into()
}

pub fn app_icon(value: &str, theme: &str) -> Option<PathBuf> {
    let path = Path::new(value);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_owned());
    }
    if path.components().count() != 1 {
        return None;
    }
    let name = match path.extension().and_then(|ext| ext.to_str()) {
        Some("png" | "svg" | "xpm") => path.file_stem()?.to_str()?,
        _ => value,
    };
    freedesktop_icons::lookup(name)
        .with_theme(theme)
        .with_size(32)
        .with_cache()
        .find()
        .or_else(|| {
            freedesktop_icons::lookup(name)
                .with_size(32)
                .with_cache()
                .find()
        })
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    matches!(ext, "png" | "svg" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
                })
        })
}

pub fn thumbnail(path: &Path) -> Option<image::Handle> {
    let mut reader = ::image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = ::image::Limits::default();
    limits.max_image_width = Some(12_288);
    limits.max_image_height = Some(12_288);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let thumb = reader.decode().ok()?.thumbnail(336, 216).into_rgba8();
    Some(image::Handle::from_rgba(
        thumb.width(),
        thumb.height(),
        thumb.into_raw(),
    ))
}
