//! Centralized file upload validation: filename sanitization, extension /
//! MIME allowlisting, and size limits.
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileUploadError {
    EmptyFilename,
    PathTraversal,
    DisallowedExtension(String),
    DisallowedContentType(String),
    ExtensionContentTypeMismatch,
    TooLarge { size: usize, max: usize },
    /// The bytes do not match any allowed image format's magic signature.
    UnrecognizedImageFormat,
    /// The declared content type does not match the detected image format.
    ContentTypeSignatureMismatch { declared: String, detected: String },
    /// The image header is truncated or otherwise malformed.
    MalformedImage,
    /// The image's pixel dimensions exceed the configured bounds.
    DimensionsTooLarge { width: u32, height: u32, max_width: u32, max_height: u32 },
    /// The image carries embedded metadata that must be stripped before use.
    MetadataPresent(&'static str),
}

impl fmt::Display for FileUploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFilename => write!(f, "filename must not be empty"),
            Self::PathTraversal => write!(f, "filename contains path traversal characters"),
            Self::DisallowedExtension(ext) => write!(f, "file extension '{ext}' is not allowed"),
            Self::DisallowedContentType(ct) => write!(f, "content type '{ct}' is not allowed"),
            Self::ExtensionContentTypeMismatch => {
                write!(f, "file extension does not match declared content type")
            }
            Self::TooLarge { size, max } => {
                write!(f, "file size {size} bytes exceeds maximum of {max} bytes")
            }
            Self::UnrecognizedImageFormat => {
                write!(f, "file contents do not match a supported image format")
            }
            Self::ContentTypeSignatureMismatch { declared, detected } => write!(
                f,
                "declared content type '{declared}' does not match detected format '{detected}'"
            ),
            Self::MalformedImage => write!(f, "image header is malformed or truncated"),
            Self::DimensionsTooLarge {
                width,
                height,
                max_width,
                max_height,
            } => write!(
                f,
                "image dimensions {width}x{height} exceed maximum of {max_width}x{max_height}"
            ),
            Self::MetadataPresent(kind) => {
                write!(f, "image contains embedded {kind} metadata that must be stripped")
            }
        }
    }
}

impl std::error::Error for FileUploadError {}

/// Upload constraints for a given endpoint (avatar upload, match evidence,
/// etc). Construct with [`FileUploadRules::images`] for the common case, or
/// build directly for a custom allowlist.
pub struct FileUploadRules {
    pub allowed_extensions: &'static [&'static str],
    pub allowed_content_types: &'static [&'static str],
    pub max_size_bytes: usize,
}

impl FileUploadRules {
    /// Common image-upload policy: jpg/jpeg/png/webp, 5 MiB max.
    pub fn images() -> Self {
        Self {
            allowed_extensions: &["jpg", "jpeg", "png", "webp"],
            allowed_content_types: &["image/jpeg", "image/png", "image/webp"],
            max_size_bytes: 5 * 1024 * 1024,
        }
    }
}

/// Pixel-dimension bounds for a decoded image. Derivatives are generated
/// from the original, so the original must stay within a bounded size to
/// avoid decompression bombs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageDimensionLimits {
    pub max_width: u32,
    pub max_height: u32,
}

impl ImageDimensionLimits {
    /// Avatar policy: at most 4096x4096 source pixels.
    pub fn avatars() -> Self {
        Self {
            max_width: 4096,
            max_height: 4096,
        }
    }
}

/// Image formats we can verify by magic bytes and decode for derivatives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
}

impl ImageFormat {
    pub fn content_type(self) -> &'static str {
        match self {
            Self::Jpeg => "image/jpeg",
            Self::Png => "image/png",
            Self::Webp => "image/webp",
        }
    }
}

/// Detect the image format from the leading magic bytes. Returns `None` if
/// the payload does not start with a recognized signature.
pub fn detect_image_format(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(ImageFormat::Jpeg);
    }
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some(ImageFormat::Png);
    }
    if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        return Some(ImageFormat::Webp);
    }
    None
}

/// Read the pixel dimensions from an image header without decoding pixels.
/// Only the header is inspected, so this is cheap and safe against
/// decompression bombs (callers still enforce [`ImageDimensionLimits`]).
pub fn read_image_dimensions(bytes: &[u8], format: ImageFormat) -> Result<(u32, u32), FileUploadError> {
    match format {
        ImageFormat::Png => read_png_dimensions(bytes),
        ImageFormat::Jpeg => read_jpeg_dimensions(bytes),
        ImageFormat::Webp => read_webp_dimensions(bytes),
    }
}

