//! BPS patches, as made by Floating IPS (`flips`) and beat, which is what
//! FTL's version rollbacks are. A patch records the size and CRC32 of the
//! file it applies to and of the file it makes, so it can tell whether a
//! file is the one it's for, and whether it applied correctly.
//!
//! The format: `BPS1`, then the source's size, the target's size and the
//! metadata's size as numbers (see [`Reader::number`]), the metadata, then
//! commands up to the last 12 bytes, which are the CRC32s of the source,
//! the target and the patch before them, little-endian. Each command is a
//! number whose lowest two bits say what it does and the rest how many
//! bytes, less one:
//!
//! 0. copy that many bytes from the source, at the same offset as the
//!    target's end;
//! 1. copy that many bytes from the patch;
//! 2. move a cursor in the source by a signed number that follows, and
//!    copy from there;
//! 3. the same with a cursor in the target, so it can repeat what it just
//!    wrote.

/// A parsed BPS patch.
#[derive(Debug)]
pub struct Patch<'a> {
    pub source_size: u64,
    pub target_size: u64,
    pub source_crc: u32,
    pub target_crc: u32,
    /// The commands.
    body: &'a [u8],
}

/// Reads numbers and bytes from a patch.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self
            .bytes
            .get(self.at)
            .ok_or("the patch ends in the middle of a command")?;
        self.at += 1;
        Ok(b)
    }

    /// A number: seven bits a byte, lowest first, the last byte's top bit
    /// set. Each byte after the first also adds the value it would have had
    /// at zero, so every number has one encoding.
    fn number(&mut self) -> Result<u64, String> {
        let too_big = || "the patch has a number too big to be right".to_owned();
        let mut value: u64 = 0;
        let mut shift: u64 = 1;
        loop {
            let b = self.byte()?;
            value = u64::from(b & 0x7f)
                .checked_mul(shift)
                .and_then(|v| v.checked_add(value))
                .ok_or_else(too_big)?;
            if b & 0x80 != 0 {
                return Ok(value);
            }
            shift = shift.checked_shl(7).ok_or_else(too_big)?;
            value = value.checked_add(shift).ok_or_else(too_big)?;
        }
    }

    fn bytes(&mut self, len: usize) -> Result<&[u8], String> {
        let end = self
            .at
            .checked_add(len)
            .filter(|&end| end <= self.bytes.len())
            .ok_or("the patch ends in the middle of a command")?;
        let bytes = &self.bytes[self.at..end];
        self.at = end;
        Ok(bytes)
    }
}

fn crc_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn size(n: u64) -> Result<usize, String> {
    usize::try_from(n).map_err(|_| "the patch is for a file too big to be right".to_owned())
}

impl<'a> Patch<'a> {
    /// Reads a patch's header and checks it's whole.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        if bytes.len() < 4 + 3 + 12 || !bytes.starts_with(b"BPS1") {
            return Err("it isn't a BPS patch".to_owned());
        }
        let footer = bytes.len() - 12;
        if crc32fast::hash(&bytes[..footer + 8]) != crc_at(bytes, footer + 8) {
            return Err("the patch is damaged (its checksum is wrong)".to_owned());
        }
        let mut r = Reader {
            bytes: &bytes[..footer],
            at: 4,
        };
        let source_size = r.number()?;
        let target_size = r.number()?;
        let metadata = r.number()?;
        r.bytes(size(metadata)?)?;
        Ok(Self {
            source_size,
            target_size,
            source_crc: crc_at(bytes, footer),
            target_crc: crc_at(bytes, footer + 4),
            body: &bytes[r.at..footer],
        })
    }

    /// Whether `source` is the file this patch is for.
    pub fn applies_to(&self, source: &[u8]) -> bool {
        source.len() as u64 == self.source_size && crc32fast::hash(source) == self.source_crc
    }

    /// Whether `file` is what this patch makes.
    pub fn made(&self, file: &[u8]) -> bool {
        file.len() as u64 == self.target_size && crc32fast::hash(file) == self.target_crc
    }

    /// What applying the patch to `source` makes.
    pub fn apply(&self, source: &[u8]) -> Result<Vec<u8>, String> {
        if !self.applies_to(source) {
            return Err("the patch isn't for this file".to_owned());
        }
        let target_size = size(self.target_size)?;
        let mut target: Vec<u8> = Vec::with_capacity(target_size);
        let mut r = Reader {
            bytes: self.body,
            at: 0,
        };
        let mut source_at: usize = 0;
        let mut target_at: usize = 0;
        let out_of_range = || "the patch copies from past the end of a file".to_owned();
        while r.at < self.body.len() {
            let command = r.number()?;
            let len = size((command >> 2) + 1)?;
            if target.len() + len > target_size {
                return Err("the patch makes a bigger file than it says".to_owned());
            }
            match command & 3 {
                0 => {
                    let start = target.len();
                    let from = source.get(start..start + len).ok_or_else(out_of_range)?;
                    target.extend_from_slice(from);
                }
                1 => target.extend_from_slice(r.bytes(len)?),
                2 => {
                    source_at = moved(source_at, r.number()?).ok_or_else(out_of_range)?;
                    let from = source
                        .get(source_at..source_at + len)
                        .ok_or_else(out_of_range)?;
                    target.extend_from_slice(from);
                    source_at += len;
                }
                _ => {
                    target_at = moved(target_at, r.number()?).ok_or_else(out_of_range)?;
                    // It may copy what it's writing, a byte at a time.
                    if target_at >= target.len() {
                        return Err(out_of_range());
                    }
                    for _ in 0..len {
                        target.push(target[target_at]);
                        target_at += 1;
                    }
                }
            }
        }
        if !self.made(&target) {
            return Err("the patch didn't make the file it should have".to_owned());
        }
        Ok(target)
    }
}

/// `at` moved by a relative offset: the lowest bit is the sign, the rest
/// the distance.
fn moved(at: usize, offset: u64) -> Option<usize> {
    let distance = usize::try_from(offset >> 1).ok()?;
    if offset & 1 == 1 {
        at.checked_sub(distance)
    } else {
        at.checked_add(distance)
    }
}
