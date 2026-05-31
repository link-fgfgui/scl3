use scl_core::auth::structs::AuthMethod;
use tracing::error;

pub fn avatar_cache_dir() -> std::path::PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("scl3")
        .join("avatars")
}

pub fn save_avatar_cache(uuid: &str, head_skin: &[u8], hat_skin: &[u8]) {
    if head_skin.is_empty() {
        return;
    }
    let dir = avatar_cache_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        error!("创建头像缓存目录失败: {}", e);
        return;
    }
    let mut img = match image::RgbaImage::from_raw(8, 8, head_skin.to_vec()) {
        Some(img) => img,
        None => return,
    };
    if let Some(hat) = image::RgbaImage::from_raw(8, 8, hat_skin.to_vec()) {
        for (x, y, hat_pixel) in hat.enumerate_pixels() {
            if hat_pixel.0[3] > 0 {
                img.put_pixel(x, y, *hat_pixel);
            }
        }
    }
    let resized = image::imageops::resize(&img, 128, 128, image::imageops::FilterType::Nearest);
    let path = dir.join(format!("{}.png", uuid));
    if let Err(e) = resized.save(&path) {
        error!("保存头像缓存失败: {}", e);
    }
}

pub fn save_avatar_from_auth_method(method: &AuthMethod) {
    let (uuid, head_skin, hat_skin) = match method {
        AuthMethod::Mojang {
            uuid,
            head_skin,
            hat_skin,
            ..
        }
        | AuthMethod::Microsoft {
            uuid,
            head_skin,
            hat_skin,
            ..
        }
        | AuthMethod::AuthlibInjector {
            uuid,
            head_skin,
            hat_skin,
            ..
        } => (uuid.as_str(), head_skin.as_slice(), hat_skin.as_slice()),
        _ => return,
    };
    save_avatar_cache(uuid, head_skin, hat_skin);
}

pub fn load_avatar_image(uuid: &str) -> Option<slint::Image> {
    let path = avatar_cache_dir().join(format!("{}.png", uuid));
    let img = image::open(&path).ok()?;
    let rgba = img.to_rgba8();
    Some(slint::Image::from_rgba8(
        slint::SharedPixelBuffer::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()),
    ))
}