fn read_png_dimensions(bytes: &[u8]) -> Result<(u32, u32), FileUploadError> {
    // IHDR width/height are big-endian u32 at offsets 16..24.
    if bytes.len() < 24 {
        return Err(FileUploadError::MalformedImage);
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 {
        return Err(FileUploadError::MalformedImage);
    }
    Ok((width, height))
}

fn read_jpeg_dimensions(bytes: &[u8]) -> Result<(u32, u32), FileUploadError> {
    let mut i = 2usize; // skip SOI (FF D8)
    while i + 1 < bytes.len() {
        if bytes[i] != 0xFF {
            return Err(FileUploadError::MalformedImage);
        }
        let marker = bytes[i + 1];
        // Standalone markers carry no length payload.
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        if i + 3 >= bytes.len() {
            return Err(FileUploadError::MalformedImage);
        }
        let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        if seg_len < 2 {
            return Err(FileUploadError::MalformedImage);
        }
        // SOF0..SOF15 except DHT (C4), JPG (C8), DAC (CC) carry dimensions.
        let is_sof = (0xC0..=0xCF).contains(&marker)
            && marker != 0xC4
            && marker != 0xC8
            && marker != 0xCC;
        if is_sof {
            if i + 9 >= bytes.len() {
                return Err(FileUploadError::MalformedImage);
            }
            let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            if width == 0 || height == 0 {
                return Err(FileUploadError::MalformedImage);
            }
            return Ok((width, height));
        }
        i += 2 + seg_len;
    }
    Err(FileUploadError::MalformedImage)
}

fn read_webp_dimensions(bytes: &[u8]) -> Result<(u32, u32), FileUploadError> {
    if bytes.len() < 30 {
        return Err(FileUploadError::MalformedImage);
    }
    let chunk = &bytes[12..16];
    match chunk {
        b"VP8 " => {
            // Lossy: dimensions at 26..30 as 14-bit little-endian values.
            let width = (u16::from_le_bytes([bytes[26], bytes[27]]) & 0x3FFF) as u32;
            let height = (u16::from_le_bytes([bytes[28], bytes[29]]) & 0x3FFF) as u32;
            if width == 0 || height == 0 {
                return Err(FileUploadError::MalformedImage);
            }
            Ok((width, height))
        }
        b"VP8L" => {
            // Lossless: 14-bit dimensions packed after the 0x2F signature.
            if bytes.len() < 25 || bytes[20] != 0x2F {
                return Err(FileUploadError::MalformedImage);
            }
            let bits = u32::from_le_bytes([bytes[21], bytes[22], bytes[23], bytes[24]]);
            let width = (bits & 0x3FFF) + 1;
            let height = ((bits >> 14) & 0x3FFF) + 1;
            Ok((width, height))
        }
        b"VP8X" => {
            // Extended: 24-bit little-endian canvas dimensions minus one.
            if bytes.len() < 30 {
                return Err(FileUploadError::MalformedImage);
            }
            let width = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]) + 1;
            let height = u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0]) + 1;
            Ok((width, height))
        }
        _ => Err(FileUploadError::MalformedImage),
    }
}

/// Detect embedded metadata that must be stripped before the image is
/// persisted or served. Returns the offending metadata kind, if any.
pub fn detect_embedded_metadata(bytes: &[u8], format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Jpeg => {
            // APP1 (EXIF / XMP) and APP13 (IPTC / Photoshop) segments.
            let mut i = 2usize;
            while i + 3 < bytes.len() {
                if bytes[i] != 0xFF {
                    return None;
                }
                let marker = bytes[i + 1];
                if marker == 0xDA {
                    // Start of scan: metadata segments precede it.
                    return None;
                }
                if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
                    i += 2;
                    continue;
                }
                let seg_len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
                if seg_len < 2 {
                    return None;
                }
                if marker == 0xE1 {
                    return Some("EXIF/XMP");
                }
                if marker == 0xED {
                    return Some("IPTC");
                }
                i += 2 + seg_len;
            }
            None
        }
        ImageFormat::Png => {
            // tEXt / iTXt / zTXt / eXIf ancillary chunks.
            let mut i = 8usize;
            while i + 8 <= bytes.len() {
                let len = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
                    as usize;
                let kind = &bytes[i + 4..i + 8];
                if matches!(kind, b"tEXt" | b"iTXt" | b"zTXt" | b"eXIf") {
                    return Some("PNG text/EXIF");
                }
                if kind == b"IEND" {
                    return None;
                }
                i = i.checked_add(12 + len)?;
            }
            None
        }
        ImageFormat::Webp => {
            // EXIF / XMP chunks inside the RIFF container.
            let mut i = 12usize;
            while i + 8 <= bytes.len() {
                let kind = &bytes[i..i + 4];
                let len = u32::from_le_bytes([bytes[i + 4], bytes[i + 5], bytes[i + 6], bytes[i + 7]])
                    as usize;
                if kind == b"EXIF" || kind == b"XMP " {
                    return Some("WebP EXIF/XMP");
                }
                i = i.checked_add(8 + len + (len & 1))?;
            }
            None
        }
    }
}

