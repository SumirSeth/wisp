//! Small, bounded reader for CustomDestinations v2 and serialized Shell Links.
//! Format: https://github.com/libyal/dtformats/blob/main/documentation/Jump%20lists%20format.asciidoc
//! Link boundaries follow MS-SHLLINK, never a search for magic inside payloads.

pub const MAX_FILE: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 128;
const LINK_CLSID: [u8; 16] = [1, 20, 2, 0, 0, 0, 0, 0, 192, 0, 0, 0, 0, 0, 0, 70];

/// Reflected CRC64, init all ones, no final xor, over uppercase UTF-16LE
/// WITHOUT a NUL terminator. Explicit AUMIDs need no path normalization.
/// Source and known vectors: https://www.hexacorn.com/tools/appid_calc.pl
/// and https://www.hexacorn.com/blog/2013/04/30/jumplists-file-names-and-appid-calculator/
pub fn app_id_hash(aumid: &str) -> u64 {
    let mut crc = u64::MAX;
    for byte in aumid.to_uppercase().encode_utf16().flat_map(u16::to_le_bytes) {
        crc ^= u64::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0x92c6_4265_d321_39a4 } else { 0 };
        }
    }
    crc
}
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(len)?;
        let value = self.data.get(self.pos..end)?;
        self.pos = end;
        Some(value)
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_le_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn count(&mut self) -> Option<usize> {
        let count = self.u32()? as usize;
        (count <= MAX_ENTRIES).then_some(count)
    }
    fn link(&mut self) -> Option<&'a [u8]> {
        if self.take(16)? != LINK_CLSID {
            return None;
        }
        let start = self.pos;
        if self.u32()? != 76 || self.take(16)? != LINK_CLSID {
            return None;
        }
        let flags = self.u32()?;
        self.take(52)?;
        if flags & 1 != 0 {
            let len = self.u16()? as usize;
            self.take(len)?;
        }
        if flags & 2 != 0 {
            let len = self.u32()? as usize;
            if len < 28 {
                return None;
            }
            self.take(len.checked_sub(4)?)?;
        }
        for mask in [4, 8, 16, 32, 64] {
            if flags & mask != 0 {
                let chars = self.u16()? as usize;
                self.take(chars.checked_mul(if flags & 128 != 0 { 2 } else { 1 })?)?;
            }
        }
        loop {
            let size = self.u32()? as usize;
            if size == 0 {
                break;
            }
            if size < 8 {
                return None;
            }
            self.take(size.checked_sub(4)?)?;
        }
        self.data.get(start..self.pos)
    }
}

