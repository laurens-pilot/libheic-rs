use std::error::Error;
use std::fmt::{Display, Formatter};
use std::mem::size_of;

const BASIC_HEADER_SIZE: usize = 8;
const LARGE_SIZE_FIELD_SIZE: usize = 8;
const UUID_EXTENDED_TYPE_SIZE: usize = 16;
const UUID_BOX_TYPE: [u8; 4] = *b"uuid";
const FTYP_BOX_TYPE: [u8; 4] = *b"ftyp";
const META_BOX_TYPE: [u8; 4] = *b"meta";
const PITM_BOX_TYPE: [u8; 4] = *b"pitm";
const ILOC_BOX_TYPE: [u8; 4] = *b"iloc";
const IINF_BOX_TYPE: [u8; 4] = *b"iinf";
const INFE_BOX_TYPE: [u8; 4] = *b"infe";
const INFE_ITEM_TYPE_MIME: [u8; 4] = *b"mime";
const INFE_ITEM_TYPE_URI: [u8; 4] = *b"uri ";
const FTYP_FIXED_FIELDS_SIZE: usize = 8;
const BRAND_FIELD_SIZE: usize = 4;
const FULL_BOX_HEADER_SIZE: usize = 4;

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

impl<'a> ParsedBox<'a> {
    pub fn payload_offset(&self) -> u64 {
        self.offset + u64::from(self.header.header_size)
    }

    /// Iterate immediate child boxes inside this box payload.
    pub fn children(&self) -> BoxIter<'a> {
        // Provenance: mirrors libheif child parsing in
        // libheif/libheif/box.cc:Box::read_children, where child reads are
        // range-limited to the parent payload and keep absolute offsets.
        BoxIter::with_offset(self.payload, self.payload_offset())
    }

    pub fn parse_children(&self) -> Result<Vec<ParsedBox<'a>>, ParseBoxError> {
        self.children().collect()
    }

    /// Parse this box payload as an `ftyp` box.
    pub fn parse_ftyp(&self) -> Result<FileTypeBox, ParseFileTypeBoxError> {
        if self.header.box_type.as_bytes() != FTYP_BOX_TYPE {
            return Err(ParseFileTypeBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_ftyp_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as a FullBox header.
    pub fn parse_full_box_header(&self) -> Result<FullBoxHeader, ParseFullBoxError> {
        parse_full_box_payload(self.payload, self.payload_offset()).map(|(full_box, _, _)| full_box)
    }

    /// Parse this box payload as a `meta` box.
    pub fn parse_meta(&self) -> Result<MetaBox<'a>, ParseMetaBoxError> {
        if self.header.box_type.as_bytes() != META_BOX_TYPE {
            return Err(ParseMetaBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        // Provenance: mirrors libheif's FullBox + meta parse sequence in
        // libheif/libheif/box.cc:FullBox::parse_full_box_header and
        // libheif/libheif/box.cc:Box_meta::parse.
        let (full_box, meta_payload, meta_payload_offset) =
            parse_full_box_payload(self.payload, self.payload_offset())?;
        if full_box.version != 0 {
            return Err(ParseMetaBoxError::UnsupportedVersion {
                offset: self.payload_offset(),
                version: full_box.version,
            });
        }

        Ok(MetaBox {
            full_box,
            payload: meta_payload,
            payload_offset: meta_payload_offset,
        })
    }

    /// Parse this box payload as a `pitm` box.
    pub fn parse_pitm(&self) -> Result<PrimaryItemBox, ParsePrimaryItemBoxError> {
        if self.header.box_type.as_bytes() != PITM_BOX_TYPE {
            return Err(ParsePrimaryItemBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_pitm_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `iloc` box.
    pub fn parse_iloc(&self) -> Result<ItemLocationBox, ParseItemLocationBoxError> {
        if self.header.box_type.as_bytes() != ILOC_BOX_TYPE {
            return Err(ParseItemLocationBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_iloc_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `infe` box.
    pub fn parse_infe(&self) -> Result<ItemInfoEntryBox, ParseItemInfoEntryBoxError> {
        if self.header.box_type.as_bytes() != INFE_BOX_TYPE {
            return Err(ParseItemInfoEntryBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_infe_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `iinf` box.
    pub fn parse_iinf(&self) -> Result<ItemInfoBox, ParseItemInfoBoxError> {
        if self.header.box_type.as_bytes() != IINF_BOX_TYPE {
            return Err(ParseItemInfoBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_iinf_payload(self.payload, self.payload_offset())
    }
}

/// Parsed `ftyp` box payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTypeBox {
    pub major_brand: FourCc,
    pub minor_version: u32,
    pub compatible_brands: Vec<FourCc>,
}

/// Parsed FullBox header fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FullBoxHeader {
    pub version: u8,
    pub flags: u32,
}

/// Parsed `pitm` (primary item) payload fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimaryItemBox {
    pub full_box: FullBoxHeader,
    pub item_id: u32,
}

/// Parsed `iloc` extent entry fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ItemLocationExtent {
    pub index: u64,
    pub offset: u64,
    pub length: u64,
}

/// Parsed `iloc` item entry fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemLocationItem {
    pub item_id: u32,
    pub construction_method: u8,
    pub data_reference_index: u16,
    pub base_offset: u64,
    pub extents: Vec<ItemLocationExtent>,
}

/// Parsed `iloc` payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemLocationBox {
    pub full_box: FullBoxHeader,
    pub offset_size: u8,
    pub length_size: u8,
    pub base_offset_size: u8,
    pub index_size: u8,
    pub items: Vec<ItemLocationItem>,
}

/// Parsed `infe` (item info entry) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInfoEntryBox {
    pub full_box: FullBoxHeader,
    pub item_id: u32,
    pub item_protection_index: u16,
    pub item_type: Option<FourCc>,
    pub item_name: Vec<u8>,
    pub content_type: Option<Vec<u8>>,
    pub content_encoding: Option<Vec<u8>>,
    pub item_uri_type: Option<Vec<u8>>,
    pub hidden_item: bool,
}

/// Parsed `iinf` (item info) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemInfoBox {
    pub full_box: FullBoxHeader,
    pub item_count: u32,
    pub entries: Vec<ItemInfoEntryBox>,
}

/// Parsed `meta` payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MetaBox<'a> {
    pub full_box: FullBoxHeader,
    pub payload: &'a [u8],
    pub payload_offset: u64,
}

impl<'a> MetaBox<'a> {
    pub fn children(&self) -> BoxIter<'a> {
        BoxIter::with_offset(self.payload, self.payload_offset)
    }

    pub fn parse_children(&self) -> Result<Vec<ParsedBox<'a>>, ParseBoxError> {
        self.children().collect()
    }
}

/// Errors returned when parsing an `ftyp` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseFileTypeBoxError {
    UnexpectedBoxType { offset: u64, actual: FourCc },
    PayloadTooSmall { offset: u64, available: usize },
    IncompleteCompatibleBrand { offset: u64, bytes: usize },
}

impl Display for ParseFileTypeBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseFileTypeBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected ftyp box at offset {offset}, got box type {actual}"
            ),
            ParseFileTypeBoxError::PayloadTooSmall { offset, available } => write!(
                f,
                "ftyp payload too small at offset {offset} (available: {available} bytes)"
            ),
            ParseFileTypeBoxError::IncompleteCompatibleBrand { offset, bytes } => write!(
                f,
                "ftyp compatible brands field has trailing bytes at offset {offset} (remaining bytes: {bytes})"
            ),
        }
    }
}

