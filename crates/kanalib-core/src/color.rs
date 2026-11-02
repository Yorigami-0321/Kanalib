//! F-064: ICC/EXIF colour management.
//!
//! Images carrying a non-sRGB ICC profile (Display P3, Adobe RGB, ...) are
//! converted to sRGB before they reach a thumbnail or the viewer. Pictures
//! without an ICC profile keep their existing behaviour; an embedded profile
//! that is effectively sRGB is left untouched as well. Profile parse failures
//! only downgrade to "no colour management" and never fail thumbnail or scan
//! generation.
//!
//! The conversion is done with Little CMS (`lcms2`, MIT, statically linked).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result, bail};
use exif::{In, Tag};
use image::{
    DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, RgbImage, RgbaImage,
};
use lcms2::{
    CIExyY, CIExyYTRIPLE, ColorSpaceSignature, Intent, PixelFormat, Profile, ToneCurve, Transform,
};

/// Prefix used by the on-demand viewer preview cache. Previews live in
/// `thumbs/previews/` and are removed when the viewer switches image or exits;
/// they are deliberately outside the 10 GiB thumbnail budget/cleanup scan.
pub const PREVIEW_CACHE_VERSION: &str = "p1";

/// EXIF `ColorSpace` / `InteroperabilityIndex` hint. Used only when a file has
/// no ICC profile; a real ICC profile always wins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExifColorSpace {
    /// No usable tag: keep the historical "assume sRGB" behaviour.
    #[default]
    Unspecified,
    /// EXIF ColorSpace = 1.
    Srgb,
    /// EXIF ColorSpace = 2, or 0xFFFF with InteroperabilityIndex = R03.
    AdobeRgb,
}

impl ExifColorSpace {
    pub fn label(self) -> &'static str {
        match self {
            Self::Unspecified | Self::Srgb => "sRGB",
            Self::AdobeRgb => "Adobe RGB (EXIF)",
        }
    }
}

/// Colour space decision for one image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorProfile {
    /// No ICC, sRGB ICC, or an unidentified EXIF colour space: bytes are used
    /// exactly as before.
    Srgb,
    /// Embedded ICC profile that is not effectively sRGB.
    Icc(Vec<u8>),
    /// No ICC, but EXIF declares Adobe RGB (ColorSpace=2 or R03).
    AdobeRgb,
}

impl ColorProfile {
    pub fn needs_conversion(&self) -> bool {
        !matches!(self, Self::Srgb)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Srgb => "sRGB",
            Self::Icc(_) => "ICC",
            Self::AdobeRgb => "Adobe RGB (EXIF)",
        }
    }
}

