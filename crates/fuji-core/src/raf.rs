//! Fujifilm RAF container.
//!
//! A RAF file starts with a fixed big-endian header that points at a full
//! JPEG rendition of the shot (with the camera's film simulation applied).
//! Reading only that JPEG means we never have to demosaic X-Trans data to
//! browse a library, and over SMB we only pull a few MB instead of 50+.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

pub const MAGIC: &[u8; 16] = b"FUJIFILMCCD-RAW ";

/// Header bytes needed to locate the embedded JPEG.
const HEADER_LEN: usize = 92;
const JPEG_OFFSET_AT: usize = 84;
const JPEG_LENGTH_AT: usize = 88;

/// `(offset, length)` of the embedded JPEG, parsed from the first 92 bytes.
pub fn parse_header(header: &[u8]) -> io::Result<(u64, u64)> {
    if header.len() < HEADER_LEN || &header[..16] != MAGIC {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a RAF file"));
    }
    let be = |at: usize| u32::from_be_bytes(header[at..at + 4].try_into().unwrap()) as u64;
    let (offset, len) = (be(JPEG_OFFSET_AT), be(JPEG_LENGTH_AT));
    if offset < HEADER_LEN as u64 || len < 4 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RAF has no embedded JPEG",
        ));
    }
    Ok((offset, len))
}

/// A buffered reader positioned on the embedded JPEG and limited to its length.
pub fn open_embedded_jpeg(path: &Path) -> io::Result<BufReader<io::Take<File>>> {
    let mut file = File::open(path)?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header)?;
    let (offset, len) = parse_header(&header)?;
    // A damaged or half-copied file can claim more than it holds.
    let size = file.metadata()?.len();
    if offset >= size {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "RAF preview is past the end of the file",
        ));
    }
    let len = len.min(size - offset);
    file.seek(SeekFrom::Start(offset))?;
    Ok(BufReader::with_capacity(64 * 1024, file.take(len)))
}

pub fn read_embedded_jpeg(path: &Path) -> io::Result<Vec<u8>> {
    read_embedded_jpeg_prefix(path, u64::MAX)
}

/// The first `max` bytes of the embedded JPEG (or all of it, if shorter).
pub fn read_embedded_jpeg_prefix(path: &Path, max: u64) -> io::Result<Vec<u8>> {
    let reader = open_embedded_jpeg(path)?;
    let len = reader.get_ref().limit().min(max);
    let mut buf = Vec::with_capacity(len as usize);
    reader.take(len).read_to_end(&mut buf)?;
    if buf.len() < 4 || buf[0] != 0xFF || buf[1] != 0xD8 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "RAF preview is not a JPEG",
        ));
    }
    Ok(buf)
}

/// Builds a minimal RAF wrapper around `jpeg`. Used by tests and fixtures.
#[doc(hidden)]
pub fn wrap_jpeg_for_tests(jpeg: &[u8]) -> Vec<u8> {
    let offset = 160u32;
    let mut out = vec![0u8; offset as usize];
    out[..16].copy_from_slice(MAGIC);
    out[16..20].copy_from_slice(b"0201");
    out[28..36].copy_from_slice(b"X-T5\0\0\0\0");
    out[JPEG_OFFSET_AT..JPEG_OFFSET_AT + 4].copy_from_slice(&offset.to_be_bytes());
    out[JPEG_LENGTH_AT..JPEG_LENGTH_AT + 4].copy_from_slice(&(jpeg.len() as u32).to_be_bytes());
    out.extend_from_slice(jpeg);
    // Stand-in for the CFA payload that follows in a real file.
    out.extend_from_slice(&[0u8; 256]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A damaged or half-copied RAF can claim a preview far bigger than the
    /// file. Only what's actually there is read, so nothing huge is
    /// allocated.
    #[test]
    fn damaged_lengths_are_capped_by_the_file() {
        let jpeg = crate::test_jpeg(40, 30, None);
        let mut raf = wrap_jpeg_for_tests(&jpeg);
        raf[JPEG_LENGTH_AT..JPEG_LENGTH_AT + 4].copy_from_slice(&0xFFFF_FF00u32.to_be_bytes());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.RAF");
        std::fs::write(&path, &raf).unwrap();
        let got = read_embedded_jpeg(&path).unwrap();
        assert!(got.len() <= raf.len());
        assert!(got.starts_with(&jpeg));

        // A preview that would start past the end of the file.
        raf[JPEG_OFFSET_AT..JPEG_OFFSET_AT + 4].copy_from_slice(&0x7FFF_0000u32.to_be_bytes());
        std::fs::write(&path, &raf).unwrap();
        assert!(read_embedded_jpeg(&path).is_err());
    }

    #[test]
    fn rejects_non_raf() {
        assert!(parse_header(&[0u8; 100]).is_err());
        assert!(parse_header(b"short").is_err());
    }

    #[test]
    fn finds_embedded_jpeg() {
        let jpeg = [0xFF, 0xD8, 1, 2, 3, 0xFF, 0xD9];
        let raf = wrap_jpeg_for_tests(&jpeg);
        assert_eq!(parse_header(&raf).unwrap(), (160, jpeg.len() as u64));

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("DSCF0001.RAF");
        std::fs::write(&path, &raf).unwrap();
        assert_eq!(read_embedded_jpeg(&path).unwrap(), jpeg);
    }
}
