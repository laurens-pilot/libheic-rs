use std::error::Error;
use std::fmt::{Display, Formatter};

const BASIC_HEADER_SIZE: usize = 8;
const LARGE_SIZE_FIELD_SIZE: usize = 8;
const UUID_EXTENDED_TYPE_SIZE: usize = 16;
const UUID_BOX_TYPE: [u8; 4] = *b"uuid";

/// Four-character box type code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct FourCc([u8; 4]);

impl FourCc {
    pub const fn new(bytes: [u8; 4]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(self) -> [u8; 4] {
        self.0
    }
}

impl Display for FourCc {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match std::str::from_utf8(&self.0) {
            Ok(code) => write!(f, "{code}"),
            Err(_) => write!(
                f,
                "{:02x}{:02x}{:02x}{:02x}",
                self.0[0], self.0[1], self.0[2], self.0[3]
            ),
        }
    }
}

/// Parsed ISO BMFF box header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoxHeader {
    pub box_type: FourCc,
    pub box_size: u64,
    pub header_size: u8,
    pub uuid: Option<[u8; 16]>,
}

impl BoxHeader {
    pub fn payload_size(&self) -> u64 {
        self.box_size - u64::from(self.header_size)
    }
}

/// Zero-copy parsed box view into an input slice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParsedBox<'a> {
    pub header: BoxHeader,
    pub payload: &'a [u8],
    pub offset: u64,
}

/// Errors returned by strict BMFF box parsing.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseBoxError {
    TruncatedHeader {
        offset: u64,
        available: usize,
    },
    TruncatedLargeSize {
        offset: u64,
        available: usize,
    },
    TruncatedUuid {
        offset: u64,
        available: usize,
    },
    InvalidBoxSize {
        offset: u64,
        box_size: u64,
        header_size: u8,
    },
    BoxOutOfBounds {
        offset: u64,
        box_size: u64,
        available: u64,
    },
}

impl Display for ParseBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseBoxError::TruncatedHeader { offset, available } => write!(
                f,
                "truncated BMFF box header at offset {offset} (available: {available} bytes)"
            ),
            ParseBoxError::TruncatedLargeSize { offset, available } => write!(
                f,
                "truncated BMFF large-size field at offset {offset} (available: {available} bytes)"
            ),
            ParseBoxError::TruncatedUuid { offset, available } => write!(
                f,
                "truncated BMFF uuid extended type at offset {offset} (available: {available} bytes)"
            ),
            ParseBoxError::InvalidBoxSize {
                offset,
                box_size,
                header_size,
            } => write!(
                f,
                "invalid BMFF box size at offset {offset}: box size {box_size} < header size {header_size}"
            ),
            ParseBoxError::BoxOutOfBounds {
                offset,
                box_size,
                available,
            } => write!(
                f,
                "BMFF box at offset {offset} exceeds parent range: size {box_size}, available {available}"
            ),
        }
    }
}

impl Error for ParseBoxError {}

/// Iterate top-level boxes from an input slice.
pub struct BoxIter<'a> {
    input: &'a [u8],
    cursor: usize,
    base_offset: u64,
    finished: bool,
}

impl<'a> BoxIter<'a> {
    pub fn new(input: &'a [u8]) -> Self {
        Self::with_offset(input, 0)
    }

    pub fn with_offset(input: &'a [u8], base_offset: u64) -> Self {
        Self {
            input,
            cursor: 0,
            base_offset,
            finished: false,
        }
    }
}

impl<'a> Iterator for BoxIter<'a> {
    type Item = Result<ParsedBox<'a>, ParseBoxError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished || self.cursor >= self.input.len() {
            return None;
        }

        let remaining = &self.input[self.cursor..];
        let offset = self.base_offset + self.cursor as u64;
        match parse_next_box(remaining, offset) {
            Ok((parsed_box, consumed_len)) => {
                self.cursor += consumed_len;
                Some(Ok(parsed_box))
            }
            Err(err) => {
                self.finished = true;
                Some(Err(err))
            }
        }
    }
}

/// Parse all top-level boxes from an input slice.
pub fn parse_boxes(input: &[u8]) -> Result<Vec<ParsedBox<'_>>, ParseBoxError> {
    BoxIter::new(input).collect()
}