/// Full validation for an image upload: metadata checks (name/type/size),
/// magic-byte verification, dimension bounds, and metadata-stripping
/// requirements. Returns the sanitized filename and detected format.
///
/// Callers must strip embedded metadata (e.g. by re-encoding through the
/// image pipeline) before persisting; this function rejects payloads that
/// still carry metadata so a failed strip can never leak through.
pub fn validate_image_upload(
    filename: &str,
    content_type: &str,
    bytes: &[u8],
    rules: &FileUploadRules,
    limits: &ImageDimensionLimits,
) -> Result<(String, ImageFormat), FileUploadError> {
    let sanitized = validate_file_upload(filename, content_type, bytes.len(), rules)?;

    let format = detect_image_format(bytes).ok_or(FileUploadError::UnrecognizedImageFormat)?;

    let declared = content_type.trim().to_ascii_lowercase();
    if declared != format.content_type() {
        return Err(FileUploadError::ContentTypeSignatureMismatch {
            declared,
            detected: format.content_type().to_string(),
        });
    }

    let (width, height) = read_image_dimensions(bytes, format)?;
    if width > limits.max_width || height > limits.max_height {
        return Err(FileUploadError::DimensionsTooLarge {
            width,
            height,
            max_width: limits.max_width,
            max_height: limits.max_height,
        });
    }

    if let Some(kind) = detect_embedded_metadata(bytes, format) {
        return Err(FileUploadError::MetadataPresent(kind));
    }

    Ok((sanitized, format))
}

/// Sanitize an untrusted filename: strip any directory components and
/// reject characters outside a conservative safe set, so the result is
/// always safe to use as a bare filename (never re-parented via `..`, never
/// containing null bytes or path separators).
pub fn sanitize_filename(input: &str) -> Result<String, FileUploadError> {
    if input.trim().is_empty() {
        return Err(FileUploadError::EmptyFilename);
    }

    // Take only the final path component, defeating `../../etc/passwd`
    // style traversal regardless of the separator style supplied.
    let base = input
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(input)
        .trim();

    if base.is_empty() || base == "." || base == ".." {
        return Err(FileUploadError::PathTraversal);
    }

    let sanitized: String = base
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.contains("..") {
        return Err(FileUploadError::PathTraversal);
    }

    Ok(sanitized)
}

fn extension_of(filename: &str) -> Option<String> {
    filename
        .rsplit('.')
        .next()
        .filter(|ext| *ext != filename)
        .map(|ext| ext.to_ascii_lowercase())
}