impl Error for ParseFileTypeBoxError {}

/// Errors returned when parsing a FullBox header.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseFullBoxError {
    PayloadTooSmall { offset: u64, available: usize },
}

impl Display for ParseFullBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseFullBoxError::PayloadTooSmall { offset, available } => write!(
                f,
                "full box payload too small at offset {offset} (available: {available} bytes)"
            ),
        }
    }
}

impl Error for ParseFullBoxError {}

/// Errors returned when parsing a `meta` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseMetaBoxError {
    UnexpectedBoxType { offset: u64, actual: FourCc },
    FullBox(ParseFullBoxError),
    UnsupportedVersion { offset: u64, version: u8 },
}

impl Display for ParseMetaBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseMetaBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected meta box at offset {offset}, got box type {actual}"
            ),
            ParseMetaBoxError::FullBox(err) => write!(f, "{err}"),
            ParseMetaBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "meta box at offset {offset} has unsupported full box version {version}"
            ),
        }
    }
}

impl Error for ParseMetaBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseMetaBoxError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseMetaBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing a `pitm` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsePrimaryItemBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    FullBox(ParseFullBoxError),
    UnsupportedVersion {
        offset: u64,
        version: u8,
    },
    PayloadTooSmallForItemId {
        offset: u64,
        version: u8,
        available: usize,
        required: usize,
    },
}

impl Display for ParsePrimaryItemBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsePrimaryItemBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected pitm box at offset {offset}, got box type {actual}"
            ),
            ParsePrimaryItemBoxError::FullBox(err) => write!(f, "{err}"),
            ParsePrimaryItemBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "pitm box at offset {offset} has unsupported full box version {version}"
            ),
            ParsePrimaryItemBoxError::PayloadTooSmallForItemId {
                offset,
                version,
                available,
                required,
            } => write!(
                f,
                "pitm version {version} item_ID field too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParsePrimaryItemBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParsePrimaryItemBoxError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParsePrimaryItemBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Size descriptor fields in an `iloc` box header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemLocationField {
    Offset,
    Length,
    BaseOffset,
    Index,
}

impl Display for ItemLocationField {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ItemLocationField::Offset => write!(f, "offset"),
            ItemLocationField::Length => write!(f, "length"),
            ItemLocationField::BaseOffset => write!(f, "base_offset"),
            ItemLocationField::Index => write!(f, "index"),
        }
    }
}

/// Errors returned when parsing an `iloc` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemLocationBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    FullBox(ParseFullBoxError),
    UnsupportedVersion {
        offset: u64,
        version: u8,
    },
    UnsupportedFieldSize {
        offset: u64,
        field: ItemLocationField,
        size: u8,
    },
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
}

impl Display for ParseItemLocationBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemLocationBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected iloc box at offset {offset}, got box type {actual}"
            ),
            ParseItemLocationBoxError::FullBox(err) => write!(f, "{err}"),
            ParseItemLocationBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "iloc box at offset {offset} has unsupported full box version {version}"
            ),
            ParseItemLocationBoxError::UnsupportedFieldSize {
                offset,
                field,
                size,
            } => write!(
                f,
                "iloc {field}_size field has unsupported size {size} at offset {offset}"
            ),
            ParseItemLocationBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "iloc payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParseItemLocationBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemLocationBoxError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseItemLocationBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing an `infe` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemInfoEntryBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    FullBox(ParseFullBoxError),
    UnsupportedVersion {
        offset: u64,
        version: u8,
    },
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
}

impl Display for ParseItemInfoEntryBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemInfoEntryBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected infe box at offset {offset}, got box type {actual}"
            ),
            ParseItemInfoEntryBoxError::FullBox(err) => write!(f, "{err}"),
            ParseItemInfoEntryBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "infe box at offset {offset} has unsupported full box version {version}"
            ),
            ParseItemInfoEntryBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "infe payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParseItemInfoEntryBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemInfoEntryBoxError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseItemInfoEntryBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing an `iinf` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemInfoBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    FullBox(ParseFullBoxError),
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
    EntryCountTooLarge {
        offset: u64,
        item_count: u32,
    },
    ChildBox(ParseBoxError),
    DeclaredEntryCountMismatch {
        offset: u64,
        declared: u32,
        parsed: usize,
    },
    UnexpectedEntryBoxType {
        offset: u64,
        actual: FourCc,
    },
    Entry(ParseItemInfoEntryBoxError),
}

impl Display for ParseItemInfoBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemInfoBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected iinf box at offset {offset}, got box type {actual}"
            ),
            ParseItemInfoBoxError::FullBox(err) => write!(f, "{err}"),
            ParseItemInfoBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "iinf payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseItemInfoBoxError::EntryCountTooLarge { offset, item_count } => write!(
                f,
                "iinf item_count {item_count} cannot be represented at offset {offset}"
            ),
            ParseItemInfoBoxError::ChildBox(err) => write!(f, "{err}"),
            ParseItemInfoBoxError::DeclaredEntryCountMismatch {
                offset,
                declared,
                parsed,
            } => write!(
                f,
                "iinf declared {declared} item info entries but parsed {parsed} entries at offset {offset}"
            ),
            ParseItemInfoBoxError::UnexpectedEntryBoxType { offset, actual } => write!(
                f,
                "expected infe child box in iinf at offset {offset}, got box type {actual}"
            ),
            ParseItemInfoBoxError::Entry(err) => write!(f, "{err}"),
        }
    }
}