#[derive(Debug)]
pub struct DecodedImage {
    pub image: DynamicImage,
    pub profile: ColorProfile,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewOutcome {
    /// Source is sRGB (or untagged): the original file can be shown directly.
    Unchanged,
    /// A colour-managed PNG was written to the requested target path.
    Converted,
}

/// Read the EXIF colour-space hint. Kept public so `media::read_exif` can carry
/// it through the scan prefetch without parsing EXIF a second time.
pub fn exif_color_space(exif: &exif::Exif) -> ExifColorSpace {
    // `kamadak-exif` models ColorSpace in the Exif sub-IFD, but some writers put
    // it in IFD0 (Tiff context). Match by tag number so both are recognised.
    let color_space_tag = Tag::ColorSpace.number();
    let field = exif
        .get_field(Tag::ColorSpace, In::PRIMARY)
        .or_else(|| {
            exif.fields()
                .find(|field| field.tag.number() == color_space_tag)
        });
    match field.and_then(|field| field.value.get_uint(0)) {
        Some(1) => ExifColorSpace::Srgb,
        Some(2) => ExifColorSpace::AdobeRgb,
        Some(0xFFFF) => {
            let interop_tag = Tag::InteroperabilityIndex.number();
            let interop = exif
                .get_field(Tag::InteroperabilityIndex, In::PRIMARY)
                .or_else(|| {
                    exif.fields()
                        .find(|field| field.tag.number() == interop_tag)
                })
                .and_then(|field| match &field.value {
                    exif::Value::Ascii(values) => values
                        .first()
                        .and_then(|value| std::str::from_utf8(value).ok())
                        .map(|value| value.trim_matches('\0').trim().to_string()),
                    _ => None,
                });
            if interop
                .as_deref()
                .is_some_and(|value| value.eq_ignore_ascii_case("R03"))
            {
                ExifColorSpace::AdobeRgb
            } else {
                ExifColorSpace::Unspecified
            }
        }
        _ => ExifColorSpace::Unspecified,
    }
}

/// Decode an image, reading its ICC/EXIF colour space in the same pass.
/// The image is *not* converted here; callers decide when to convert.
pub fn decode(path: &Path) -> std::result::Result<DecodedImage, ImageError> {
    decode_inner(path, None)
}

/// Same as [`decode`], but uses an EXIF hint already parsed by the scanner so
/// untagged files do not need a second EXIF parse.
pub(crate) fn decode_with_hint(
    path: &Path,
    hint: ExifColorSpace,
) -> std::result::Result<DecodedImage, ImageError> {
    decode_inner(path, Some(hint))
}

fn decode_inner(
    path: &Path,
    hint: Option<ExifColorSpace>,
) -> std::result::Result<DecodedImage, ImageError> {
    let reader = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(ImageError::IoError)?;
    let mut decoder = reader.into_decoder()?;
    let profile = profile_from_decoder(&mut decoder, path, hint);
    let image = DynamicImage::from_decoder(decoder)?;
    Ok(DecodedImage { image, profile })
}

/// Inspect a source file without decoding pixels. Returns `Srgb` when the
/// profile cannot be read (ffmpeg-only formats, damaged headers, ...).
pub fn source_profile(path: &Path) -> ColorProfile {
    match open_decoder(path) {
        Ok(mut decoder) => profile_from_decoder(&mut decoder, path, None),
        Err(error) => {
            log::debug!("color profile {}: {error}", path.display());
            ColorProfile::Srgb
        }
    }
}

/// Inspect a source file with a scanner-provided EXIF hint. Used by the ffmpeg
/// thumbnail prefetch, where `media::read_exif` has already parsed EXIF.
pub fn source_profile_with_hint(path: &Path, hint: ExifColorSpace) -> ColorProfile {
    match open_decoder(path) {
        Ok(mut decoder) => match decoder.icc_profile() {
            Ok(Some(bytes)) => classify_icc_profile(bytes, hint),
            Ok(None) => profile_from_exif(hint),
            Err(error) => {
                log::warn!("read ICC {}: {error}; using EXIF hint", path.display());
                profile_from_exif(hint)
            }
        },
        Err(error) => {
            log::debug!("color profile {}: {error}; using EXIF hint", path.display());
            profile_from_exif(hint)
        }
    }
}

fn open_decoder(
    path: &Path,
) -> std::result::Result<impl ImageDecoder + '_, ImageError> {
    let reader = ImageReader::open(path)
        .and_then(|reader| reader.with_guessed_format())
        .map_err(ImageError::IoError)?;
    reader.into_decoder()
}

fn profile_from_decoder(
    decoder: &mut (impl ImageDecoder + ?Sized),
    path: &Path,
    hint: Option<ExifColorSpace>,
) -> ColorProfile {
    let icc = match decoder.icc_profile() {
        Ok(icc) => icc,
        Err(error) => {
            log::warn!("read ICC {}: {error}", path.display());
            None
        }
    };
    if let Some(bytes) = icc {
        return classify_icc_profile(bytes, hint.unwrap_or_default());
    }
    let hint = hint.unwrap_or_else(|| {
        decoder
            .exif_metadata()
            .ok()
            .flatten()
            .map(|raw| exif_hint_from_raw(&raw))
            .unwrap_or_default()
    });
    profile_from_exif(hint)
}

fn profile_from_exif(hint: ExifColorSpace) -> ColorProfile {
    match hint {
        ExifColorSpace::AdobeRgb => ColorProfile::AdobeRgb,
        ExifColorSpace::Srgb | ExifColorSpace::Unspecified => ColorProfile::Srgb,
    }
}

