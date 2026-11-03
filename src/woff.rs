//! WOFF 1 to plain sfnt (TTF/OTF): the text system takes sfnt, icon themes ship `.woff` (Seti does).
use std::collections::BTreeMap;
use std::io::Read as _;

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The font as sfnt: a WOFF is unpacked, sfnt data is returned as is; `None` for anything else (WOFF2 needs Brotli: not supported).
pub fn to_sfnt(data: &[u8]) -> Option<Vec<u8>> {
    match data.get(..4)? {
        b"wOFF" => {}
        [0, 1, 0, 0] | b"OTTO" | b"true" => return rebuild(data.get(..4)?, sfnt_tables(data)?),
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
    rebuild(data.get(4..8)?, tables)
}

type Table<'a> = (&'a [u8], u32, Vec<u8>);

fn sfnt_tables(data: &[u8]) -> Option<Vec<Table<'_>>> {
    let n = u16::from_be_bytes(data.get(4..6)?.try_into().ok()?) as usize;
    (0..n)
        .map(|i| {
            let e = 12 + i * 16;
            let (offset, len) = (be32(data, e + 8)? as usize, be32(data, e + 12)? as usize);
            Some((data.get(e..e + 4)?, be32(data, e + 4)?, data.get(offset..offset.checked_add(len)?)?.to_vec()))
        })
        .collect()
}

fn checksum(b: &[u8]) -> u32 {
    b.chunks(4).fold(0u32, |s, c| s.wrapping_add(u32::from_be_bytes([c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0), *c.get(3).unwrap_or(&0)])))
}

/// character -> glyph of a format 4 or 12 cmap subtable.
fn read_subtable(t: &[u8]) -> Option<BTreeMap<u32, u16>> {
    let u16_at = |at: usize| Some(u16::from_be_bytes(t.get(at..at + 2)?.try_into().ok()?));
    let mut map = BTreeMap::new();
    match u16_at(0)? {
        4 => {
            let segs = u16_at(6)? as usize / 2;
            for i in 0..segs {
                let (end, start, delta, range) = (u16_at(14 + 2 * i)?, u16_at(16 + 2 * segs + 2 * i)?, u16_at(16 + 4 * segs + 2 * i)?, u16_at(16 + 6 * segs + 2 * i)?);
                for c in start..=end.min(0xfffe) {
                    let at = 16 + 6 * segs + 2 * i + range as usize + 2 * (c - start) as usize;
                    let g = if range == 0 { c.wrapping_add(delta) } else { match u16_at(at)? { 0 => 0, g => g.wrapping_add(delta) } };
                    if g != 0 {
                        map.insert(c as u32, g);
                    }
                }
            }
        }
        12 => {
            for i in 0..be32(t, 12)? as usize {
                let (start, end, g) = (be32(t, 16 + 12 * i)?, be32(t, 20 + 12 * i)?, be32(t, 24 + 12 * i)?);
                for c in start..=end.min(0xffff) {
                    map.insert(c, u16::try_from(g + (c - start)).ok()?);
                }
            }
        }
        _ => return None,
    }
    Some(map)
}

/// A cmap that also maps `m`, as format 4 under the Unicode and Windows BMP records; `None` when `m` is mapped already or the cmap is not understood.
/// GPUI's mac text system silently drops a font without an `m` glyph (it measures with it), and Seti, an icon font, has none.
fn cmap_with_m(cmap: &[u8], glyphs: u16) -> Option<Vec<u8>> {
    let records = u16::from_be_bytes(cmap.get(2..4)?.try_into().ok()?) as usize;
    let mut map = (0..records).find_map(|i| {
        let off = be32(cmap, 8 + 8 * i)? as usize;
        read_subtable(cmap.get(off..)?)
    })?;
    if glyphs < 2 || map.contains_key(&('m' as u32)) {
        return None;
    }
    map.insert('m' as u32, 1);
    // one segment per run of consecutive characters with consecutive glyphs
    let mut segs: Vec<(u16, u16, u16)> = Vec::new(); // start, end, delta
    for (&c, &g) in map.iter().filter(|(c, _)| **c < 0xffff) {
        let (c, delta) = (c as u16, g.wrapping_sub(c as u16));
        match segs.last_mut() {
            Some(s) if s.1 + 1 == c && s.2 == delta => s.1 = c,
            _ => segs.push((c, c, delta)),
        }
    }
    segs.push((0xffff, 0xffff, 1));
    let n = segs.len() as u16;
    let entry_selector = n.ilog2() as u16;
    let search_range = 2 * (1u16 << entry_selector);
    let mut sub = vec![0, 4, 0, 0, 0, 0]; // format, length, language (length below)
    for v in [n * 2, search_range, entry_selector, n * 2 - search_range] {
        sub.extend(v.to_be_bytes());
    }
    let column = |sub: &mut Vec<u8>, f: fn(&(u16, u16, u16)) -> u16| segs.iter().for_each(|s| sub.extend(f(s).to_be_bytes()));
    column(&mut sub, |s| s.1); // endCode
    sub.extend([0, 0]); // reservedPad
    column(&mut sub, |s| s.0); // startCode
    column(&mut sub, |s| s.2); // idDelta
    column(&mut sub, |_| 0); // idRangeOffset
    let len = (sub.len() as u16).to_be_bytes();
    sub[2..4].copy_from_slice(&len);
    let mut out = vec![0, 0, 0, 2];
    for (p, e) in [(0u16, 3u16), (3, 1)] {
        out.extend(p.to_be_bytes());
        out.extend(e.to_be_bytes());
        out.extend(20u32.to_be_bytes());
    }
    out.extend(sub);
    Some(out)
}

fn rebuild(flavor: &[u8], mut tables: Vec<Table<'_>>) -> Option<Vec<u8>> {
    let glyphs = tables.iter().find(|t| t.0 == b"maxp").and_then(|t| t.2.get(4..6)).map_or(0, |g| u16::from_be_bytes([g[0], g[1]]));
    if let Some(t) = tables.iter_mut().find(|t| t.0 == b"cmap") {
        if let Some(body) = cmap_with_m(&t.2, glyphs) {
            (t.1, t.2) = (checksum(&body), body);
        }
    }
    let n = tables.len();
    let mut out = Vec::new();
    out.extend_from_slice(flavor);
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
    fn seti_gains_an_m_and_keeps_its_glyphs() {
        let cmap_of = |f: &[u8]| {
            let t = sfnt_tables(f).unwrap().into_iter().find(|t| t.0 == b"cmap").unwrap().2;
            read_subtable(&t[be32(&t, 16).unwrap() as usize..]).unwrap() // second record: Windows BMP
        };
        let fixed = to_sfnt(include_bytes!("../assets/seti/seti.woff")).unwrap();
        let mut map = cmap_of(&fixed);
        assert_eq!(map.remove(&('m' as u32)), Some(1));
        assert!(map.len() > 100 && map.keys().all(|c| (0xe000..0xf900).contains(c)), "the icons stay");
        assert_eq!(to_sfnt(&fixed).unwrap(), fixed, "sfnt input is stable");
    }

    #[test]
    fn other_data_is_refused_or_passed_through() {
        assert!(to_sfnt(b"wOF2....").is_none());
        assert!(to_sfnt(b"hello").is_none());
        assert!(to_sfnt(b"wOFF").is_none(), "truncated");
        assert!(to_sfnt(b"OTTOxxxx").is_none(), "no table directory");
    }
}