impl Error for ParseItemInfoBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemInfoBoxError::FullBox(err) => Some(err),
            ParseItemInfoBoxError::ChildBox(err) => Some(err),
            ParseItemInfoBoxError::Entry(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseItemInfoBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

impl From<ParseBoxError> for ParseItemInfoBoxError {
    fn from(value: ParseBoxError) -> Self {
        Self::ChildBox(value)
    }
}

impl From<ParseItemInfoEntryBoxError> for ParseItemInfoBoxError {
    fn from(value: ParseItemInfoEntryBoxError) -> Self {
        Self::Entry(value)
    }
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

/// Iterate BMFF boxes from an input slice.
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

fn parse_ftyp_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<FileTypeBox, ParseFileTypeBoxError> {
    // Provenance: mirrors the ftyp field layout and brand iteration used in
    // libheif/libheif/box.cc:Box_ftyp::parse.
    if payload.len() < FTYP_FIXED_FIELDS_SIZE {
        return Err(ParseFileTypeBoxError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
        });
    }

    let major_brand = read_fourcc(&payload[0..4]);
    let minor_version = read_u32_be(&payload[4..8]);
    let compatible_brand_bytes = &payload[8..];
    let remainder = compatible_brand_bytes.len() % BRAND_FIELD_SIZE;
    if remainder != 0 {
        return Err(ParseFileTypeBoxError::IncompleteCompatibleBrand {
            offset: payload_offset + FTYP_FIXED_FIELDS_SIZE as u64,
            bytes: compatible_brand_bytes.len(),
        });
    }

    let compatible_brands = compatible_brand_bytes
        .chunks_exact(BRAND_FIELD_SIZE)
        .map(read_fourcc)
        .collect();

    Ok(FileTypeBox {
        major_brand,
        minor_version,
        compatible_brands,
    })
}

fn parse_full_box_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<(FullBoxHeader, &[u8], u64), ParseFullBoxError> {
    if payload.len() < FULL_BOX_HEADER_SIZE {
        return Err(ParseFullBoxError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
        });
    }

    let data = read_u32_be(&payload[0..FULL_BOX_HEADER_SIZE]);
    let full_box = FullBoxHeader {
        version: (data >> 24) as u8,
        flags: data & 0x00FF_FFFF,
    };

    Ok((
        full_box,
        &payload[FULL_BOX_HEADER_SIZE..],
        payload_offset + FULL_BOX_HEADER_SIZE as u64,
    ))
}