/// Validate an uploaded file's name, declared content type, and size
/// against `rules`. Returns the sanitized filename on success.
///
/// This checks metadata only (name/type/size) — callers handling files
/// where content spoofing is a serious concern (executables disguised with
/// an image extension) should additionally verify the file's magic bytes
/// server-side before persisting it.
pub fn validate_file_upload(
    filename: &str,
    content_type: &str,
    size_bytes: usize,
    rules: &FileUploadRules,
) -> Result<String, FileUploadError> {
    let sanitized = sanitize_filename(filename)?;

    if size_bytes > rules.max_size_bytes {
        return Err(FileUploadError::TooLarge {
            size: size_bytes,
            max: rules.max_size_bytes,
        });
    }

    let content_type_normalized = content_type.trim().to_ascii_lowercase();
    if !rules
        .allowed_content_types
        .iter()
        .any(|ct| *ct == content_type_normalized)
    {
        return Err(FileUploadError::DisallowedContentType(content_type_normalized));
    }

    let ext = extension_of(&sanitized).ok_or_else(|| {
        FileUploadError::DisallowedExtension("(none)".to_string())
    })?;
    if !rules.allowed_extensions.iter().any(|e| *e == ext) {
        return Err(FileUploadError::DisallowedExtension(ext));
    }

    // Reject the classic `evil.php.jpg` double-extension trick: every
    // dot-separated segment before the final extension must itself look
    // like a plausible extension-free filename, i.e. we simply require
    // there be exactly one dot, matching a single trusted extension.
    if sanitized.matches('.').count() > 1 {
        return Err(FileUploadError::ExtensionContentTypeMismatch);
    }

    Ok(sanitized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_path_traversal() {
        assert_eq!(
            sanitize_filename("../../etc/passwd").unwrap(),
            "passwd"
        );
    }

    #[test]
    fn accepts_valid_image_upload() {
        let rules = FileUploadRules::images();
        let result = validate_file_upload("avatar.png", "image/png", 1024, &rules);
        assert_eq!(result.unwrap(), "avatar.png");
    }

    #[test]
    fn rejects_double_extension() {
        let rules = FileUploadRules::images();
        let result = validate_file_upload("evil.php.png", "image/png", 1024, &rules);
        assert!(result.is_err());
    }

    #[test]
    fn rejects_oversized_file() {
        let rules = FileUploadRules::images();
        let result = validate_file_upload("avatar.png", "image/png", 100 * 1024 * 1024, &rules);
        assert_eq!(
            result,
            Err(FileUploadError::TooLarge {
                size: 100 * 1024 * 1024,
                max: rules.max_size_bytes
            })
        );
    }

    /// Build a minimal PNG header with the given dimensions and optional
    /// trailing chunk (used to simulate metadata).
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        v.extend_from_slice(&13u32.to_be_bytes());
        v.extend_from_slice(b"IHDR");
        v.extend_from_slice(&width.to_be_bytes());
        v.extend_from_slice(&height.to_be_bytes());
        v.extend_from_slice(&[8, 6, 0, 0, 0]);
        v.extend_from_slice(&[0, 0, 0, 0]); // fake CRC
        v
    }

    fn png_with_text_chunk(width: u32, height: u32) -> Vec<u8> {
        let mut v = png_header(width, height);
        v.extend_from_slice(&4u32.to_be_bytes());
        v.extend_from_slice(b"tEXt");
        v.extend_from_slice(b"abcd");
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }

    #[test]
    fn detects_supported_formats() {
        assert_eq!(detect_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(ImageFormat::Jpeg));
        assert_eq!(detect_image_format(&png_header(1, 1)), Some(ImageFormat::Png));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(detect_image_format(&webp), Some(ImageFormat::Webp));
    }

    #[test]
    fn rejects_malformed_payload() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let garbage = b"not an image at all".to_vec();
        assert_eq!(
            validate_image_upload("avatar.png", "image/png", &garbage, &rules, &limits),
            Err(FileUploadError::UnrecognizedImageFormat)
        );
    }

    #[test]
    fn rejects_truncated_png_header() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let truncated = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0];
        assert_eq!(
            validate_image_upload("avatar.png", "image/png", &truncated, &rules, &limits),
            Err(FileUploadError::MalformedImage)
        );
    }

    #[test]
    fn rejects_content_type_signature_mismatch() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let png = png_header(64, 64);
        assert_eq!(
            validate_image_upload("avatar.jpg", "image/jpeg", &png, &rules, &limits),
            Err(FileUploadError::ContentTypeSignatureMismatch {
                declared: "image/jpeg".to_string(),
                detected: "image/png".to_string(),
            })
        );
    }

    #[test]
    fn rejects_oversized_dimensions() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let huge = png_header(10_000, 10_000);
        assert_eq!(
            validate_image_upload("avatar.png", "image/png", &huge, &rules, &limits),
            Err(FileUploadError::DimensionsTooLarge {
                width: 10_000,
                height: 10_000,
                max_width: 4096,
                max_height: 4096,
            })
        );
    }

    #[test]
    fn rejects_png_with_embedded_metadata() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let with_meta = png_with_text_chunk(64, 64);
        assert_eq!(
            validate_image_upload("avatar.png", "image/png", &with_meta, &rules, &limits),
            Err(FileUploadError::MetadataPresent("PNG text/EXIF"))
        );
    }

    #[test]
    fn accepts_clean_png() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let clean = png_header(256, 256);
        let (name, format) =
            validate_image_upload("avatar.png", "image/png", &clean, &rules, &limits).unwrap();
        assert_eq!(name, "avatar.png");
        assert_eq!(format, ImageFormat::Png);
    }

    #[test]
    fn rejects_jpeg_with_exif() {
        let rules = FileUploadRules::images();
        let limits = ImageDimensionLimits::avatars();
        let mut jpeg = vec![0xFF, 0xD8];
        // APP1 (EXIF) segment.
        jpeg.extend_from_slice(&[0xFF, 0xE1, 0x00, 0x08]);
        jpeg.extend_from_slice(b"Exif\0\0");
        assert_eq!(
            validate_image_upload("avatar.jpg", "image/jpeg", &jpeg, &rules, &limits),
            Err(FileUploadError::MetadataPresent("EXIF/XMP"))
        );
    }
}
