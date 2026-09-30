//! Image variants through Cloudflare Image Transformations: resized copies
//! made on demand by Cloudflare's edge from a URL, never by the Worker.

use std::fmt::Write as _;

/// How a [`Variant`] fits the image in its `width` × `height` box (Cloudflare's `fit` option).
///
/// # Examples
///
/// ```
/// use ocre::storage::{Fit, Variant};
///
/// let thumb = Variant::new().width(200).height(200).fit(Fit::Cover);
/// assert_eq!(thumb.path("/photos/1/image"), "/cdn-cgi/image/width=200,height=200,fit=cover,format=auto/photos/1/image");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Like `Contain`, but never enlarges a smaller image (Active Storage's `resize_to_limit`).
    ScaleDown,
    /// Fits inside the box, keeping the aspect ratio (`resize_to_fit`).
    Contain,
    /// Fills the box, cropping what overflows (`resize_to_fill`).
    Cover,
    /// Like `Cover`, but never enlarges a smaller image.
    Crop,
    /// Like `Contain`, then pads to the exact box size (`resize_and_pad`).
    Pad,
}

impl Fit {
    fn as_str(self) -> &'static str {
        match self {
            Self::ScaleDown => "scale-down",
            Self::Contain => "contain",
            Self::Cover => "cover",
            Self::Crop => "crop",
            Self::Pad => "pad",
        }
    }
}

/// A resized version of an image, made by Cloudflare Image Transformations (Active Storage's variants).
///
/// [`Variant::path`] builds `/cdn-cgi/image/<options>/<source>`: Cloudflare's
/// edge fetches the source image (for example the app's own route that
/// [`serve`](crate::storage::serve)s it), resizes it and caches the result.
/// Variants are lazy (made on the first request, like Rails'
/// `variant(...).processed` on first view) and never touch the Worker's
/// CPU or R2's storage. `format=auto` is always set: browsers that accept
/// AVIF or WebP get it, others get the original format.
///
/// Needs a custom domain (a zone on Cloudflare) with Transformations turned
/// on in the dashboard (Images > Transformations); `*.workers.dev` hosts
/// cannot use it. Declare variants as constants, like [`Rules`](crate::storage::Rules).
///
/// # Examples
///
/// ```
/// use ocre::storage::{Fit, Variant};
///
/// const THUMB: Variant = Variant::new().width(300).height(300).fit(Fit::Cover).quality(80);
/// const LARGE: Variant = Variant::new().width(1600);
///
/// assert_eq!(
///     THUMB.path("/photos/7/image"),
///     "/cdn-cgi/image/width=300,height=300,fit=cover,quality=80,format=auto/photos/7/image"
/// );
/// assert_eq!(LARGE.path("https://files.example.com/a.jpg"), "/cdn-cgi/image/width=1600,format=auto/https://files.example.com/a.jpg");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Variant {
    width: Option<u32>,
    height: Option<u32>,
    fit: Option<Fit>,
    quality: Option<u8>,
}

impl Variant {
    /// A variant with no resizing: only `format=auto`.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(ocre::storage::Variant::new().path("/logo.png"), "/cdn-cgi/image/format=auto/logo.png");
    /// ```
    pub const fn new() -> Self {
        Self { width: None, height: None, fit: None, quality: None }
    }

    /// Sets the largest width, in pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// let path = ocre::storage::Variant::new().width(640).path("/a.png");
    /// assert_eq!(path, "/cdn-cgi/image/width=640,format=auto/a.png");
    /// ```
    pub const fn width(mut self, pixels: u32) -> Self {
        self.width = Some(pixels);
        self
    }

    /// Sets the largest height, in pixels.
    ///
    /// # Examples
    ///
    /// ```
    /// let path = ocre::storage::Variant::new().height(480).path("/a.png");
    /// assert_eq!(path, "/cdn-cgi/image/height=480,format=auto/a.png");
    /// ```
    pub const fn height(mut self, pixels: u32) -> Self {
        self.height = Some(pixels);
        self
    }

    /// Sets how the image fits the `width` × `height` box; without it, Cloudflare scales the image down to fit.
    ///
    /// # Examples
    ///
    /// ```
    /// use ocre::storage::{Fit, Variant};
    ///
    /// let path = Variant::new().width(100).height(100).fit(Fit::Pad).path("/a.png");
    /// assert_eq!(path, "/cdn-cgi/image/width=100,height=100,fit=pad,format=auto/a.png");
    /// ```
    pub const fn fit(mut self, fit: Fit) -> Self {
        self.fit = Some(fit);
        self
    }

    /// Sets the JPEG/WebP/AVIF quality, 1 to 100 (values outside are clamped).
    ///
    /// # Examples
    ///
    /// ```
    /// let path = ocre::storage::Variant::new().quality(200).path("/a.jpg");
    /// assert_eq!(path, "/cdn-cgi/image/quality=100,format=auto/a.jpg");
    /// ```
    pub const fn quality(mut self, quality: u8) -> Self {
        self.quality = Some(if quality == 0 {
            1
        } else if quality > 100 {
            100
        } else {
            quality
        });
        self
    }

    /// The same-origin path of this variant of `source`: `/cdn-cgi/image/<options>/<source>`.
    ///
    /// `source` is a path on the same zone (`/photos/1/image`, the leading
    /// `/` is dropped as Cloudflare expects) or an absolute `https://` URL
    /// (allowed in the zone's Transformations settings). Pure; use it in
    /// templates as `src="{{ THUMB.path(photo_path) }}"`.
    ///
    /// # Examples
    ///
    /// ```
    /// let path = ocre::storage::Variant::new().width(64).path("//avatars/1");
    /// assert_eq!(path, "/cdn-cgi/image/width=64,format=auto/avatars/1");
    /// ```
    pub fn path(&self, source: &str) -> String {
        let mut path = String::from("/cdn-cgi/image/");
        if let Some(width) = self.width {
            write!(path, "width={width},").expect("writing to a String");
        }
        if let Some(height) = self.height {
            write!(path, "height={height},").expect("writing to a String");
        }
        if let Some(fit) = self.fit {
            write!(path, "fit={},", fit.as_str()).expect("writing to a String");
        }
        if let Some(quality) = self.quality {
            write!(path, "quality={quality},").expect("writing to a String");
        }
        path.push_str("format=auto/");
        let absolute = source.starts_with("https://") || source.starts_with("http://");
        path.push_str(if absolute { source } else { source.trim_start_matches('/') });
        path
    }
}

#[cfg(test)]
#[path = "../../tests/storage/variant.rs"]
mod tests;
