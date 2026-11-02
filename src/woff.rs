//! WOFF 1 to plain sfnt (TTF/OTF): the text system takes sfnt, icon themes ship `.woff` (Seti does).
use std::io::Read as _;

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The font as sfnt: a WOFF is unpacked, sfnt data is returned as is; `None` for anything else (WOFF2 needs Brotli: not supported).
pub fn to_sfnt(data: &[u8]) -> Option<Vec<u8>> {
    match data.get(..4)? {
        b"wOFF" => {}
        [0, 1, 0, 0] | b"OTTO" | b"true" => return Some(data.to_vec()),
        _ => return None,
    }
    let n = u16::from_be_bytes(data.get(12..14)?.try_into().ok()?) as usize;
    let mut tables = Vec::with_capacity(n);
    for i in 0..n {
        let e = 44 + i * 20;
        let (offset, comp, orig) = (be32(data, e + 4)? as usize, be32(data, e + 8)? as usize, be32(data, e + 12)? as usize);
        let raw = data.get(offset..offset.checked_add(comp)?)?;
        let body = if comp == orig {
            raw.to_vec()
        } else {
            let mut out = Vec::with_capacity(orig);
            flate2::read::ZlibDecoder::new(raw).take(orig as u64 + 1).read_to_end(&mut out).ok().filter(|_| out.len() == orig)?;
            out
        };
        tables.push((data.get(e..e + 4)?, be32(data, e + 16)?, body));
    }
    let mut out = Vec::new();
    out.extend_from_slice(data.get(4..8)?); // flavor
    let entry_selector = n.max(1).ilog2() as u16;
    let search_range = 16 * (1u16 << entry_selector);
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&((n as u16 * 16).saturating_sub(search_range)).to_be_bytes());
    let mut offset = 12 + 16 * n;
    for (tag, sum, body) in &tables {
        out.extend_from_slice(tag);
        out.extend_from_slice(&sum.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        offset += body.len().next_multiple_of(4);
    }
    for (_, _, body) in &tables {
        out.extend_from_slice(body);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seti_unpacks_to_a_truetype_font() {
        let ttf = to_sfnt(include_bytes!("../assets/seti/seti.woff")).unwrap();
        assert_eq!(&ttf[..4], [0, 1, 0, 0]);
        let tables = u16::from_be_bytes([ttf[4], ttf[5]]) as usize;
        let tags: Vec<&[u8]> = (0..tables).map(|i| &ttf[12 + i * 16..16 + i * 16]).collect();
        assert!(tags.contains(&&b"cmap"[..]) && tags.contains(&&b"glyf"[..]));
        // every table lies inside the file
        for i in 0..tables {
            let (off, len) = (be32(&ttf, 12 + i * 16 + 8).unwrap() as usize, be32(&ttf, 12 + i * 16 + 12).unwrap() as usize);
            assert!(off + len <= ttf.len());
        }
    }

    #[test]
    fn other_data_is_refused_or_passed_through() {
        assert!(to_sfnt(b"wOF2....").is_none());
        assert!(to_sfnt(b"hello").is_none());
        assert!(to_sfnt(b"wOFF").is_none(), "truncated");
        assert_eq!(to_sfnt(b"OTTOxxxx").unwrap(), b"OTTOxxxx");
    }
}
