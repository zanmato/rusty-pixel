use std::collections::HashMap;

use libvips::{Result, VipsImage, ops};

/// How many levels of a band share a bucket. Compression noise around a flat
/// background stays inside one bucket this wide.
const BUCKET_SHIFT: u8 = 3;
const BUCKET_LEVELS: usize = 1 << BUCKET_SHIFT;

/// The canvas colour used when nothing was sampled: white, and fully
/// transparent when the image has an alpha band so a transparent image never
/// gets an opaque frame.
pub fn blank(img: &VipsImage) -> Vec<f64> {
  let mut colour = vec![255.0; img.get_bands().max(1) as usize];
  if img.image_hasalpha()
    && let Some(alpha) = colour.last_mut()
  {
    *alpha = 0.0;
  }
  colour
}

/// The most common colour in the four corners of the image, one value per
/// band.
///
/// Only the corners are sampled, not the whole border. A product that reaches
/// the edges of its picture, such as a phone case with rounded corners, covers
/// most of the border but leaves the background visible in the corners.
///
/// Returns `None` for images that are not 8 bit or have more than four bands,
/// callers fall back to [`blank`].
///
/// The image is read several times, so pass one that is held in memory.
pub fn dominant_colour(img: &VipsImage) -> Result<Option<Vec<f64>>> {
  let (width, height, bands) = (img.get_width(), img.get_height(), img.get_bands() as usize);

  if !matches!(img.get_format(), Ok(ops::BandFormat::Uchar)) || !(1..=4).contains(&bands) {
    return Ok(None);
  }

  // Squares of two percent of the short side, at least two pixels, never
  // past the middle
  let side = ((width.min(height) as f64 * 0.02).round() as i32)
    .max(2)
    .min(height / 2)
    .min(width / 2)
    .max(1);

  let strips = [
    (0, 0, side, side),
    (width - side, 0, side, side),
    (0, height - side, side, side),
    (width - side, height - side, side, side),
  ];

  // Per bucket: pixel count and, per band, how often each level occurs
  let mut buckets: HashMap<u32, (u32, [[u32; BUCKET_LEVELS]; 4])> = HashMap::new();

  for (left, top, w, h) in strips {
    let pixels = ops::extract_area(img, left, top, w, h)?.image_write_to_memory();

    for pixel in pixels.chunks_exact(bands) {
      let key = pixel
        .iter()
        .fold(0u32, |key, v| (key << 5) | (*v >> BUCKET_SHIFT) as u32);
      let entry = buckets.entry(key).or_insert((0, [[0; BUCKET_LEVELS]; 4]));
      entry.0 += 1;
      for (band, v) in pixel.iter().enumerate() {
        entry.1[band][*v as usize % BUCKET_LEVELS] += 1;
      }
    }
  }

  // Ties are broken on the key so the result does not depend on map order
  let Some((key, (_, levels))) = buckets
    .into_iter()
    .max_by_key(|(key, (count, _))| (*count, *key))
  else {
    return Ok(None);
  };

  // Within the winning bucket take the most common level of each band, which
  // gives pure white for a white background instead of an average like 254
  let colour = (0..bands)
    .map(|band| {
      let base = (key >> (5 * (bands - 1 - band))) & 0x1f;
      let level = (0..BUCKET_LEVELS)
        .max_by_key(|l| (levels[band][*l], *l))
        .unwrap_or(0);
      ((base << BUCKET_SHIFT) as usize + level) as f64
    })
    .collect();

  Ok(Some(colour))
}
