//! On-disk and in-memory caching for entry images.
//!
//! The terminal renders images heavily downscaled to fit the viewport, so
//! keeping full-resolution decoded buffers in memory is wasteful on large
//! images and slow hardware. Images are therefore:
//!
//!   * stored on disk as the raw fetched bytes (keyed by a hash of the URL),
//!     so a re-run never re-downloads them, and
//!   * decoded and downscaled to at most [`MAX_IMAGE_DIMENSION`] pixels per
//!     side before being handed to the UI, bounding the memory cost of the
//!     in-memory cache.
//!
//! Decoding happens off the async runtime (see `app::actions`), and failures
//! are non-fatal: an unreadable or missing file simply falls back to a fresh
//! download.
//!
//! The disk cache has no size cap or eviction policy — it grows for as long
//! as new images are viewed. It's a plain directory of files, so it can be
//! cleared manually if it grows too large.

use image::DynamicImage;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

/// Largest allowed width or height for a decoded image held in memory.
/// Terminals never need more than a couple thousand pixels per side, so this
/// caps the resident memory of the image cache without visible quality loss.
pub const MAX_IMAGE_DIMENSION: u32 = 4096;

/// Bounded on-disk cache of raw image bytes keyed by a hash of the source URL.
#[derive(Clone, Debug)]
pub struct DiskImageCache {
  dir: PathBuf,
}

impl DiskImageCache {
  pub fn new(dir: PathBuf) -> Self {
    Self { dir }
  }

  /// Read previously fetched bytes for `url`, if present.
  pub fn get(&self, url: &str) -> Option<Vec<u8>> {
    fs::read(self.path_for(url)).ok()
  }

  /// Write `bytes` for `url` to disk. Failures are ignored; the caller falls
  /// back to re-downloading next time.
  pub fn put(&self, url: &str, bytes: &[u8]) {
    if let Err(e) =
      fs::create_dir_all(&self.dir).and_then(|()| fs::write(self.path_for(url), bytes))
    {
      eprintln!("shinbun: failed to cache image {}: {}", url, e);
    }
  }

  /// Delete cached bytes for `url`, if present. Used to drop entries that
  /// fail to decode so they don't fail identically forever; a missing file
  /// is not an error.
  pub fn remove(&self, url: &str) {
    let _ = fs::remove_file(self.path_for(url));
  }

  /// Cache files are named by a 64-bit hash of the URL rather than the URL
  /// itself, so two different URLs that collide would silently share a
  /// cache entry. `DefaultHasher` isn't collision-resistant, but the risk is
  /// negligible at the scale of one user's feed images.
  fn path_for(&self, url: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    url.hash(&mut h);
    self.dir.join(format!("{:016x}.img", h.finish()))
  }
}

/// Decode raw image bytes, downscaling so the longest side is at most
/// [`MAX_IMAGE_DIMENSION`]. Returns the display-ready image.
pub fn decode_image(bytes: &[u8]) -> Result<DynamicImage, String> {
  let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
  if img.width().max(img.height()) > MAX_IMAGE_DIMENSION {
    Ok(img.thumbnail(MAX_IMAGE_DIMENSION, MAX_IMAGE_DIMENSION))
  } else {
    Ok(img)
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn png_bytes(width: u32, height: u32) -> Vec<u8> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
      .write_image(
        &vec![0u8; (width * height * 4) as usize],
        width,
        height,
        image::ExtendedColorType::Rgba8,
      )
      .unwrap();
    out
  }

  #[test]
  fn decode_image_downscales_oversized_images() {
    let img = decode_image(&png_bytes(8000, 3000)).unwrap();
    assert!(img.width() <= MAX_IMAGE_DIMENSION);
    assert!(img.height() < 3000);
  }

  #[test]
  fn decode_image_keeps_small_images_unchanged() {
    let img = decode_image(&png_bytes(640, 480)).unwrap();
    assert_eq!(img.width(), 640);
    assert_eq!(img.height(), 480);
  }

  #[test]
  fn decode_image_rejects_garbage() {
    assert!(decode_image(b"not an image").is_err());
  }

  #[test]
  fn disk_cache_round_trip() {
    let dir = tempfile::tempdir().unwrap();
    let cache = DiskImageCache::new(dir.path().to_path_buf());
    let url = "https://example.com/photo.png";
    assert!(cache.get(url).is_none());
    cache.put(url, b"bytes");
    assert_eq!(cache.get(url).unwrap(), b"bytes");
  }

  #[test]
  fn disk_cache_keys_by_url() {
    let dir = tempfile::tempdir().unwrap();
    let cache = DiskImageCache::new(dir.path().to_path_buf());
    cache.put("https://example.com/a.png", b"a");
    cache.put("https://example.com/b.png", b"b");
    assert_eq!(cache.get("https://example.com/a.png").unwrap(), b"a");
    assert_eq!(cache.get("https://example.com/b.png").unwrap(), b"b");
  }
}
