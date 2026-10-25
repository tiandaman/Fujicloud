//! Adobe DNG raws, such as iPhone ProRAW: the size of the picture and the
//! JPEG preview stored inside, which is what we show. Only the directory
//! entries and the preview itself are read, never the raw data.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use crate::Error;

/// Where a JPEG preview sits in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Preview {
    offset: u64,
    len: u64,
    pixels: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The developed picture's size, before orientation.
    pub width: u32,
    pub height: u32,
    pub orientation: u16,
    previews: Vec<Preview>,
}

fn bad(msg: &str) -> Error {
    Error::Decode(format!("DNG: {msg}"))
}

struct Reader<R> {
    r: R,
    big_endian: bool,
    len: u64,
}

impl<R: Read + Seek> Reader<R> {
    fn u16_at(&mut self, at: u64) -> Result<u16, Error> {
        let mut b = [0; 2];
        self.r.seek(SeekFrom::Start(at))?;
        self.r.read_exact(&mut b)?;
        Ok(if self.big_endian {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn u32_at(&mut self, at: u64) -> Result<u32, Error> {
        let mut b = [0; 4];
        self.r.seek(SeekFrom::Start(at))?;
        self.r.read_exact(&mut b)?;
        Ok(if self.big_endian {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    /// An entry's values as integers (rationals rounded); at most `max`.
    fn values(&mut self, entry: u64, max: usize) -> Result<Vec<u64>, Error> {
        let kind = self.u16_at(entry + 2)?;
        let count = self.u32_at(entry + 4)? as usize;
        let size = match kind {
            1 | 7 => 1,
            3 => 2,
            4 | 13 => 4,
            5 => 8,
            _ => return Ok(Vec::new()),
        };
        let at = if count * size <= 4 {
            entry + 8
        } else {
            self.u32_at(entry + 8)? as u64
        };
        let mut out = Vec::new();
        for i in 0..count.min(max) {
            let pos = at + (i * size) as u64;
            out.push(match kind {
                1 | 7 => {
                    let mut b = [0];
                    self.r.seek(SeekFrom::Start(pos))?;
                    self.r.read_exact(&mut b)?;
                    b[0] as u64
                }
                3 => self.u16_at(pos)? as u64,
                4 | 13 => self.u32_at(pos)? as u64,
                _ => {
                    let (n, d) = (self.u32_at(pos)?, self.u32_at(pos + 4)?);
                    if d == 0 {
                        0
                    } else {
                        (n as u64 + d as u64 / 2) / d as u64
                    }
                }
            });
        }
        Ok(out)
    }
}

/// The handful of tags we look at in one image directory.
#[derive(Default)]
struct Dir {
    subfile: u64,
    width: u64,
    height: u64,
    bits: Option<u64>,
    compression: u64,
    photometric: Option<u64>,
    strips: Vec<u64>,
    strip_lens: Vec<u64>,
    jpeg_at: Option<u64>,
    jpeg_len: Option<u64>,
    crop: Vec<u64>,
    orientation: Option<u64>,
    children: Vec<u64>,
    next: u64,
}

fn read_dir<R: Read + Seek>(r: &mut Reader<R>, at: u64) -> Result<Dir, Error> {
    let count = r.u16_at(at)? as u64;
    if count == 0 || count > 2000 || at + 2 + count * 12 + 4 > r.len {
        return Err(bad("damaged directory"));
    }
    let mut d = Dir::default();
    for i in 0..count {
        let e = at + 2 + i * 12;
        let first = |r: &mut Reader<R>| -> Result<Option<u64>, Error> {
            Ok(r.values(e, 1)?.first().copied())
        };
        match r.u16_at(e)? {
            0x00FE => d.subfile = first(r)?.unwrap_or(0),
            0x0100 => d.width = first(r)?.unwrap_or(0),
            0x0101 => d.height = first(r)?.unwrap_or(0),
            0x0102 => d.bits = first(r)?,
            0x0103 => d.compression = first(r)?.unwrap_or(1),
            0x0106 => d.photometric = first(r)?,
            0x0111 => d.strips = r.values(e, 4096)?,
            0x0112 => d.orientation = first(r)?,
            0x0117 => d.strip_lens = r.values(e, 4096)?,
            0x014A => d.children = r.values(e, 16)?,
            0x0201 => d.jpeg_at = first(r)?,
            0x0202 => d.jpeg_len = first(r)?,
            0xC620 => d.crop = r.values(e, 2)?,
            _ => {}
        }
    }
    d.next = r.u32_at(at + 2 + count * 12)? as u64;
    Ok(d)
}

impl Dir {
    /// A baseline JPEG we can show (not the lossless-compressed raw data).
    fn preview(&self, file_len: u64) -> Option<Preview> {
        let colour = matches!(self.photometric, None | Some(2) | Some(6));
        let eight_bit = matches!(self.bits, None | Some(8));
        let (offset, len) = match (self.jpeg_at, self.jpeg_len) {
            (Some(at), Some(len)) => (at, len),
            _ if matches!(self.compression, 6 | 7) && self.strips.len() == 1 => {
                (self.strips[0], *self.strip_lens.first()?)
            }
            _ => return None,
        };
        (colour && eight_bit && len > 0 && offset + len <= file_len).then_some(Preview {
            offset,
            len,
            pixels: self.width * self.height,
        })
    }
}

pub fn info(path: &Path) -> Result<Info, Error> {
    let file = File::open(path)?;
    let len = file.metadata()?.len();
    let mut r = Reader {
        r: BufReader::with_capacity(16 * 1024, file),
        big_endian: false,
        len,
    };
    let mut head = [0u8; 8];
    r.r.read_exact(&mut head)?;
    r.big_endian = match &head[..4] {
        b"II*\0" => false,
        b"MM\0*" => true,
        _ => return Err(bad("not a TIFF file")),
    };
    let first = r.u32_at(4)? as u64;

    let mut info = Info {
        width: 0,
        height: 0,
        orientation: 1,
        previews: Vec::new(),
    };
    let mut main: Option<(u64, u64)> = None;
    let mut queue = vec![first];
    let mut seen = Vec::new();
    while let Some(at) = queue.pop() {
        if at == 0 || at >= len || seen.contains(&at) || seen.len() > 64 {
            continue;
        }
        seen.push(at);
        let d = read_dir(&mut r, at)?;
        if at == first {
            info.orientation = d.orientation.unwrap_or(1) as u16;
            queue.push(d.next);
        }
        queue.extend(&d.children);
        info.previews.extend(d.preview(len));
        // The full-size image; its crop is what a raw developer outputs.
        if d.subfile & 1 == 0 && main.is_none_or(|(w, h)| d.width * d.height > w * h) {
            main = Some(match d.crop[..] {
                [w, h] if w > 0 && h > 0 => (w, h),
                _ => (d.width, d.height),
            });
        }
    }
    let (w, h) = main.ok_or_else(|| bad("no image inside"))?;
    (info.width, info.height) = (w as u32, h as u32);
    info.previews.sort_by_key(|p| std::cmp::Reverse(p.pixels));
    Ok(info)
}

/// The largest JPEG preview, and the orientation to show it in.
pub fn preview(path: &Path) -> Result<(Vec<u8>, u16), Error> {
    let info = info(path)?;
    let mut file = File::open(path)?;
    for p in &info.previews {
        let mut jpeg = vec![0; p.len as usize];
        file.seek(SeekFrom::Start(p.offset))?;
        file.read_exact(&mut jpeg)?;
        if jpeg.starts_with(&[0xFF, 0xD8]) {
            return Ok((jpeg, info.orientation));
        }
    }
    Err(bad("no preview inside"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A little-endian DNG like an iPhone's: IFD0 a small thumbnail with
    /// two SubIFDs, the raw (lossless, 16-bit) and a JPEG preview.
    pub(crate) fn sample(preview_jpeg: &[u8], orientation: u16) -> Vec<u8> {
        fn entry(out: &mut Vec<u8>, tag: u16, kind: u16, count: u32, value: u32) {
            out.extend(tag.to_le_bytes());
            out.extend(kind.to_le_bytes());
            out.extend(count.to_le_bytes());
            out.extend(value.to_le_bytes());
        }
        fn dir(out: &mut Vec<u8>, entries: &[(u16, u16, u32, u32)], next: u32) {
            out.extend((entries.len() as u16).to_le_bytes());
            for &(t, k, c, v) in entries {
                entry(out, t, k, c, v);
            }
            out.extend(next.to_le_bytes());
        }
        let ifd0 = 8u32;
        let sub_list = 200u32;
        let raw_dir = 300u32;
        let preview_dir = 500u32;
        let preview_at = 700u32;
        let mut out = b"II*\0".to_vec();
        out.extend(ifd0.to_le_bytes());
        dir(
            &mut out,
            &[
                (0x00FE, 4, 1, 1),
                (0x0100, 4, 1, 256),
                (0x0101, 4, 1, 192),
                (0x0102, 3, 1, 8),
                (0x0103, 3, 1, 1),
                (0x0112, 3, 1, orientation as u32),
                (0x014A, 4, 2, sub_list),
            ],
            0,
        );
        out.resize(sub_list as usize, 0);
        out.extend(raw_dir.to_le_bytes());
        out.extend(preview_dir.to_le_bytes());
        out.resize(raw_dir as usize, 0);
        dir(
            &mut out,
            &[
                (0x00FE, 4, 1, 0),
                (0x0100, 4, 1, 4284),
                (0x0101, 4, 1, 5712),
                (0x0102, 3, 1, 16),
                (0x0103, 3, 1, 7),
                (0x0106, 3, 1, 34892),
                (0x0111, 4, 1, 0),
                (0x0117, 4, 1, 0),
                (0xC620, 3, 2, 4032 | (3024 << 16)),
            ],
            0,
        );
        out.resize(preview_dir as usize, 0);
        dir(
            &mut out,
            &[
                (0x00FE, 4, 1, 1),
                (0x0100, 4, 1, 64),
                (0x0101, 4, 1, 48),
                (0x0102, 3, 1, 8),
                (0x0103, 3, 1, 7),
                (0x0106, 3, 1, 6),
                (0x0111, 4, 1, preview_at),
                (0x0117, 4, 1, preview_jpeg.len() as u32),
            ],
            0,
        );
        out.resize(preview_at as usize, 0);
        out.extend(preview_jpeg);
        out
    }

    #[test]
    fn finds_the_size_and_the_preview_but_not_the_raw_data() {
        let jpeg = [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
        let path = std::env::temp_dir().join(format!("fuji-dng-{}.dng", std::process::id()));
        std::fs::write(&path, sample(&jpeg, 6)).unwrap();

        let found = info(&path).unwrap();
        assert_eq!(
            (found.width, found.height, found.orientation),
            (4032, 3024, 6)
        );
        assert_eq!(found.previews.len(), 1, "the 16-bit raw isn't a preview");
        assert_eq!(preview(&path).unwrap(), (jpeg.to_vec(), 6));

        std::fs::write(&path, b"II*\0\xff\xff\xff\x7f").unwrap();
        assert!(info(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }
}