fn parse_next_box(input: &[u8], offset: u64) -> Result<(ParsedBox<'_>, usize), ParseBoxError> {
    let available = input.len() as u64;
    let (header, header_len) = parse_header(input, offset, available)?;
    let box_len = header.box_size as usize;
    let payload = &input[header_len..box_len];
    Ok((
        ParsedBox {
            header,
            payload,
            offset,
        },
        box_len,
    ))
}

fn parse_header(
    input: &[u8],
    offset: u64,
    available: u64,
) -> Result<(BoxHeader, usize), ParseBoxError> {
    if input.len() < BASIC_HEADER_SIZE {
        return Err(ParseBoxError::TruncatedHeader {
            offset,
            available: input.len(),
        });
    }

    // Provenance: this mirrors the libheif header and range checks in
    // libheif/libheif/box.cc:BoxHeader::parse_header and Box::read.
    let box_size_32 = read_u32_be(&input[0..4]);
    let box_type = FourCc::new([input[4], input[5], input[6], input[7]]);

    let mut header_size = BASIC_HEADER_SIZE;
    let box_size = if box_size_32 == 1 {
        let needed = BASIC_HEADER_SIZE + LARGE_SIZE_FIELD_SIZE;
        if input.len() < needed {
            return Err(ParseBoxError::TruncatedLargeSize {
                offset,
                available: input.len(),
            });
        }

        header_size = needed;
        read_u64_be(&input[BASIC_HEADER_SIZE..needed])
    } else if box_size_32 == 0 {
        available
    } else {
        u64::from(box_size_32)
    };

    let mut uuid = None;
    if box_type.as_bytes() == UUID_BOX_TYPE {
        let needed = header_size + UUID_EXTENDED_TYPE_SIZE;
        if input.len() < needed {
            return Err(ParseBoxError::TruncatedUuid {
                offset,
                available: input.len(),
            });
        }

        let mut uuid_bytes = [0_u8; 16];
        uuid_bytes.copy_from_slice(&input[header_size..needed]);
        uuid = Some(uuid_bytes);
        header_size = needed;
    }

    let header_size_u8 = header_size as u8;
    let header_size_u64 = u64::from(header_size_u8);
    if box_size < header_size_u64 {
        return Err(ParseBoxError::InvalidBoxSize {
            offset,
            box_size,
            header_size: header_size_u8,
        });
    }

    if box_size > available {
        return Err(ParseBoxError::BoxOutOfBounds {
            offset,
            box_size,
            available,
        });
    }

    Ok((
        BoxHeader {
            box_type,
            box_size,
            header_size: header_size_u8,
            uuid,
        },
        header_size,
    ))
}

fn read_u32_be(input: &[u8]) -> u32 {
    u32::from_be_bytes([input[0], input[1], input[2], input[3]])
}

