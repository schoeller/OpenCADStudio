// Device-independent bitmap (BITMAPINFO + pixel bits) → RGBA.
//
// Metafile blit records carry a headerless DIB: a BITMAPINFOHEADER, an
// optional palette / bitfield masks, then the pixel data. Rather than decode
// the many bpp/compression variants by hand, synthesize the 14-byte
// BITMAPFILEHEADER in front and let the `image` crate's BMP decoder do it.

/// Decode a metafile-embedded DIB. `bmi` is the BITMAPINFO (header +
/// palette), `bits` the pixel data. Returns RGBA plus dimensions.
pub fn decode(bmi: &[u8], bits: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if bmi.len() < 12 {
        return None;
    }
    let file_size = 14 + bmi.len() + bits.len();
    let pixel_offset = 14 + bmi.len();
    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes());
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    bmp.extend_from_slice(bmi);
    bmp.extend_from_slice(bits);
    let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    Some((rgba.into_raw(), w, h))
}

/// Decode a *packed* DIB (BITMAPINFO immediately followed by the bits, no
/// explicit split) — the layout WMF blit records embed. The pixel-data offset
/// is derived from the header size, palette length, and bitfield masks.
pub fn decode_packed(dibdata: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    if dibdata.len() < 12 {
        return None;
    }
    let bi_size = u32::from_le_bytes(dibdata[0..4].try_into().ok()?) as usize;
    if bi_size < 12 || bi_size > dibdata.len() {
        return None;
    }
    let (bpp, compression, clr_used) = if bi_size == 12 {
        // BITMAPCOREHEADER: u16 bitcount at 10, palette entries are 3 bytes.
        let bpp = u16::from_le_bytes(dibdata[10..12].try_into().ok()?) as usize;
        (bpp, 0usize, 0usize)
    } else {
        let bpp = u16::from_le_bytes(dibdata[14..16].try_into().ok()?) as usize;
        let comp = u32::from_le_bytes(dibdata[16..20].try_into().ok()?) as usize;
        let used = u32::from_le_bytes(dibdata[32..36].try_into().ok()?) as usize;
        (bpp, comp, used)
    };
    let pal_entry = if bi_size == 12 { 3 } else { 4 };
    let pal_count = if bpp <= 8 {
        if clr_used > 0 {
            clr_used.min(1 << bpp)
        } else {
            1 << bpp
        }
    } else {
        clr_used
    };
    // BI_BITFIELDS (3) on a 40-byte header carries three u32 masks.
    let masks = if compression == 3 && bi_size == 40 {
        12
    } else {
        0
    };
    let offset = bi_size + masks + pal_count * pal_entry;
    if offset > dibdata.len() {
        return None;
    }
    decode(&dibdata[..offset], &dibdata[offset..])
}

/// Split a record's trailing `BITMAPINFO + bits` region where the two ranges
/// are given as offsets from the record start. Returns `(bmi, bits)` slices,
/// or `None` when either range falls outside the record.
pub fn ranges<'a>(
    record: &'a [u8],
    off_bmi: usize,
    cb_bmi: usize,
    off_bits: usize,
    cb_bits: usize,
) -> Option<(&'a [u8], &'a [u8])> {
    let bmi = record.get(off_bmi..off_bmi.checked_add(cb_bmi)?)?;
    let bits = record.get(off_bits..off_bits.checked_add(cb_bits)?)?;
    Some((bmi, bits))
}