/// Validate the ENTIRE file before exposing tasks. Other categories are walked
/// but not displayed. Unknown versions, categories, classes, missing footers,
/// truncated lengths, excessive counts/sizes and trailing bytes fail closed.
pub fn task_links(bytes: &[u8]) -> Option<Vec<&[u8]>> {
    if bytes.len() > MAX_FILE {
        return None;
    }
    let mut r = Reader { data: bytes, pos: 0 };
    if r.u32()? != 2 {
        return None;
    }
    let categories = r.count()?;
    if r.u32()? != 0 {
        return None;
    }
    let mut tasks = Vec::new();
    let mut total = 0usize;
    for _ in 0..categories {
        let kind = r.u32()?;
        match kind {
            0 | 2 => {
                if kind == 0 {
                    let chars = r.u16()? as usize;
                    let name = r.take(chars.checked_mul(2)?)?;
                    let units: Vec<_> = name
                        .as_chunks::<2>()
                        .0
                        .iter()
                        .copied()
                        .map(u16::from_le_bytes)
                        .collect();
                    String::from_utf16(&units).ok()?;
                }
                let count = r.count()?;
                total = total.checked_add(count)?;
                if total > MAX_ENTRIES {
                    return None;
                }
                for _ in 0..count {
                    let link = r.link()?;
                    if kind == 2 {
                        tasks.push(link);
                    }
                }
            }
            1 => {
                if !matches!(r.u32()?, 1 | 2) {
                    return None;
                }
            }
            _ => return None,
        }
        if r.u32()? != 0xbabffbab {
            return None;
        }
    }
    (r.pos == bytes.len()).then_some(tasks)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn two_tasks() -> Vec<u8> {
        let mut bytes: Vec<u8> = [2u32, 1, 0, 2, 2].into_iter().flat_map(u32::to_le_bytes).collect();
        for _ in 0..2 {
            bytes.extend(LINK_CLSID);
            bytes.extend(76u32.to_le_bytes());
            bytes.extend(LINK_CLSID);
            bytes.extend([0; 56]);
            bytes.extend([0; 4]);
        }
        bytes.extend(0xbabffbabu32.to_le_bytes());
        bytes
    }
    #[test]
    fn published_hashes() {
        assert_eq!(app_id_hash("Microsoft.InternetExplorer.Default"), 0x28c8b86deab549a1);
        assert_eq!(app_id_hash("Microsoft.Windows.StickyNotes"), 0x337ed59af273c758);
        // Known-folder normalized Notepad x64 identity from the same reference.
        assert_eq!(
            app_id_hash(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe"),
            0x9b9cdc69c1c24e2b
        );
    }
    #[test]
    fn valid_two_tasks() {
        assert_eq!(task_links(&two_tasks()).unwrap().len(), 2);
    }
    #[test]
    fn walks_named_and_known_categories_without_showing_them() {
        let task_file = two_tasks();
        let mut bytes: Vec<u8> = [2u32, 3, 0, 1, 2, 0xbabffbab, 0]
            .into_iter()
            .flat_map(u32::to_le_bytes)
            .collect();
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(('X' as u16).to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(0xbabffbabu32.to_le_bytes());
        bytes.extend(&task_file[12..]);
        assert_eq!(task_links(&bytes).unwrap().len(), 2);
        let mut bad = bytes.clone();
        bad[16..20].copy_from_slice(&99u32.to_le_bytes());
        assert!(task_links(&bad).is_none());
    }
    #[test]
    fn checks_link_variable_lengths_and_extra_data() {
        let mut bytes: Vec<u8> = [2u32, 1, 0, 2, 1].into_iter().flat_map(u32::to_le_bytes).collect();
        bytes.extend(LINK_CLSID);
        bytes.extend(76u32.to_le_bytes());
        bytes.extend(LINK_CLSID);
        bytes.extend(0xfdu32.to_le_bytes()); // IDList, Unicode and all StringData, no LinkInfo.
        bytes.extend([0; 52]);
        bytes.extend(2u16.to_le_bytes());
        bytes.extend([0; 2]);
        for _ in 0..5 {
            bytes.extend(1u16.to_le_bytes());
            bytes.extend(('X' as u16).to_le_bytes());
        }
        bytes.extend(8u32.to_le_bytes());
        bytes.extend(0xa0000001u32.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes.extend(0xbabffbabu32.to_le_bytes());
        assert_eq!(task_links(&bytes).unwrap().len(), 1);
        for end in 0..bytes.len() {
            assert!(task_links(&bytes[..end]).is_none());
        }
        bytes[112..114].copy_from_slice(&u16::MAX.to_le_bytes());
        assert!(task_links(&bytes).is_none());
    }
    #[test]
    fn rejects_every_truncation() {
        let bytes = two_tasks();
        for end in 0..bytes.len() {
            assert!(task_links(&bytes[..end]).is_none(), "{end}");
        }
    }
    #[test]
    fn rejects_wrong_magic_counts_lengths_and_trailing_bytes() {
        for (offset, value) in [
            (0, 3),
            (4, u32::MAX),
            (8, 1),
            (12, 99),
            (16, u32::MAX),
            (36, 77),
            (56, 2),
        ] {
            let mut bytes = two_tasks();
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
            assert!(task_links(&bytes).is_none(), "offset {offset}");
        }
        let mut bytes = two_tasks();
        bytes.push(0);
        assert!(task_links(&bytes).is_none());
        assert!(task_links(&vec![0; MAX_FILE + 1]).is_none());
    }
}