fn parse_pitm_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<PrimaryItemBox, ParsePrimaryItemBoxError> {
    // Provenance: mirrors libheif pitm parsing in
    // libheif/libheif/box.cc:Box_pitm::parse, where version 0 uses a 16-bit
    // item_ID, version 1 uses a 32-bit item_ID, and versions >1 are rejected.
    let (full_box, pitm_payload, pitm_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    let item_id = match full_box.version {
        0 => {
            let required = size_of::<u16>();
            if pitm_payload.len() < required {
                return Err(ParsePrimaryItemBoxError::PayloadTooSmallForItemId {
                    offset: pitm_payload_offset,
                    version: full_box.version,
                    available: pitm_payload.len(),
                    required,
                });
            }
            u32::from(read_u16_be(&pitm_payload[..required]))
        }
        1 => {
            let required = size_of::<u32>();
            if pitm_payload.len() < required {
                return Err(ParsePrimaryItemBoxError::PayloadTooSmallForItemId {
                    offset: pitm_payload_offset,
                    version: full_box.version,
                    available: pitm_payload.len(),
                    required,
                });
            }
            read_u32_be(&pitm_payload[..required])
        }
        version => {
            return Err(ParsePrimaryItemBoxError::UnsupportedVersion {
                offset: payload_offset,
                version,
            });
        }
    };

    Ok(PrimaryItemBox { full_box, item_id })
}

fn parse_iloc_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ItemLocationBox, ParseItemLocationBoxError> {
    // Provenance: mirrors iloc parsing flow in libheif/libheif/box.cc:Box_iloc::parse
    // (supported versions, size descriptor fields, per-item records, and extents)
    // while adding explicit field-size validation and truncation errors.
    let (full_box, iloc_payload, iloc_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version > 2 {
        return Err(ParseItemLocationBoxError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    let mut cursor = 0_usize;
    let size_fields = read_u16_cursor(
        iloc_payload,
        &mut cursor,
        iloc_payload_offset,
        "size descriptor",
    )?;
    let offset_size = ((size_fields >> 12) & 0xF) as u8;
    let length_size = ((size_fields >> 8) & 0xF) as u8;
    let base_offset_size = ((size_fields >> 4) & 0xF) as u8;
    let index_size = if full_box.version >= 1 {
        (size_fields & 0xF) as u8
    } else {
        0
    };

    validate_iloc_size_field(iloc_payload_offset, ItemLocationField::Offset, offset_size)?;
    validate_iloc_size_field(iloc_payload_offset, ItemLocationField::Length, length_size)?;
    validate_iloc_size_field(
        iloc_payload_offset,
        ItemLocationField::BaseOffset,
        base_offset_size,
    )?;
    if full_box.version >= 1 {
        validate_iloc_size_field(iloc_payload_offset, ItemLocationField::Index, index_size)?;
    }

    let item_count = if full_box.version < 2 {
        u32::from(read_u16_cursor(
            iloc_payload,
            &mut cursor,
            iloc_payload_offset,
            "item_count",
        )?)
    } else {
        read_u32_cursor(iloc_payload, &mut cursor, iloc_payload_offset, "item_count")?
    };

    let mut items = Vec::with_capacity(item_count as usize);
    for _ in 0..item_count {
        let item_id = if full_box.version < 2 {
            u32::from(read_u16_cursor(
                iloc_payload,
                &mut cursor,
                iloc_payload_offset,
                "item_ID",
            )?)
        } else {
            read_u32_cursor(iloc_payload, &mut cursor, iloc_payload_offset, "item_ID")?
        };

        let construction_method = if full_box.version >= 1 {
            (read_u16_cursor(
                iloc_payload,
                &mut cursor,
                iloc_payload_offset,
                "construction_method",
            )? & 0xF) as u8
        } else {
            0
        };
        let data_reference_index = read_u16_cursor(
            iloc_payload,
            &mut cursor,
            iloc_payload_offset,
            "data_reference_index",
        )?;
        let base_offset = read_iloc_sized_u64(
            iloc_payload,
            &mut cursor,
            iloc_payload_offset,
            base_offset_size,
            ItemLocationField::BaseOffset,
        )?;
        let extent_count = read_u16_cursor(
            iloc_payload,
            &mut cursor,
            iloc_payload_offset,
            "extent_count",
        )?;

        let mut extents = Vec::with_capacity(usize::from(extent_count));
        for _ in 0..extent_count {
            let index = if full_box.version >= 1 && index_size > 0 {
                read_iloc_sized_u64(
                    iloc_payload,
                    &mut cursor,
                    iloc_payload_offset,
                    index_size,
                    ItemLocationField::Index,
                )?
            } else {
                0
            };
            let offset = read_iloc_sized_u64(
                iloc_payload,
                &mut cursor,
                iloc_payload_offset,
                offset_size,
                ItemLocationField::Offset,
            )?;
            let length = read_iloc_sized_u64(
                iloc_payload,
                &mut cursor,
                iloc_payload_offset,
                length_size,
                ItemLocationField::Length,
            )?;
            extents.push(ItemLocationExtent {
                index,
                offset,
                length,
            });
        }

        items.push(ItemLocationItem {
            item_id,
            construction_method,
            data_reference_index,
            base_offset,
            extents,
        });
    }

    Ok(ItemLocationBox {
        full_box,
        offset_size,
        length_size,
        base_offset_size,
        index_size,
        items,
    })
}

fn parse_infe_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ItemInfoEntryBox, ParseItemInfoEntryBoxError> {
    // Provenance: mirrors infe field parsing in
    // libheif/libheif/box.cc:Box_infe::parse, including supported version
    // handling (0..=3), hidden-item flag decoding for v2+, and conditional
    // MIME/URI string fields based on item_type.
    let (full_box, infe_payload, infe_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version > 3 {
        return Err(ParseItemInfoEntryBoxError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    let mut cursor = 0_usize;
    let item_id;
    let item_protection_index;
    let mut item_type = None;
    let item_name;
    let mut content_type = None;
    let mut content_encoding = None;
    let mut item_uri_type = None;
    let hidden_item;

    if full_box.version <= 1 {
        item_id = u32::from(read_u16_cursor_infe(
            infe_payload,
            &mut cursor,
            infe_payload_offset,
            "item_ID",
        )?);
        item_protection_index = read_u16_cursor_infe(
            infe_payload,
            &mut cursor,
            infe_payload_offset,
            "item_protection_index",
        )?;
        item_name = read_c_string_cursor(infe_payload, &mut cursor);
        content_type = Some(read_c_string_cursor(infe_payload, &mut cursor));
        content_encoding = Some(read_c_string_cursor(infe_payload, &mut cursor));
        hidden_item = false;
    } else {
        hidden_item = (full_box.flags & 0x1) != 0;
        item_id = if full_box.version == 2 {
            u32::from(read_u16_cursor_infe(
                infe_payload,
                &mut cursor,
                infe_payload_offset,
                "item_ID",
            )?)
        } else {
            read_u32_cursor_infe(infe_payload, &mut cursor, infe_payload_offset, "item_ID")?
        };
        item_protection_index = read_u16_cursor_infe(
            infe_payload,
            &mut cursor,
            infe_payload_offset,
            "item_protection_index",
        )?;
        let parsed_item_type =
            read_fourcc_cursor_infe(infe_payload, &mut cursor, infe_payload_offset, "item_type")?;
        item_type = Some(parsed_item_type);
        item_name = read_c_string_cursor(infe_payload, &mut cursor);
        if parsed_item_type.as_bytes() == INFE_ITEM_TYPE_MIME {
            content_type = Some(read_c_string_cursor(infe_payload, &mut cursor));
            content_encoding = Some(read_c_string_cursor(infe_payload, &mut cursor));
        } else if parsed_item_type.as_bytes() == INFE_ITEM_TYPE_URI {
            item_uri_type = Some(read_c_string_cursor(infe_payload, &mut cursor));
        }
    }

    Ok(ItemInfoEntryBox {
        full_box,
        item_id,
        item_protection_index,
        item_type,
        item_name,
        content_type,
        content_encoding,
        item_uri_type,
        hidden_item,
    })
}

fn parse_iinf_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ItemInfoBox, ParseItemInfoBoxError> {
    // Provenance: mirrors iinf parsing flow in libheif/libheif/box.cc:Box_iinf::parse,
    // where entry_count width is 16-bit for v0 and 32-bit for v1+.
    let (full_box, iinf_payload, iinf_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    let mut cursor = 0_usize;
    let item_count = if full_box.version == 0 {
        u32::from(read_u16_cursor_iinf(
            iinf_payload,
            &mut cursor,
            iinf_payload_offset,
            "item_count",
        )?)
    } else {
        read_u32_cursor_iinf(iinf_payload, &mut cursor, iinf_payload_offset, "item_count")?
    };
    let declared_item_count =
        usize::try_from(item_count).map_err(|_| ParseItemInfoBoxError::EntryCountTooLarge {
            offset: iinf_payload_offset,
            item_count,
        })?;

    let entries_payload = &iinf_payload[cursor..];
    let entries_payload_offset = iinf_payload_offset + cursor as u64;
    let child_boxes: Vec<ParsedBox<'_>> =
        BoxIter::with_offset(entries_payload, entries_payload_offset)
            .collect::<Result<Vec<_>, ParseBoxError>>()?;
    if child_boxes.len() != declared_item_count {
        return Err(ParseItemInfoBoxError::DeclaredEntryCountMismatch {
            offset: entries_payload_offset,
            declared: item_count,
            parsed: child_boxes.len(),
        });
    }

    let mut entries = Vec::with_capacity(child_boxes.len());
    for child in child_boxes {
        if child.header.box_type.as_bytes() != INFE_BOX_TYPE {
            return Err(ParseItemInfoBoxError::UnexpectedEntryBoxType {
                offset: child.offset,
                actual: child.header.box_type,
            });
        }
        entries.push(parse_infe_payload(child.payload, child.payload_offset())?);
    }

    Ok(ItemInfoBox {
        full_box,
        item_count,
        entries,
    })
}

fn validate_iloc_size_field(
    offset: u64,
    field: ItemLocationField,
    size: u8,
) -> Result<(), ParseItemLocationBoxError> {
    if matches!(size, 0 | 4 | 8) {
        return Ok(());
    }

    Err(ParseItemLocationBoxError::UnsupportedFieldSize {
        offset,
        field,
        size,
    })
}

fn read_iloc_sized_u64(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    size: u8,
    field: ItemLocationField,
) -> Result<u64, ParseItemLocationBoxError> {
    match size {
        0 => Ok(0),
        4 => Ok(u64::from(read_u32_cursor(
            payload,
            cursor,
            payload_offset,
            iloc_field_context(field),
        )?)),
        8 => read_u64_cursor(payload, cursor, payload_offset, iloc_field_context(field)),
        _ => Err(ParseItemLocationBoxError::UnsupportedFieldSize {
            offset: payload_offset + *cursor as u64,
            field,
            size,
        }),
    }
}

fn iloc_field_context(field: ItemLocationField) -> &'static str {
    match field {
        ItemLocationField::Offset => "extent_offset",
        ItemLocationField::Length => "extent_length",
        ItemLocationField::BaseOffset => "base_offset",
        ItemLocationField::Index => "extent_index",
    }
}

fn read_u16_cursor_infe(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseItemInfoEntryBoxError> {
    let bytes = take_cursor_bytes_infe(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor_infe(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseItemInfoEntryBoxError> {
    let bytes = take_cursor_bytes_infe(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn read_fourcc_cursor_infe(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<FourCc, ParseItemInfoEntryBoxError> {
    let bytes = take_cursor_bytes_infe(payload, cursor, BRAND_FIELD_SIZE, payload_offset, context)?;
    Ok(read_fourcc(bytes))
}

fn take_cursor_bytes_infe<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseItemInfoEntryBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseItemInfoEntryBoxError::PayloadTooSmall {
            offset: payload_offset + start as u64,
            context,
            available,
            required: size,
        });
    }

    let end = start + size;
    *cursor = end;
    Ok(&payload[start..end])
}

fn read_u16_cursor_iinf(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseItemInfoBoxError> {
    let bytes = take_cursor_bytes_iinf(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor_iinf(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseItemInfoBoxError> {
    let bytes = take_cursor_bytes_iinf(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn take_cursor_bytes_iinf<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseItemInfoBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseItemInfoBoxError::PayloadTooSmall {
            offset: payload_offset + start as u64,
            context,
            available,
            required: size,
        });
    }

    let end = start + size;
    *cursor = end;
    Ok(&payload[start..end])
}

fn read_c_string_cursor(payload: &[u8], cursor: &mut usize) -> Vec<u8> {
    if *cursor >= payload.len() {
        return Vec::new();
    }

    let start = *cursor;
    let tail = &payload[start..];
    if let Some(terminator) = tail.iter().position(|byte| *byte == 0) {
        let end = start + terminator;
        *cursor = end + 1;
        payload[start..end].to_vec()
    } else {
        *cursor = payload.len();
        payload[start..].to_vec()
    }
}

fn read_u16_cursor(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseItemLocationBoxError> {
    let bytes = take_cursor_bytes(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseItemLocationBoxError> {
    let bytes = take_cursor_bytes(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn read_u64_cursor(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u64, ParseItemLocationBoxError> {
    let bytes = take_cursor_bytes(payload, cursor, size_of::<u64>(), payload_offset, context)?;
    Ok(read_u64_be(bytes))
}

fn take_cursor_bytes<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseItemLocationBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseItemLocationBoxError::PayloadTooSmall {
            offset: payload_offset + start as u64,
            context,
            available,
            required: size,
        });
    }

    let end = start + size;
    *cursor = end;
    Ok(&payload[start..end])
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

fn read_u16_be(input: &[u8]) -> u16 {
    u16::from_be_bytes([input[0], input[1]])
}

fn read_fourcc(input: &[u8]) -> FourCc {
    FourCc::new([input[0], input[1], input[2], input[3]])
}

fn read_u64_be(input: &[u8]) -> u64 {
    u64::from_be_bytes([
        input[0], input[1], input[2], input[3], input[4], input[5], input[6], input[7],
    ])
}

#[cfg(test)]
mod tests {
    use super::{
        parse_boxes, BoxIter, FourCc, ItemLocationField, ParseBoxError, ParseFileTypeBoxError,
        ParseFullBoxError, ParseItemInfoBoxError, ParseItemInfoEntryBoxError,
        ParseItemLocationBoxError, ParseMetaBoxError, ParsePrimaryItemBoxError, BASIC_HEADER_SIZE,
        FULL_BOX_HEADER_SIZE, LARGE_SIZE_FIELD_SIZE, UUID_EXTENDED_TYPE_SIZE,
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

    #[test]
    fn parses_nested_child_boxes_inside_parent_payload() {
        let child_a = make_basic_box(*b"hdlr", &[0x00, 0x00, 0x00, 0x00]);
        let child_b = make_basic_box(*b"pitm", &[0x01, 0x02]);
        let mut meta_payload = Vec::new();
        meta_payload.extend_from_slice(&child_a);
        meta_payload.extend_from_slice(&child_b);
        let bytes = make_basic_box(*b"meta", &meta_payload);

        let top_level = parse_boxes(&bytes).expect("top-level box should parse");
        let children = top_level[0]
            .parse_children()
            .expect("child boxes should parse");
        assert_eq!(children.len(), 2);
        assert_eq!(children[0].header.box_type.as_bytes(), *b"hdlr");
        assert_eq!(children[1].header.box_type.as_bytes(), *b"pitm");
        assert_eq!(children[0].offset, 8);
        assert_eq!(children[1].offset, 8 + child_a.len() as u64);
    }

    #[test]
    fn rejects_child_box_past_parent_payload_range() {
        let mut invalid_child = Vec::new();
        invalid_child.extend_from_slice(&16_u32.to_be_bytes());
        invalid_child.extend_from_slice(b"hdlr");
        invalid_child.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let bytes = make_basic_box(*b"meta", &invalid_child);

        let top_level = parse_boxes(&bytes).expect("parent box should parse");
        let err = top_level[0]
            .parse_children()
            .expect_err("out-of-range child must fail");
        assert_eq!(
            err,
            ParseBoxError::BoxOutOfBounds {
                offset: 8,
                box_size: 16,
                available: 12,
            }
        );
    }

    #[test]
    fn size_zero_child_box_consumes_remaining_parent_payload() {
        let mut meta_payload = Vec::new();
        meta_payload.extend_from_slice(&0_u32.to_be_bytes());
        meta_payload.extend_from_slice(b"free");
        meta_payload.extend_from_slice(&[0x10, 0x11, 0x12]);
        meta_payload.extend_from_slice(&12_u32.to_be_bytes());
        meta_payload.extend_from_slice(b"skip");
        meta_payload.extend_from_slice(&[0x20, 0x21, 0x22, 0x23]);
        let bytes = make_basic_box(*b"meta", &meta_payload);

        let top_level = parse_boxes(&bytes).expect("parent box should parse");
        let mut children = top_level[0].children();
        let first = children
            .next()
            .expect("first child result should exist")
            .expect("size=0 child should parse");
        assert_eq!(first.header.box_type.as_bytes(), *b"free");
        assert_eq!(first.header.box_size, meta_payload.len() as u64);
        assert_eq!(first.offset, 8);
        assert_eq!(first.payload, &meta_payload[8..]);
        assert!(children.next().is_none());
    }

    #[test]
    fn parses_ftyp_payload_fields() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"mif1");
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(b"miaf");
        payload.extend_from_slice(b"avif");
        let bytes = make_basic_box(*b"ftyp", &payload);

        let top_level = parse_boxes(&bytes).expect("ftyp box should parse");
        let ftyp = top_level[0]
            .parse_ftyp()
            .expect("ftyp payload should parse");
        assert_eq!(ftyp.major_brand.as_bytes(), *b"mif1");
        assert_eq!(ftyp.minor_version, 0);
        assert_eq!(
            ftyp.compatible_brands,
            vec![FourCc::new(*b"miaf"), FourCc::new(*b"avif")]
        );
    }

    #[test]
    fn rejects_ftyp_payload_smaller_than_fixed_fields() {
        let bytes = make_basic_box(*b"ftyp", &[0, 1, 2, 3, 4, 5, 6]);
        let top_level = parse_boxes(&bytes).expect("ftyp box should parse");

        let err = top_level[0]
            .parse_ftyp()
            .expect_err("short ftyp payload must fail");
        assert_eq!(
            err,
            ParseFileTypeBoxError::PayloadTooSmall {
                offset: 8,
                available: 7
            }
        );
    }

    #[test]
    fn rejects_ftyp_payload_with_incomplete_compatible_brand() {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"mif1");
        payload.extend_from_slice(&0_u32.to_be_bytes());
        payload.extend_from_slice(&[0xaa, 0xbb, 0xcc]);
        let bytes = make_basic_box(*b"ftyp", &payload);
        let top_level = parse_boxes(&bytes).expect("ftyp box should parse");

        let err = top_level[0]
            .parse_ftyp()
            .expect_err("incomplete compatible brand must fail");
        assert_eq!(
            err,
            ParseFileTypeBoxError::IncompleteCompatibleBrand {
                offset: 16,
                bytes: 3
            }
        );
    }

    #[test]
    fn rejects_ftyp_parse_for_non_ftyp_box() {
        let bytes = make_basic_box(*b"free", &[0x01, 0x02, 0x03, 0x04]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_ftyp()
            .expect_err("parsing non-ftyp as ftyp must fail");
        assert_eq!(
            err,
            ParseFileTypeBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free")
            }
        );
    }

    #[test]
    fn parses_full_box_header_fields() {
        let payload = [0x01, 0x02, 0x03, 0x04, 0xaa];
        let bytes = make_basic_box(*b"meta", &payload);
        let top_level = parse_boxes(&bytes).expect("meta box should parse");

        let full_box = top_level[0]
            .parse_full_box_header()
            .expect("full box header should parse");
        assert_eq!(full_box.version, 1);
        assert_eq!(full_box.flags, 0x0002_0304);
    }

    #[test]
    fn rejects_full_box_header_when_payload_too_small() {
        let bytes = make_basic_box(*b"meta", &[0x01, 0x02, 0x03]);
        let top_level = parse_boxes(&bytes).expect("meta box should parse");

        let err = top_level[0]
            .parse_full_box_header()
            .expect_err("short full box payload must fail");
        assert_eq!(
            err,
            ParseFullBoxError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 3
            }
        );
    }

    #[test]
    fn parses_meta_full_box_and_children() {
        let child = make_basic_box(*b"hdlr", &[0x00, 0x00, 0x00, 0x00]);
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&child);
        let bytes = make_basic_box(*b"meta", &payload);

        let top_level = parse_boxes(&bytes).expect("meta box should parse");
        let meta = top_level[0]
            .parse_meta()
            .expect("meta payload should parse");
        assert_eq!(meta.full_box.version, 0);
        assert_eq!(meta.full_box.flags, 0);
        assert_eq!(
            meta.payload_offset,
            (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64
        );

        let children = meta.parse_children().expect("meta children should parse");
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].header.box_type.as_bytes(), *b"hdlr");
        assert_eq!(
            children[0].offset,
            (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64
        );
    }

    #[test]
    fn rejects_meta_parse_for_non_meta_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_meta()
            .expect_err("parsing non-meta as meta must fail");
        assert_eq!(
            err,
            ParseMetaBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free")
            }
        );
    }

    #[test]
    fn rejects_meta_parse_for_unsupported_full_box_version() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        payload.extend_from_slice(&make_basic_box(*b"hdlr", &[0x00, 0x00, 0x00, 0x00]));
        let bytes = make_basic_box(*b"meta", &payload);
        let top_level = parse_boxes(&bytes).expect("meta box should parse");

        let err = top_level[0]
            .parse_meta()
            .expect_err("unsupported meta version must fail");
        assert_eq!(
            err,
            ParseMetaBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 1
            }
        );
    }

    #[test]
    fn rejects_meta_parse_when_full_box_header_is_truncated() {
        let bytes = make_basic_box(*b"meta", &[0x01, 0x02, 0x03]);
        let top_level = parse_boxes(&bytes).expect("meta box should parse");

        let err = top_level[0]
            .parse_meta()
            .expect_err("meta with short full box header must fail");
        assert_eq!(
            err,
            ParseMetaBoxError::FullBox(ParseFullBoxError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 3
            })
        );
    }

    #[test]
    fn parses_pitm_version_zero_item_id() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&0x1234_u16.to_be_bytes());
        let bytes = make_basic_box(*b"pitm", &payload);
        let top_level = parse_boxes(&bytes).expect("pitm box should parse");

        let pitm = top_level[0]
            .parse_pitm()
            .expect("pitm v0 payload should parse");
        assert_eq!(pitm.full_box.version, 0);
        assert_eq!(pitm.full_box.flags, 0);
        assert_eq!(pitm.item_id, 0x1234);
    }

    #[test]
    fn parses_pitm_version_one_item_id() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x01]); // version=1, flags=1
        payload.extend_from_slice(&0x1234_5678_u32.to_be_bytes());
        let bytes = make_basic_box(*b"pitm", &payload);
        let top_level = parse_boxes(&bytes).expect("pitm box should parse");

        let pitm = top_level[0]
            .parse_pitm()
            .expect("pitm v1 payload should parse");
        assert_eq!(pitm.full_box.version, 1);
        assert_eq!(pitm.full_box.flags, 1);
        assert_eq!(pitm.item_id, 0x1234_5678);
    }

    #[test]
    fn rejects_pitm_parse_for_non_pitm_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_pitm()
            .expect_err("parsing non-pitm as pitm must fail");
        assert_eq!(
            err,
            ParsePrimaryItemBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free")
            }
        );
    }

    #[test]
    fn rejects_pitm_parse_for_unsupported_full_box_version() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        payload.extend_from_slice(&0x1234_5678_u32.to_be_bytes());
        let bytes = make_basic_box(*b"pitm", &payload);
        let top_level = parse_boxes(&bytes).expect("pitm box should parse");

        let err = top_level[0]
            .parse_pitm()
            .expect_err("unsupported pitm version must fail");
        assert_eq!(
            err,
            ParsePrimaryItemBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 2
            }
        );
    }

    #[test]
    fn rejects_pitm_parse_when_item_id_field_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        payload.extend_from_slice(&[0xaa, 0xbb, 0xcc]); // one byte short
        let bytes = make_basic_box(*b"pitm", &payload);
        let top_level = parse_boxes(&bytes).expect("pitm box should parse");

        let err = top_level[0]
            .parse_pitm()
            .expect_err("truncated pitm item_ID must fail");
        assert_eq!(
            err,
            ParsePrimaryItemBoxError::PayloadTooSmallForItemId {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                version: 1,
                available: 3,
                required: 4,
            }
        );
    }

    #[test]
    fn rejects_pitm_parse_when_full_box_header_is_truncated() {
        let bytes = make_basic_box(*b"pitm", &[0x01, 0x02, 0x03]);
        let top_level = parse_boxes(&bytes).expect("pitm box should parse");

        let err = top_level[0]
            .parse_pitm()
            .expect_err("pitm with short full box header must fail");
        assert_eq!(
            err,
            ParsePrimaryItemBoxError::FullBox(ParseFullBoxError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 3
            })
        );
    }

    #[test]
    fn parses_iloc_version_zero_item_and_extent() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        payload.extend_from_slice(&0x1234_u16.to_be_bytes()); // item_ID
        payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        payload.extend_from_slice(&0x0102_0304_u32.to_be_bytes()); // base_offset
        payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
        payload.extend_from_slice(&0x10_u32.to_be_bytes()); // extent_offset
        payload.extend_from_slice(&0x20_u32.to_be_bytes()); // extent_length
        let bytes = make_basic_box(*b"iloc", &payload);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let iloc = top_level[0]
            .parse_iloc()
            .expect("iloc v0 payload should parse");
        assert_eq!(iloc.full_box.version, 0);
        assert_eq!(iloc.full_box.flags, 0);
        assert_eq!(iloc.offset_size, 4);
        assert_eq!(iloc.length_size, 4);
        assert_eq!(iloc.base_offset_size, 4);
        assert_eq!(iloc.index_size, 0);
        assert_eq!(iloc.items.len(), 1);
        assert_eq!(iloc.items[0].item_id, 0x1234);
        assert_eq!(iloc.items[0].construction_method, 0);
        assert_eq!(iloc.items[0].data_reference_index, 0);
        assert_eq!(iloc.items[0].base_offset, 0x0102_0304);
        assert_eq!(iloc.items[0].extents.len(), 1);
        assert_eq!(iloc.items[0].extents[0].index, 0);
        assert_eq!(iloc.items[0].extents[0].offset, 0x10);
        assert_eq!(iloc.items[0].extents[0].length, 0x20);
    }

    #[test]
    fn parses_iloc_version_two_with_index_and_u64_fields() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        payload.extend_from_slice(&0x8884_u16.to_be_bytes()); // offset/length/base_offset=8, index=4
        payload.extend_from_slice(&1_u32.to_be_bytes()); // item_count
        payload.extend_from_slice(&0x1122_3344_u32.to_be_bytes()); // item_ID
        payload.extend_from_slice(&0x0002_u16.to_be_bytes()); // construction_method=2
        payload.extend_from_slice(&1_u16.to_be_bytes()); // data_reference_index
        payload.extend_from_slice(&0x0102_0304_0506_0708_u64.to_be_bytes()); // base_offset
        payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
        payload.extend_from_slice(&5_u32.to_be_bytes()); // extent_index
        payload.extend_from_slice(&0x10_u64.to_be_bytes()); // extent_offset
        payload.extend_from_slice(&0x20_u64.to_be_bytes()); // extent_length
        let bytes = make_basic_box(*b"iloc", &payload);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let iloc = top_level[0]
            .parse_iloc()
            .expect("iloc v2 payload should parse");
        assert_eq!(iloc.full_box.version, 2);
        assert_eq!(iloc.offset_size, 8);
        assert_eq!(iloc.length_size, 8);
        assert_eq!(iloc.base_offset_size, 8);
        assert_eq!(iloc.index_size, 4);
        assert_eq!(iloc.items.len(), 1);
        assert_eq!(iloc.items[0].item_id, 0x1122_3344);
        assert_eq!(iloc.items[0].construction_method, 2);
        assert_eq!(iloc.items[0].data_reference_index, 1);
        assert_eq!(iloc.items[0].base_offset, 0x0102_0304_0506_0708);
        assert_eq!(iloc.items[0].extents.len(), 1);
        assert_eq!(iloc.items[0].extents[0].index, 5);
        assert_eq!(iloc.items[0].extents[0].offset, 0x10);
        assert_eq!(iloc.items[0].extents[0].length, 0x20);
    }

    #[test]
    fn rejects_iloc_parse_for_non_iloc_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_iloc()
            .expect_err("parsing non-iloc as iloc must fail");
        assert_eq!(
            err,
            ParseItemLocationBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free")
            }
        );
    }

    #[test]
    fn rejects_iloc_parse_for_unsupported_full_box_version() {
        let bytes = make_basic_box(*b"iloc", &[0x03, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let err = top_level[0]
            .parse_iloc()
            .expect_err("unsupported iloc version must fail");
        assert_eq!(
            err,
            ParseItemLocationBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 3
            }
        );
    }

    #[test]
    fn rejects_iloc_parse_for_invalid_base_offset_size() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&0x4420_u16.to_be_bytes()); // base_offset_size=2 (invalid)
        let bytes = make_basic_box(*b"iloc", &payload);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let err = top_level[0]
            .parse_iloc()
            .expect_err("invalid iloc base_offset_size must fail");
        assert_eq!(
            err,
            ParseItemLocationBoxError::UnsupportedFieldSize {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                field: ItemLocationField::BaseOffset,
                size: 2,
            }
        );
    }

    #[test]
    fn rejects_iloc_parse_for_invalid_index_size_on_version_one() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        payload.extend_from_slice(&0x4442_u16.to_be_bytes()); // index_size=2 (invalid)
        let bytes = make_basic_box(*b"iloc", &payload);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let err = top_level[0]
            .parse_iloc()
            .expect_err("invalid iloc index_size must fail");
        assert_eq!(
            err,
            ParseItemLocationBoxError::UnsupportedFieldSize {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                field: ItemLocationField::Index,
                size: 2,
            }
        );
    }

    #[test]
    fn rejects_iloc_parse_when_base_offset_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        payload.extend_from_slice(&[0xaa, 0xbb]); // truncated base_offset (needs 4 bytes)
        let bytes = make_basic_box(*b"iloc", &payload);
        let top_level = parse_boxes(&bytes).expect("iloc box should parse");

        let err = top_level[0]
            .parse_iloc()
            .expect_err("truncated base_offset must fail");
        assert_eq!(
            err,
            ParseItemLocationBoxError::PayloadTooSmall {
                offset: 20,
                context: "base_offset",
                available: 2,
                required: 4,
            }
        );
    }

    #[test]
    fn parses_infe_version_two_entry_with_item_type() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x01]); // version=2, flags=1 (hidden)
        payload.extend_from_slice(&0x0042_u16.to_be_bytes()); // item_ID
        payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        payload.extend_from_slice(b"av01"); // item_type
        payload.extend_from_slice(b"primary\0"); // item_name
        let bytes = make_basic_box(*b"infe", &payload);
        let top_level = parse_boxes(&bytes).expect("infe box should parse");

        let infe = top_level[0]
            .parse_infe()
            .expect("infe v2 payload should parse");
        assert_eq!(infe.full_box.version, 2);
        assert_eq!(infe.full_box.flags, 1);
        assert_eq!(infe.item_id, 0x42);
        assert_eq!(infe.item_protection_index, 0);
        assert_eq!(infe.item_type, Some(FourCc::new(*b"av01")));
        assert_eq!(infe.item_name, b"primary".to_vec());
        assert_eq!(infe.content_type, None);
        assert_eq!(infe.content_encoding, None);
        assert_eq!(infe.item_uri_type, None);
        assert!(infe.hidden_item);
    }

    #[test]
    fn parses_infe_version_three_mime_entry_fields() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]); // version=3, flags=0
        payload.extend_from_slice(&0x1234_5678_u32.to_be_bytes()); // item_ID
        payload.extend_from_slice(&2_u16.to_be_bytes()); // item_protection_index
        payload.extend_from_slice(b"mime"); // item_type
        payload.extend_from_slice(b"Exif\0"); // item_name
        payload.extend_from_slice(b"application/rdf+xml\0"); // content_type
        payload.extend_from_slice(b"compress_zlib\0"); // content_encoding
        let bytes = make_basic_box(*b"infe", &payload);
        let top_level = parse_boxes(&bytes).expect("infe box should parse");

        let infe = top_level[0]
            .parse_infe()
            .expect("infe v3 MIME payload should parse");
        assert_eq!(infe.full_box.version, 3);
        assert_eq!(infe.item_id, 0x1234_5678);
        assert_eq!(infe.item_protection_index, 2);
        assert_eq!(infe.item_type, Some(FourCc::new(*b"mime")));
        assert_eq!(infe.item_name, b"Exif".to_vec());
        assert_eq!(infe.content_type, Some(b"application/rdf+xml".to_vec()));
        assert_eq!(infe.content_encoding, Some(b"compress_zlib".to_vec()));
        assert_eq!(infe.item_uri_type, None);
        assert!(!infe.hidden_item);
    }

    #[test]
    fn parses_infe_version_zero_legacy_fields() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&0x0203_u16.to_be_bytes()); // item_ID
        payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // item_protection_index
        payload.extend_from_slice(b"legacy\0"); // item_name
        payload.extend_from_slice(b"image/jpeg\0"); // content_type
        payload.extend_from_slice(b"gzip\0"); // content_encoding
        let bytes = make_basic_box(*b"infe", &payload);
        let top_level = parse_boxes(&bytes).expect("infe box should parse");

        let infe = top_level[0]
            .parse_infe()
            .expect("infe v0 payload should parse");
        assert_eq!(infe.full_box.version, 0);
        assert_eq!(infe.item_id, 0x0203);
        assert_eq!(infe.item_protection_index, 1);
        assert_eq!(infe.item_type, None);
        assert_eq!(infe.item_name, b"legacy".to_vec());
        assert_eq!(infe.content_type, Some(b"image/jpeg".to_vec()));
        assert_eq!(infe.content_encoding, Some(b"gzip".to_vec()));
        assert_eq!(infe.item_uri_type, None);
        assert!(!infe.hidden_item);
    }

    #[test]
    fn rejects_infe_parse_for_non_infe_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_infe()
            .expect_err("parsing non-infe as infe must fail");
        assert_eq!(
            err,
            ParseItemInfoEntryBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_infe_parse_for_unsupported_version() {
        let bytes = make_basic_box(*b"infe", &[0x04, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("infe box should parse");

        let err = top_level[0]
            .parse_infe()
            .expect_err("unsupported infe version must fail");
        assert_eq!(
            err,
            ParseItemInfoEntryBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 4,
            }
        );
    }

    #[test]
    fn rejects_infe_parse_when_item_id_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x03, 0x00, 0x00, 0x00]); // version=3, flags=0
        payload.extend_from_slice(&[0xaa, 0xbb, 0xcc]); // truncated item_ID
        let bytes = make_basic_box(*b"infe", &payload);
        let top_level = parse_boxes(&bytes).expect("infe box should parse");

        let err = top_level[0]
            .parse_infe()
            .expect_err("truncated infe item_ID must fail");
        assert_eq!(
            err,
            ParseItemInfoEntryBoxError::PayloadTooSmall {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                context: "item_ID",
                available: 3,
                required: 4,
            }
        );
    }

    #[test]
    fn parses_iinf_version_zero_with_infe_entries() {
        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x01]); // version=2, flags=1
        infe_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"av01"); // item_type
        infe_payload.extend_from_slice(b"primary\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let bytes = make_basic_box(*b"iinf", &iinf_payload);
        let top_level = parse_boxes(&bytes).expect("iinf box should parse");

        let iinf = top_level[0]
            .parse_iinf()
            .expect("iinf payload should parse");
        assert_eq!(iinf.full_box.version, 0);
        assert_eq!(iinf.item_count, 1);
        assert_eq!(iinf.entries.len(), 1);
        assert_eq!(iinf.entries[0].item_id, 1);
        assert_eq!(iinf.entries[0].item_type, Some(FourCc::new(*b"av01")));
        assert!(iinf.entries[0].hidden_item);
    }

    #[test]
    fn parses_iinf_version_one_with_u32_count() {
        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&0x0020_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"hvc1"); // item_type
        infe_payload.extend_from_slice(b"image\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        iinf_payload.extend_from_slice(&1_u32.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let bytes = make_basic_box(*b"iinf", &iinf_payload);
        let top_level = parse_boxes(&bytes).expect("iinf box should parse");

        let iinf = top_level[0]
            .parse_iinf()
            .expect("iinf v1 payload should parse");
        assert_eq!(iinf.full_box.version, 1);
        assert_eq!(iinf.item_count, 1);
        assert_eq!(iinf.entries.len(), 1);
        assert_eq!(iinf.entries[0].item_id, 0x20);
        assert_eq!(iinf.entries[0].item_type, Some(FourCc::new(*b"hvc1")));
    }

    #[test]
    fn rejects_iinf_parse_for_non_iinf_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_iinf()
            .expect_err("parsing non-iinf as iinf must fail");
        assert_eq!(
            err,
            ParseItemInfoBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_iinf_when_declared_entry_count_mismatches_payload() {
        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"av01");
        infe_payload.extend_from_slice(b"a\0");
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&2_u16.to_be_bytes()); // item_count (mismatch)
        iinf_payload.extend_from_slice(&infe); // only one entry present
        let bytes = make_basic_box(*b"iinf", &iinf_payload);
        let top_level = parse_boxes(&bytes).expect("iinf box should parse");

        let err = top_level[0]
            .parse_iinf()
            .expect_err("mismatched iinf item_count must fail");
        assert_eq!(
            err,
            ParseItemInfoBoxError::DeclaredEntryCountMismatch {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE + 2) as u64,
                declared: 2,
                parsed: 1,
            }
        );
    }

    #[test]
    fn rejects_iinf_when_child_box_type_is_not_infe() {
        let child = make_basic_box(*b"free", &[0x01, 0x02, 0x03, 0x04]);
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        payload.extend_from_slice(&child);
        let bytes = make_basic_box(*b"iinf", &payload);
        let top_level = parse_boxes(&bytes).expect("iinf box should parse");

        let err = top_level[0]
            .parse_iinf()
            .expect_err("unexpected iinf child type must fail");
        assert_eq!(
            err,
            ParseItemInfoBoxError::UnexpectedEntryBoxType {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE + 2) as u64,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_iinf_when_item_count_field_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        payload.extend_from_slice(&[0xaa, 0xbb]); // truncated 32-bit item_count
        let bytes = make_basic_box(*b"iinf", &payload);
        let top_level = parse_boxes(&bytes).expect("iinf box should parse");

        let err = top_level[0]
            .parse_iinf()
            .expect_err("truncated iinf item_count must fail");
        assert_eq!(
            err,
            ParseItemInfoBoxError::PayloadTooSmall {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                context: "item_count",
                available: 2,
                required: 4,
            }
        );
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