fn read_u64_be(input: &[u8]) -> u64 {
    u64::from_be_bytes([
        input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        parse_boxes, BoxIter, ParseBoxError, BASIC_HEADER_SIZE, LARGE_SIZE_FIELD_SIZE,
        UUID_EXTENDED_TYPE_SIZE,
    };

    #[test]
    fn parses_single_basic_box() {
        let bytes = make_basic_box(*b"ftyp", &[0x6d, 0x69, 0x66, 0x31]);
        let parsed = parse_boxes(&bytes).expect("basic box should parse");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].header.box_type.as_bytes(), *b"ftyp");
        assert_eq!(parsed[0].header.box_size, 12);
        assert_eq!(parsed[0].header.header_size, 8);
        assert_eq!(parsed[0].header.payload_size(), 4);
        assert_eq!(parsed[0].payload, &[0x6d, 0x69, 0x66, 0x31]);
        assert_eq!(parsed[0].offset, 0);
    }

    #[test]
    fn parses_large_size_box() {
        let payload = [0xde, 0xad, 0xbe, 0xef];
        let bytes = make_large_box(*b"meta", &payload);
        let parsed = parse_boxes(&bytes).expect("large-size box should parse");
        assert_eq!(parsed[0].header.box_type.as_bytes(), *b"meta");
        assert_eq!(parsed[0].header.box_size, 20);
        assert_eq!(parsed[0].header.header_size, 16);
        assert_eq!(parsed[0].payload, &payload);
    }

    #[test]
    fn parses_uuid_box_with_extended_type() {
        let uuid: [u8; 16] = [
            0x22, 0xcc, 0x04, 0xc7, 0xd6, 0xd9, 0x4e, 0x07, 0x9d, 0x90, 0x4e, 0xb6, 0xec, 0xba,
            0xf3, 0xa3,
        ];
        let payload = [0x01, 0x02, 0x03];
        let bytes = make_uuid_box(uuid, &payload);
        let parsed = parse_boxes(&bytes).expect("uuid box should parse");
        assert_eq!(parsed[0].header.box_type.as_bytes(), *b"uuid");
        assert_eq!(parsed[0].header.header_size, 24);
        assert_eq!(parsed[0].header.uuid, Some(uuid));
        assert_eq!(parsed[0].payload, &payload);
    }

    #[test]
    fn parses_size_zero_box_to_end_of_parent_range() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0_u32.to_be_bytes());
        bytes.extend_from_slice(b"mdat");
        bytes.extend_from_slice(&[0x10, 0x11, 0x12]);

        let mut iter = BoxIter::new(&bytes);
        let parsed = iter
            .next()
            .expect("first box result should exist")
            .expect("size=0 box should parse");
        assert_eq!(parsed.header.box_size, bytes.len() as u64);
        assert_eq!(parsed.payload, &[0x10, 0x11, 0x12]);
        assert!(iter.next().is_none());
    }

    #[test]
    fn rejects_box_smaller_than_header() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&4_u32.to_be_bytes());
        bytes.extend_from_slice(b"free");

        let err = parse_boxes(&bytes).expect_err("size < header must fail");
        assert_eq!(
            err,
            ParseBoxError::InvalidBoxSize {
                offset: 0,
                box_size: 4,
                header_size: 8
            }
        );
    }

    #[test]
    fn rejects_box_past_available_range() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&16_u32.to_be_bytes());
        bytes.extend_from_slice(b"free");
        bytes.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);

        let err = parse_boxes(&bytes).expect_err("out-of-range box must fail");
        assert_eq!(
            err,
            ParseBoxError::BoxOutOfBounds {
                offset: 0,
                box_size: 16,
                available: 12
            }
        );
    }

    #[test]
    fn rejects_truncated_large_size_field() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&1_u32.to_be_bytes());
        bytes.extend_from_slice(b"meta");
        bytes.extend_from_slice(&[0_u8; 3]);

        let err = parse_boxes(&bytes).expect_err("truncated large-size field must fail");
        assert_eq!(
            err,
            ParseBoxError::TruncatedLargeSize {
                offset: 0,
                available: 11
            }
        );
    }

    #[test]
    fn payload_slice_is_zero_copy() {
        let bytes = make_basic_box(*b"free", &[0x03, 0x04, 0x05]);
        let parsed = parse_boxes(&bytes).expect("box should parse");
        let payload_ptr = parsed[0].payload.as_ptr();
        let expected_ptr = bytes[8..].as_ptr();
        assert_eq!(payload_ptr, expected_ptr);
    }

    fn make_basic_box(box_type: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (BASIC_HEADER_SIZE + payload.len()) as u32;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(&box_type);
        out.extend_from_slice(payload);
        out
    }

    fn make_large_box(box_type: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = (BASIC_HEADER_SIZE + LARGE_SIZE_FIELD_SIZE + payload.len()) as u64;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&1_u32.to_be_bytes());
        out.extend_from_slice(&box_type);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn make_uuid_box(uuid: [u8; 16], payload: &[u8]) -> Vec<u8> {
        let size = (BASIC_HEADER_SIZE + UUID_EXTENDED_TYPE_SIZE + payload.len()) as u32;
        let mut out = Vec::with_capacity(size as usize);
        out.extend_from_slice(&size.to_be_bytes());
        out.extend_from_slice(b"uuid");
        out.extend_from_slice(&uuid);
        out.extend_from_slice(payload);
        out
    }
}
