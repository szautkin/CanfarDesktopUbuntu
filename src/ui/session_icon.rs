use gtk4::{self as gtk};

/// Create a session type icon as a gtk::Image, sized to `pixel_size`.
pub fn session_type_icon(session_type: &str, pixel_size: i32) -> gtk::Image {
    let bytes: &[u8] = match session_type.to_lowercase().as_str() {
        "notebook" => include_bytes!("../../assets/session-notebook.jpg"),
        "desktop" | "headless" => include_bytes!("../../assets/session-desktop.png"),
        "carta" => include_bytes!("../../assets/session-carta.png"),
        "contributed" => include_bytes!("../../assets/session-contributed.png"),
        "firefly" => include_bytes!("../../assets/session-firefly.png"),
        _ => include_bytes!("../../assets/session-desktop.png"),
    };

    let gbytes = gtk::glib::Bytes::from_static(bytes);
    let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
    let pixbuf = gtk::gdk_pixbuf::Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE);

    let image = match pixbuf {
        Ok(pb) => {
            let scaled = pb
                .scale_simple(
                    pixel_size,
                    pixel_size,
                    gtk::gdk_pixbuf::InterpType::Bilinear,
                )
                .unwrap_or(pb);
            let texture = gtk::gdk::Texture::for_pixbuf(&scaled);
            gtk::Image::from_paintable(Some(&texture))
        }
        Err(_) => gtk::Image::from_icon_name("application-x-executable-symbolic"),
    };
    image.set_pixel_size(pixel_size);
    image
}

/// The session-type logo as a texture, or `None` if the asset fails to decode.
pub fn session_type_texture(session_type: &str, pixel_size: i32) -> Option<gtk::gdk::Texture> {
    let bytes: &[u8] = match session_type.to_lowercase().as_str() {
        "notebook" => include_bytes!("../../assets/session-notebook.jpg"),
        "desktop" | "headless" => include_bytes!("../../assets/session-desktop.png"),
        "carta" => include_bytes!("../../assets/session-carta.png"),
        "contributed" => include_bytes!("../../assets/session-contributed.png"),
        "firefly" => include_bytes!("../../assets/session-firefly.png"),
        _ => include_bytes!("../../assets/session-desktop.png"),
    };
    let gbytes = gtk::glib::Bytes::from_static(bytes);
    let stream = gtk::gio::MemoryInputStream::from_bytes(&gbytes);
    let pb = gtk::gdk_pixbuf::Pixbuf::from_stream(&stream, gtk::gio::Cancellable::NONE).ok()?;
    let scaled = pb
        .scale_simple(pixel_size, pixel_size, gtk::gdk_pixbuf::InterpType::Bilinear)
        .unwrap_or(pb);
    Some(gtk::gdk::Texture::for_pixbuf(&scaled))
}

/// The session-type logo clipped inside a round avatar. The raw assets carry
/// opaque white backgrounds; the avatar crop keeps cards clean.
pub fn session_type_avatar(session_type: &str, size: i32) -> libadwaita::Avatar {
    let avatar = libadwaita::Avatar::new(size, Some(session_type), true);
    if let Some(texture) = session_type_texture(session_type, size * 2) {
        avatar.set_custom_image(Some(&texture));
    }
    avatar
}