fn exif_hint_from_raw(raw: &[u8]) -> ExifColorSpace {
    let data = raw.strip_prefix(b"Exif\0\0").unwrap_or(raw);
    match exif::Reader::new().read_raw(data.to_vec()) {
        Ok(exif) => exif_color_space(&exif),
        Err(error) => {
            log::debug!("parse EXIF color space: {error}");
            ExifColorSpace::Unspecified
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IccClass {
    Srgb,
    Other,
    Invalid,
}

fn icc_classes() -> &'static Mutex<HashMap<[u8; 32], IccClass>> {
    static CLASSES: OnceLock<Mutex<HashMap<[u8; 32], IccClass>>> = OnceLock::new();
    CLASSES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn classify_icc(bytes: &[u8]) -> IccClass {
    let key = *blake3::hash(bytes).as_bytes();
    if let Some(class) = icc_classes().lock().unwrap().get(&key) {
        return *class;
    }
    let class = match Profile::new_icc(bytes) {
        Ok(profile) => {
            if profile_is_effectively_srgb(&profile) {
                IccClass::Srgb
            } else {
                IccClass::Other
            }
        }
        Err(error) => {
            log::debug!("parse ICC profile: {error}");
            IccClass::Invalid
        }
    };
    let mut classes = icc_classes().lock().unwrap();
    // The number of distinct profiles on disk is small; the cap only guards
    // against a pathological library full of unique broken profiles.
    if classes.len() > 256 {
        classes.clear();
    }
    classes.insert(key, class);
    class
}

fn classify_icc_profile(bytes: Vec<u8>, hint: ExifColorSpace) -> ColorProfile {
    match classify_icc(&bytes) {
        IccClass::Srgb => ColorProfile::Srgb,
        IccClass::Other => ColorProfile::Icc(bytes),
        IccClass::Invalid => {
            log::warn!(
                "invalid ICC profile ({} bytes); colour management skipped{}",
                bytes.len(),
                if hint == ExifColorSpace::AdobeRgb {
                    " (EXIF says Adobe RGB, but its ICC is unusable)"
                } else {
                    ""
                }
            );
            profile_from_exif(hint)
        }
    }
}

/// A profile is treated as sRGB when mapping its colors to sRGB is a no-op
/// (within 8-bit rounding). This catches sRGB profiles with slightly different
/// tags and avoids a full conversion for them.
fn profile_is_effectively_srgb(profile: &Profile) -> bool {
    if profile.color_space() != ColorSpaceSignature::RgbData {
        return false;
    }
    let destination = Profile::new_srgb();
    let Ok(transform) = build_transform(profile, PixelFormat::RGB_8, &destination, PixelFormat::RGB_8)
    else {
        return false;
    };
    let source = [
        0u8, 0, 0, 255, 255, 255, 255, 0, 0, 0, 255, 0, 0, 0, 255, 64, 64, 64, 128, 128, 128, 192,
        192, 192, 12, 200, 45, 245, 10, 130,
    ];
    let mut output = source;
    transform.transform_pixels(&source, &mut output);
    source
        .iter()
        .zip(output.iter())
        .all(|(input, output)| input.abs_diff(*output) <= 2)
}

fn build_transform(
    source: &Profile,
    input_format: PixelFormat,
    destination: &Profile,
    output_format: PixelFormat,
) -> lcms2::LCMSResult<Transform<u8, u8>> {
    Transform::new(
        source,
        input_format,
        destination,
        output_format,
        Intent::Perceptual,
    )
    .or_else(|_| {
        Transform::new(
            source,
            input_format,
            destination,
            output_format,
            Intent::RelativeColorimetric,
        )
    })
}

fn source_lcms_profile(profile: &ColorProfile) -> Result<Profile> {
    match profile {
        ColorProfile::Srgb => Ok(Profile::new_srgb()),
        ColorProfile::Icc(bytes) => {
            Profile::new_icc(bytes).context("parse embedded ICC profile")
        }
        ColorProfile::AdobeRgb => adobe_rgb_profile(),
    }
}

/// Adobe RGB (1998) primaries, used only for EXIF-only Adobe RGB files that do
/// not embed an ICC profile.
fn adobe_rgb_profile() -> Result<Profile> {
    let white = CIExyY {
        x: 0.3127,
        y: 0.3290,
        Y: 1.0,
    };
    let primaries = CIExyYTRIPLE {
        Red: CIExyY {
            x: 0.6400,
            y: 0.3300,
            Y: 1.0,
        },
        Green: CIExyY {
            x: 0.2100,
            y: 0.7100,
            Y: 1.0,
        },
        Blue: CIExyY {
            x: 0.1500,
            y: 0.0600,
            Y: 1.0,
        },
    };
    let curve = ToneCurve::new(563.0 / 256.0);
    Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve])
        .context("build Adobe RGB profile")
}

/// Convert a decoded image to sRGB. Never fails: a bad profile only logs and
/// returns the original pixels.
pub fn convert_to_srgb(image: DynamicImage, profile: &ColorProfile) -> DynamicImage {
    if !profile.needs_conversion() {
        return image;
    }
    match convert_image(&image, profile) {
        Ok(converted) => converted,
        Err(error) => {
            log::warn!(
                "color conversion failed ({}): {error:#}; keeping source pixels",
                profile.label()
            );
            image
        }
    }
}

/// Thumbnail-sized RGB conversion. The caller already flattened alpha.
pub fn convert_rgb8(image: RgbImage, profile: &ColorProfile) -> RgbImage {
    if !profile.needs_conversion() {
        return image;
    }
    match convert_image(&DynamicImage::ImageRgb8(image.clone()), profile) {
        Ok(DynamicImage::ImageRgb8(converted)) => converted,
        Ok(other) => other.to_rgb8(),
        Err(error) => {
            log::warn!(
                "thumbnail color conversion failed ({}): {error:#}; keeping source pixels",
                profile.label()
            );
            image
        }
    }
}

fn convert_image(image: &DynamicImage, profile: &ColorProfile) -> Result<DynamicImage> {
    let source = source_lcms_profile(profile)?;
    let destination = Profile::new_srgb();
    let color_space = source.color_space();
    if color_space == ColorSpaceSignature::GrayData {
        // Gray ICC: transform one channel and produce RGB. Alpha, if any, is
        // dropped; gray+alpha ICC images are rare and colour is more important.
        let luma = image.to_luma8();
        let mut converted = vec![0u8; luma.len() * 3];
        let transform = build_transform(
            &source,
            PixelFormat::GRAY_8,
            &destination,
            PixelFormat::RGB_8,
        )?;
        transform.transform_pixels(luma.as_raw(), &mut converted);
        return RgbImage::from_raw(luma.width(), luma.height(), converted)
            .map(DynamicImage::ImageRgb8)
            .context("gray ICC conversion produced a bad buffer");
    }
    if color_space != ColorSpaceSignature::RgbData {
        bail!("unsupported ICC colour space {color_space:?}");
    }
    if image.color().has_alpha() {
        let rgba = image.to_rgba8();
        let mut converted = vec![0u8; rgba.len()];
        let transform = build_transform(
            &source,
            PixelFormat::RGBA_8,
            &destination,
            PixelFormat::RGBA_8,
        )?;
        transform.transform_pixels(rgba.as_raw(), &mut converted);
        return RgbaImage::from_raw(rgba.width(), rgba.height(), converted)
            .map(DynamicImage::ImageRgba8)
            .context("RGBA ICC conversion produced a bad buffer");
    }
    let rgb = image.to_rgb8();
    let mut converted = vec![0u8; rgb.len()];
    let transform = build_transform(
        &source,
        PixelFormat::RGB_8,
        &destination,
        PixelFormat::RGB_8,
    )?;
    transform.transform_pixels(rgb.as_raw(), &mut converted);
    RgbImage::from_raw(rgb.width(), rgb.height(), converted)
        .map(DynamicImage::ImageRgb8)
        .context("RGB ICC conversion produced a bad buffer")
}

/// Convert an existing thumbnail file in place. Used after the ffmpeg batch
/// creates a 512 JPEG. Returns `false` when the profile is already sRGB.
pub fn convert_file_to_srgb(target: &Path, profile: &ColorProfile) -> Result<bool> {
    if !profile.needs_conversion() {
        return Ok(false);
    }
    let image = ImageReader::open(target)
        .with_context(|| format!("open thumbnail {}", target.display()))?
        .with_guessed_format()
        .with_context(|| format!("guess thumbnail format {}", target.display()))?
        .decode()
        .with_context(|| format!("decode thumbnail {}", target.display()))?;
    let converted = convert_to_srgb(image, profile).to_rgb8();
    let temp = target.with_extension("color.jpg");
    // Match the image-crate thumbnail path (default JPEG quality). Going through
    // `DynamicImage` also instantiates the encoder in the optimised image crate
    // instead of this crate's debug profile.
    DynamicImage::ImageRgb8(converted)
        .save_with_format(&temp, ImageFormat::Jpeg)
        .with_context(|| format!("write {}", temp.display()))?;
    let _ = std::fs::remove_file(target);
    std::fs::rename(&temp, target)
        .with_context(|| format!("replace {} with {}", target.display(), temp.display()))?;
    Ok(true)
}

/// Deterministic preview path for one source revision. The preview is
/// ephemeral (viewer lifetime), not counted by the thumbnail budget.
pub fn preview_target(source: &Path, cache_dir: &Path, mtime: u64, bytes: u64) -> PathBuf {
    let key = format!(
        "{PREVIEW_CACHE_VERSION}-{}-{mtime}-{bytes}",
        source.to_string_lossy()
    );
    let hash = blake3::hash(key.as_bytes()).to_hex();
    cache_dir
        .join("previews")
        .join(format!("{}.png", &hash.as_str()[..24]))
}

/// Check whether an sRGB preview is needed and, if so, write one. Files that
/// are already sRGB return [`PreviewOutcome::Unchanged`] without decoding the
/// full image; the viewer can display the original directly in that case.
pub fn prepare_preview(source: &Path, target: &Path) -> Result<PreviewOutcome> {
    let profile = source_profile(source);
    if !profile.needs_conversion() {
        return Ok(PreviewOutcome::Unchanged);
    }
    let decoded = decode(source)
        .with_context(|| format!("decode {} for color preview", source.display()))?;
    if !decoded.profile.needs_conversion() {
        return Ok(PreviewOutcome::Unchanged);
    }
    let converted = convert_to_srgb(decoded.image, &decoded.profile);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let temp = target.with_extension("png.tmp");
    converted
        .save_with_format(&temp, ImageFormat::Png)
        .with_context(|| format!("write {}", temp.display()))?;
    let _ = std::fs::remove_file(target);
    std::fs::rename(&temp, target)
        .with_context(|| format!("replace {} with {}", target.display(), temp.display()))?;
    Ok(PreviewOutcome::Converted)
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::fs::File;
    use std::path::Path;

    use image::{ExtendedColorType, ImageEncoder, RgbImage};
    use lcms2::{CIExyY, CIExyYTRIPLE, Profile, ToneCurve};

    /// P3-like display profile generated at test time (no third-party asset).
    pub(crate) fn display_p3_icc() -> Vec<u8> {
        let white = CIExyY {
            x: 0.3127,
            y: 0.3290,
            Y: 1.0,
        };
        let primaries = CIExyYTRIPLE {
            Red: CIExyY {
                x: 0.680,
                y: 0.320,
                Y: 1.0,
            },
            Green: CIExyY {
                x: 0.265,
                y: 0.690,
                Y: 1.0,
            },
            Blue: CIExyY {
                x: 0.150,
                y: 0.060,
                Y: 1.0,
            },
        };
        let curve = ToneCurve::new(2.2);
        Profile::new_rgb(&white, &primaries, &[&curve, &curve, &curve])
            .expect("build P3-like profile")
            .icc()
            .expect("serialize P3-like profile")
    }

    pub(crate) fn srgb_icc() -> Vec<u8> {
        Profile::new_srgb().icc().expect("serialize sRGB profile")
    }

    pub(crate) fn write_png_with_icc(path: &Path, image: &RgbImage, icc: Option<&[u8]>) {
        let file = File::create(path).expect("create png");
        let mut encoder = image::codecs::png::PngEncoder::new(file);
        if let Some(icc) = icc {
            encoder.set_icc_profile(icc.to_vec()).expect("embed ICC");
        }
        encoder
            .write_image(
                image.as_raw(),
                image.width(),
                image.height(),
                ExtendedColorType::Rgb8,
            )
            .expect("write png");
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use image::{Rgb, RgbImage};
    use lcms2::Profile;

    use super::{ColorProfile, PreviewOutcome, decode, prepare_preview, source_profile};
    use super::test_support::{display_p3_icc, srgb_icc, write_png_with_icc};

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kanalib-color-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn solid(width: u32, height: u32, color: [u8; 3]) -> RgbImage {
        RgbImage::from_pixel(width, height, Rgb(color))
    }

    fn sample_path(dir: &Path, name: &str) -> PathBuf {
        dir.join(name)
    }

    fn read_rgb(path: &Path) -> RgbImage {
        image::ImageReader::open(path)
            .unwrap()
            .with_guessed_format()
            .unwrap()
            .decode()
            .unwrap()
            .to_rgb8()
    }

    fn max_delta(a: &RgbImage, b: &RgbImage) -> u8 {
        assert_eq!(a.dimensions(), b.dimensions());
        a.pixels()
            .zip(b.pixels())
            .map(|(a, b)| {
                (0..3)
                    .map(|index| a[index].abs_diff(b[index]))
                    .max()
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn p3_like_profile_is_detected_as_non_srgb() {
        let dir = temp_dir("p3-detect");
        let path = sample_path(&dir, "p3.png");
        let palette = RgbImage::from_fn(10, 1, |x, _| {
            Rgb(match x {
                0 => [255, 0, 0],
                1 => [0, 255, 0],
                2 => [0, 0, 255],
                3 => [0, 128, 0],
                4 => [255, 128, 0],
                5 => [0, 200, 200],
                6 => [128, 0, 128],
                7 => [255, 255, 0],
                8 => [30, 180, 90],
                _ => [200, 60, 20],
            })
        });
        write_png_with_icc(&path, &palette, Some(&display_p3_icc()));
        assert!(matches!(source_profile(&path), ColorProfile::Icc(_)));
        let decoded = decode(&path).unwrap();
        assert!(decoded.profile.needs_conversion());
        let converted = super::convert_to_srgb(decoded.image.clone(), &decoded.profile).to_rgb8();
        let source = decoded.image.to_rgb8();
        assert!(
            max_delta(&source, &converted) >= 10,
            "P3 colors must move when converted to sRGB"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn srgb_icc_and_untagged_files_keep_exact_pixels() {
        let dir = temp_dir("srgb-passthrough");
        for (name, icc) in [("srgb.png", Some(srgb_icc())), ("plain.png", None)] {
            let path = sample_path(&dir, name);
            write_png_with_icc(&path, &solid(32, 24, [40, 160, 220]), icc.as_deref());
            assert_eq!(source_profile(&path), ColorProfile::Srgb);
            let decoded = decode(&path).unwrap();
            assert_eq!(decoded.profile, ColorProfile::Srgb);
            let converted = super::convert_to_srgb(decoded.image.clone(), &decoded.profile);
            assert_eq!(
                decoded.image.to_rgb8().as_raw(),
                converted.to_rgb8().as_raw(),
                "{name} must keep its bytes"
            );
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn invalid_icc_degrades_without_failing_decode() {
        let dir = temp_dir("invalid-icc");
        let path = sample_path(&dir, "bad-icc.png");
        write_png_with_icc(&path, &solid(16, 16, [12, 34, 56]), Some(b"not an ICC profile"));
        assert_eq!(source_profile(&path), ColorProfile::Srgb);
        let decoded = decode(&path).expect("invalid ICC must not fail decoding");
        assert_eq!(decoded.profile, ColorProfile::Srgb);
        assert_eq!(decoded.image.to_rgb8().get_pixel(0, 0).0, [12, 34, 56]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn srgb_like_profiles_are_recognised_by_identity_probe() {
        let icc = srgb_icc();
        let profile = Profile::new_icc(&icc).unwrap();
        assert!(super::profile_is_effectively_srgb(&profile));
    }

    #[test]
    fn exif_color_space_hints_are_parsed() {
        // Minimal TIFF IFD with ColorSpace=2 (Adobe RGB).
        let mut tiff = Vec::new();
        tiff.extend(b"II*\0");
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(1u16.to_le_bytes());
        tiff.extend(0xA001u16.to_le_bytes());
        tiff.extend(3u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(2u16.to_le_bytes());
        tiff.extend([0u8, 0u8]);
        tiff.extend(0u32.to_le_bytes());
        let exif = exif::Reader::new().read_raw(tiff.clone()).unwrap();
        assert_eq!(
            super::exif_color_space(&exif),
            super::ExifColorSpace::AdobeRgb
        );

        // The image crate may hand over EXIF with the JPEG "Exif\0\0" prefix.
        let mut prefixed = b"Exif\0\0".to_vec();
        prefixed.extend(tiff);
        assert_eq!(
            super::exif_hint_from_raw(&prefixed),
            super::ExifColorSpace::AdobeRgb
        );
    }

    #[test]
    fn jpeg_exif_color_space_is_read_by_the_decoder() {
        let dir = temp_dir("exif-jpeg");
        let path = dir.join("adobe.jpg");
        solid(16, 16, [20, 40, 60]).save(&path).unwrap();
        let jpeg = std::fs::read(&path).unwrap();
        let mut tiff = Vec::new();
        tiff.extend(b"II*\0");
        tiff.extend(8u32.to_le_bytes());
        tiff.extend(1u16.to_le_bytes());
        tiff.extend(0xA001u16.to_le_bytes());
        tiff.extend(3u16.to_le_bytes());
        tiff.extend(1u32.to_le_bytes());
        tiff.extend(2u16.to_le_bytes());
        tiff.extend([0u8, 0u8]);
        tiff.extend(0u32.to_le_bytes());
        let payload = [b"Exif\0\0".as_slice(), tiff.as_slice()].concat();
        let mut app1 = vec![0xFF, 0xE1];
        app1.extend(((payload.len() + 2) as u16).to_be_bytes());
        app1.extend(payload);
        let mut output = Vec::with_capacity(jpeg.len() + app1.len());
        output.extend(&jpeg[..2]);
        output.extend(app1);
        output.extend(&jpeg[2..]);
        std::fs::write(&path, output).unwrap();

        let decoded = decode(&path).unwrap();
        assert_eq!(
            decoded.profile,
            ColorProfile::AdobeRgb,
            "EXIF-only Adobe RGB must survive the decoder metadata path"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn preview_for_p3_is_converted_and_matches_the_source_colour_shift() {
        let dir = temp_dir("preview");
        let source = dir.join("p3.png");
        write_png_with_icc(
            &source,
            &solid(64, 48, [200, 60, 20]),
            Some(&display_p3_icc()),
        );
        let target = super::preview_target(&source, &dir.join("thumbs"), 123, 456);
        match prepare_preview(&source, &target).unwrap() {
            PreviewOutcome::Converted => {}
            other => panic!("expected converted preview, got {other:?}"),
        }
        let preview = read_rgb(&target);
        let original = read_rgb(&source);
        assert!(max_delta(&original, &preview) >= 10);
        assert!(target.starts_with(dir.join("thumbs").join("previews")));

        let plain = dir.join("plain.png");
        write_png_with_icc(&plain, &solid(8, 8, [1, 2, 3]), None);
        let plain_target = super::preview_target(&plain, &dir.join("thumbs"), 1, 2);
        assert_eq!(
            prepare_preview(&plain, &plain_target).unwrap(),
            PreviewOutcome::Unchanged
        );
        assert!(!plain_target.exists(), "untagged files need no preview file");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
