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
const IREF_BOX_TYPE: [u8; 4] = *b"iref";
const IPRP_BOX_TYPE: [u8; 4] = *b"iprp";
const IPCO_BOX_TYPE: [u8; 4] = *b"ipco";
const IPMA_BOX_TYPE: [u8; 4] = *b"ipma";
const IDAT_BOX_TYPE: [u8; 4] = *b"idat";
const AV1C_BOX_TYPE: [u8; 4] = *b"av1C";
const HVCC_BOX_TYPE: [u8; 4] = *b"hvcC";
const ISPE_BOX_TYPE: [u8; 4] = *b"ispe";
const PIXI_BOX_TYPE: [u8; 4] = *b"pixi";
const COLR_BOX_TYPE: [u8; 4] = *b"colr";
const IROT_BOX_TYPE: [u8; 4] = *b"irot";
const IMIR_BOX_TYPE: [u8; 4] = *b"imir";
const CLAP_BOX_TYPE: [u8; 4] = *b"clap";
const NCLX_COLOR_TYPE: [u8; 4] = *b"nclx";
const NCLC_COLOR_TYPE: [u8; 4] = *b"nclc";
const PROF_COLOR_TYPE: [u8; 4] = *b"prof";
const RICC_COLOR_TYPE: [u8; 4] = *b"rICC";
const INFE_ITEM_TYPE_MIME: [u8; 4] = *b"mime";
const INFE_ITEM_TYPE_URI: [u8; 4] = *b"uri ";
const AV01_ITEM_TYPE: [u8; 4] = *b"av01";
const HVC1_ITEM_TYPE: [u8; 4] = *b"hvc1";
const HEV1_ITEM_TYPE: [u8; 4] = *b"hev1";
const GRID_ITEM_TYPE: [u8; 4] = *b"grid";
const DIMG_REFERENCE_TYPE: [u8; 4] = *b"dimg";
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

    /// Parse this box payload as an `iref` box.
    pub fn parse_iref(&self) -> Result<ItemReferenceBox, ParseItemReferenceBoxError> {
        if self.header.box_type.as_bytes() != IREF_BOX_TYPE {
            return Err(ParseItemReferenceBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_iref_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `ipco` box.
    pub fn parse_ipco(
        &self,
    ) -> Result<ItemPropertyContainerBox<'a>, ParseItemPropertyContainerBoxError> {
        if self.header.box_type.as_bytes() != IPCO_BOX_TYPE {
            return Err(ParseItemPropertyContainerBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_ipco_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `ipma` box.
    pub fn parse_ipma(
        &self,
    ) -> Result<ItemPropertyAssociationBox, ParseItemPropertyAssociationBoxError> {
        if self.header.box_type.as_bytes() != IPMA_BOX_TYPE {
            return Err(ParseItemPropertyAssociationBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_ipma_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `iprp` box.
    pub fn parse_iprp(&self) -> Result<ItemPropertiesBox<'a>, ParseItemPropertiesBoxError> {
        if self.header.box_type.as_bytes() != IPRP_BOX_TYPE {
            return Err(ParseItemPropertiesBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_iprp_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `av1C` box.
    pub fn parse_av1c(
        &self,
    ) -> Result<Av1CodecConfigurationBox, ParseAv1CodecConfigurationBoxError> {
        if self.header.box_type.as_bytes() != AV1C_BOX_TYPE {
            return Err(ParseAv1CodecConfigurationBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_av1c_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `hvcC` box.
    pub fn parse_hvcc(
        &self,
    ) -> Result<HevcDecoderConfigurationBox, ParseHevcDecoderConfigurationBoxError> {
        if self.header.box_type.as_bytes() != HVCC_BOX_TYPE {
            return Err(ParseHevcDecoderConfigurationBoxError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_hvcc_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `ispe` property.
    pub fn parse_ispe(
        &self,
    ) -> Result<ImageSpatialExtentsProperty, ParseImageSpatialExtentsPropertyError> {
        if self.header.box_type.as_bytes() != ISPE_BOX_TYPE {
            return Err(ParseImageSpatialExtentsPropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_ispe_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as a `pixi` property.
    pub fn parse_pixi(
        &self,
    ) -> Result<PixelInformationProperty, ParsePixelInformationPropertyError> {
        if self.header.box_type.as_bytes() != PIXI_BOX_TYPE {
            return Err(ParsePixelInformationPropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_pixi_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as a `colr` property.
    pub fn parse_colr(
        &self,
    ) -> Result<ColorInformationProperty, ParseColorInformationPropertyError> {
        if self.header.box_type.as_bytes() != COLR_BOX_TYPE {
            return Err(ParseColorInformationPropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_colr_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `irot` property.
    pub fn parse_irot(&self) -> Result<ImageRotationProperty, ParseImageRotationPropertyError> {
        if self.header.box_type.as_bytes() != IROT_BOX_TYPE {
            return Err(ParseImageRotationPropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_irot_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as an `imir` property.
    pub fn parse_imir(&self) -> Result<ImageMirrorProperty, ParseImageMirrorPropertyError> {
        if self.header.box_type.as_bytes() != IMIR_BOX_TYPE {
            return Err(ParseImageMirrorPropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_imir_payload(self.payload, self.payload_offset())
    }

    /// Parse this box payload as a `clap` property.
    pub fn parse_clap(
        &self,
    ) -> Result<ImageCleanApertureProperty, ParseImageCleanAperturePropertyError> {
        if self.header.box_type.as_bytes() != CLAP_BOX_TYPE {
            return Err(ParseImageCleanAperturePropertyError::UnexpectedBoxType {
                offset: self.offset,
                actual: self.header.box_type,
            });
        }

        parse_clap_payload(self.payload, self.payload_offset())
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

/// Parsed single `iref` typed edge fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemReferenceEntry {
    pub reference_type: FourCc,
    pub from_item_id: u32,
    pub to_item_ids: Vec<u32>,
}

/// Parsed `iref` (item reference) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemReferenceBox {
    pub full_box: FullBoxHeader,
    pub references: Vec<ItemReferenceEntry>,
}

/// Parsed `ipma` property association entry fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ItemPropertyAssociation {
    pub essential: bool,
    pub property_index: u16,
}

/// Parsed `ipma` item association entry fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemPropertyAssociationEntry {
    pub item_id: u32,
    pub associations: Vec<ItemPropertyAssociation>,
}

/// Parsed `ipma` (item property association) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemPropertyAssociationBox {
    pub full_box: FullBoxHeader,
    pub entry_count: u32,
    pub entries: Vec<ItemPropertyAssociationEntry>,
}

/// Parsed `ipco` (item property container) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemPropertyContainerBox<'a> {
    pub properties: Vec<ParsedBox<'a>>,
}

/// Parsed `iprp` (item properties) payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ItemPropertiesBox<'a> {
    pub property_containers: Vec<ItemPropertyContainerBox<'a>>,
    pub associations: Vec<ItemPropertyAssociationBox>,
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

    /// Resolve the primary image item graph from this `meta` payload.
    pub fn resolve_primary_item(
        &self,
    ) -> Result<ResolvedPrimaryItemGraph<'a>, ResolvePrimaryItemGraphError> {
        // Provenance: mirrors the mandatory image-structure checks and primary
        // item/property lookups in libheif/libheif/file.cc:HeifFile::parse_heif_images
        // and libheif/libheif/box.cc:Box_ipco::get_properties_for_item_ID.
        let children = self.parse_children()?;

        let pitm_box = find_first_child_box(&children, PITM_BOX_TYPE)
            .cloned()
            .ok_or(ResolvePrimaryItemGraphError::MissingRequiredBox {
                offset: self.payload_offset,
                box_type: FourCc::new(PITM_BOX_TYPE),
            })?;
        let pitm = pitm_box.parse_pitm()?;

        let iloc_box = find_first_child_box(&children, ILOC_BOX_TYPE)
            .cloned()
            .ok_or(ResolvePrimaryItemGraphError::MissingRequiredBox {
                offset: self.payload_offset,
                box_type: FourCc::new(ILOC_BOX_TYPE),
            })?;
        let iloc = iloc_box.parse_iloc()?;

        let iinf_box = find_first_child_box(&children, IINF_BOX_TYPE)
            .cloned()
            .ok_or(ResolvePrimaryItemGraphError::MissingRequiredBox {
                offset: self.payload_offset,
                box_type: FourCc::new(IINF_BOX_TYPE),
            })?;
        let iinf = iinf_box.parse_iinf()?;

        let iprp_box = find_first_child_box(&children, IPRP_BOX_TYPE)
            .cloned()
            .ok_or(ResolvePrimaryItemGraphError::MissingRequiredBox {
                offset: self.payload_offset,
                box_type: FourCc::new(IPRP_BOX_TYPE),
            })?;
        let iprp = iprp_box.parse_iprp()?;

        let iref = find_first_child_box(&children, IREF_BOX_TYPE)
            .cloned()
            .map(|child| child.parse_iref())
            .transpose()?;

        let item_id = pitm.item_id;
        let item_info = iinf
            .entries
            .iter()
            .find(|entry| entry.item_id == item_id)
            .cloned()
            .ok_or(
                ResolvePrimaryItemGraphError::PrimaryItemMissingFromItemInfo {
                    offset: iinf_box.offset,
                    item_id,
                },
            )?;

        let location = iloc
            .items
            .iter()
            .find(|item| item.item_id == item_id)
            .cloned()
            .ok_or(
                ResolvePrimaryItemGraphError::PrimaryItemMissingFromLocations {
                    offset: iloc_box.offset,
                    item_id,
                },
            )?;

        let references = iref
            .as_ref()
            .map(|parsed| {
                parsed
                    .references
                    .iter()
                    .filter(|reference| reference.from_item_id == item_id)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();

        let mut flattened_properties = Vec::new();
        for property_container in &iprp.property_containers {
            flattened_properties.extend(property_container.properties.iter().cloned());
        }

        let mut properties = Vec::new();
        for association_box in &iprp.associations {
            for entry in &association_box.entries {
                if entry.item_id != item_id {
                    continue;
                }

                for association in &entry.associations {
                    if association.property_index == 0 {
                        continue;
                    }

                    let property_index = usize::from(association.property_index - 1);
                    if property_index >= flattened_properties.len() {
                        return Err(ResolvePrimaryItemGraphError::PropertyIndexOutOfRange {
                            offset: iprp_box.offset,
                            item_id,
                            property_index: association.property_index,
                            available: flattened_properties.len(),
                        });
                    }

                    properties.push(ResolvedPrimaryItemProperty {
                        essential: association.essential,
                        property_index: association.property_index,
                        property: flattened_properties[property_index].clone(),
                    });
                }
            }
        }

        Ok(ResolvedPrimaryItemGraph {
            pitm,
            iloc,
            iinf,
            iprp,
            iref,
            primary_item: ResolvedPrimaryItem {
                item_id,
                item_info,
                location,
                properties,
                references,
            },
        })
    }
}

/// Resolved primary-item property from the `meta` item-property graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPrimaryItemProperty<'a> {
    pub essential: bool,
    pub property_index: u16,
    pub property: ParsedBox<'a>,
}

/// Resolved primary image item from `meta`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPrimaryItem<'a> {
    pub item_id: u32,
    pub item_info: ItemInfoEntryBox,
    pub location: ItemLocationItem,
    pub properties: Vec<ResolvedPrimaryItemProperty<'a>>,
    pub references: Vec<ItemReferenceEntry>,
}

/// Resolved `meta` item/property/reference graph centered on the primary item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPrimaryItemGraph<'a> {
    pub pitm: PrimaryItemBox,
    pub iloc: ItemLocationBox,
    pub iinf: ItemInfoBox,
    pub iprp: ItemPropertiesBox<'a>,
    pub iref: Option<ItemReferenceBox>,
    pub primary_item: ResolvedPrimaryItem<'a>,
}

/// Extracted primary AVIF item payload ready for AV1 decode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvifPrimaryItemData {
    pub item_id: u32,
    pub construction_method: u8,
    pub payload: Vec<u8>,
}

/// Extracted primary HEIC item payload ready for HEVC decode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicPrimaryItemData {
    pub item_id: u32,
    pub construction_method: u8,
    pub payload: Vec<u8>,
}

/// Parsed HEIC `grid` primary item descriptor payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeicGridDescriptor {
    pub version: u8,
    pub rows: u16,
    pub columns: u16,
    pub output_width: u32,
    pub output_height: u32,
}

/// One resolved `dimg` tile item payload used by a HEIC `grid` primary item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicGridTileItemData {
    pub item_id: u32,
    pub construction_method: u8,
    pub hvcc: HevcDecoderConfigurationBox,
    pub payload: Vec<u8>,
}

/// Extracted HEIC `grid` primary item descriptor and resolved tile payloads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicGridPrimaryItemData {
    pub item_id: u32,
    pub construction_method: u8,
    pub descriptor: HeicGridDescriptor,
    pub tile_item_ids: Vec<u32>,
    pub tiles: Vec<HeicGridTileItemData>,
}

/// Extracted HEIC primary item data, either direct coded payload or `grid`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HeicPrimaryItemDataWithGrid {
    Coded(HeicPrimaryItemData),
    Grid(HeicGridPrimaryItemData),
}

/// Parsed `av1C` codec configuration fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Av1CodecConfigurationBox {
    pub marker: bool,
    pub version: u8,
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: bool,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    pub initial_presentation_delay_present: bool,
    pub initial_presentation_delay_minus_one: Option<u8>,
    pub config_obus: Vec<u8>,
}

/// Parsed `hvcC` NAL array fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HevcNalArray {
    pub array_completeness: bool,
    pub nal_unit_type: u8,
    pub nal_units: Vec<Vec<u8>>,
}

/// Parsed `hvcC` decoder configuration fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HevcDecoderConfigurationBox {
    pub configuration_version: u8,
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    pub general_constraint_indicator_flags: [u8; 6],
    pub general_level_idc: u8,
    pub min_spatial_segmentation_idc: u16,
    pub parallelism_type: u8,
    pub chroma_format: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub avg_frame_rate: u16,
    pub constant_frame_rate: u8,
    pub num_temporal_layers: u8,
    pub temporal_id_nested: bool,
    pub nal_length_size: u8,
    pub nal_arrays: Vec<HevcNalArray>,
}

/// Parsed `ispe` property fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageSpatialExtentsProperty {
    pub full_box: FullBoxHeader,
    pub width: u32,
    pub height: u32,
}

/// Parsed `pixi` property fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PixelInformationProperty {
    pub full_box: FullBoxHeader,
    pub bits_per_channel: Vec<u8>,
}

/// Parsed `colr` NCLX/NCLC color profile fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NclxColorProfile {
    pub colour_primaries: u16,
    pub transfer_characteristics: u16,
    pub matrix_coefficients: u16,
    pub full_range_flag: bool,
}

/// Parsed `colr` ICC profile payload fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IccColorProfile {
    pub profile_type: FourCc,
    pub profile: Vec<u8>,
}

/// Parsed `colr` payload variants.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ColorInformation {
    Nclx(NclxColorProfile),
    Icc(IccColorProfile),
}

/// Parsed `colr` property fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ColorInformationProperty {
    pub colour_type: FourCc,
    pub information: ColorInformation,
}

/// Parsed `irot` property fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageRotationProperty {
    pub rotation_ccw_degrees: u16,
}

/// Mirror direction parsed from an `imir` property.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageMirrorDirection {
    Horizontal,
    Vertical,
}

/// Parsed `imir` property fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageMirrorProperty {
    pub direction: ImageMirrorDirection,
}

/// Parsed `clap` property fields.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ImageCleanApertureProperty {
    pub clean_aperture_width_num: u32,
    pub clean_aperture_width_den: u32,
    pub clean_aperture_height_num: u32,
    pub clean_aperture_height_den: u32,
    pub horizontal_offset_num: i32,
    pub horizontal_offset_den: u32,
    pub vertical_offset_num: i32,
    pub vertical_offset_den: u32,
}

/// Parsed primary-item transform property in associated-property order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimaryItemTransformProperty {
    CleanAperture(ImageCleanApertureProperty),
    Rotation(ImageRotationProperty),
    Mirror(ImageMirrorProperty),
}

/// Aggregated primary-item transform properties in associated-property order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PrimaryItemTransformProperties {
    pub item_id: u32,
    pub transforms: Vec<PrimaryItemTransformProperty>,
}

/// Aggregated primary-item color profiles from associated `colr` properties.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PrimaryItemColorProperties {
    pub nclx: Option<NclxColorProfile>,
    pub icc: Option<IccColorProfile>,
}

/// Parsed primary AVIF properties needed before decode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AvifPrimaryItemProperties {
    pub item_id: u32,
    pub av1c: Av1CodecConfigurationBox,
    pub ispe: ImageSpatialExtentsProperty,
    pub pixi: PixelInformationProperty,
    pub colr: PrimaryItemColorProperties,
}

/// Parsed primary HEIC properties needed before decode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicPrimaryItemProperties {
    pub item_id: u32,
    pub hvcc: HevcDecoderConfigurationBox,
    pub ispe: ImageSpatialExtentsProperty,
    pub pixi: PixelInformationProperty,
    pub colr: PrimaryItemColorProperties,
}

/// Parsed primary HEIC properties needed for decoder preflight.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HeicPrimaryItemPreflightProperties {
    pub item_id: u32,
    pub hvcc: HevcDecoderConfigurationBox,
    pub ispe: ImageSpatialExtentsProperty,
    pub pixi: Option<PixelInformationProperty>,
    pub colr: PrimaryItemColorProperties,
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

/// Errors returned when parsing an `iref` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemReferenceBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    FullBox(ParseFullBoxError),
    UnsupportedVersion {
        offset: u64,
        version: u8,
    },
    ChildBox(ParseBoxError),
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
    EmptyReferenceList {
        offset: u64,
        reference_type: FourCc,
    },
}

impl Display for ParseItemReferenceBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemReferenceBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected iref box at offset {offset}, got box type {actual}"
            ),
            ParseItemReferenceBoxError::FullBox(err) => write!(f, "{err}"),
            ParseItemReferenceBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "iref box at offset {offset} has unsupported full box version {version}"
            ),
            ParseItemReferenceBoxError::ChildBox(err) => write!(f, "{err}"),
            ParseItemReferenceBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "iref payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseItemReferenceBoxError::EmptyReferenceList {
                offset,
                reference_type,
            } => write!(
                f,
                "iref typed reference box {reference_type} has zero references at offset {offset}"
            ),
        }
    }
}

impl Error for ParseItemReferenceBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemReferenceBoxError::FullBox(err) => Some(err),
            ParseItemReferenceBoxError::ChildBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseItemReferenceBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

impl From<ParseBoxError> for ParseItemReferenceBoxError {
    fn from(value: ParseBoxError) -> Self {
        Self::ChildBox(value)
    }
}

/// Errors returned when parsing an `ipco` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemPropertyContainerBoxError {
    UnexpectedBoxType { offset: u64, actual: FourCc },
    ChildBox(ParseBoxError),
}

impl Display for ParseItemPropertyContainerBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemPropertyContainerBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected ipco box at offset {offset}, got box type {actual}"
            ),
            ParseItemPropertyContainerBoxError::ChildBox(err) => write!(f, "{err}"),
        }
    }
}

impl Error for ParseItemPropertyContainerBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemPropertyContainerBoxError::ChildBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseBoxError> for ParseItemPropertyContainerBoxError {
    fn from(value: ParseBoxError) -> Self {
        Self::ChildBox(value)
    }
}

/// Errors returned when parsing an `ipma` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemPropertyAssociationBoxError {
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
    EntryCountTooLarge {
        offset: u64,
        entry_count: u32,
    },
}

impl Display for ParseItemPropertyAssociationBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemPropertyAssociationBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected ipma box at offset {offset}, got box type {actual}"
            ),
            ParseItemPropertyAssociationBoxError::FullBox(err) => write!(f, "{err}"),
            ParseItemPropertyAssociationBoxError::UnsupportedVersion { offset, version } => write!(
                f,
                "ipma box at offset {offset} has unsupported full box version {version}"
            ),
            ParseItemPropertyAssociationBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "ipma payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseItemPropertyAssociationBoxError::EntryCountTooLarge {
                offset,
                entry_count,
            } => write!(
                f,
                "ipma entry_count {entry_count} cannot be represented at offset {offset}"
            ),
        }
    }
}

impl Error for ParseItemPropertyAssociationBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemPropertyAssociationBoxError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseItemPropertyAssociationBoxError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing an `iprp` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseItemPropertiesBoxError {
    UnexpectedBoxType { offset: u64, actual: FourCc },
    ChildBox(ParseBoxError),
    PropertyContainer(ParseItemPropertyContainerBoxError),
    PropertyAssociation(ParseItemPropertyAssociationBoxError),
}

impl Display for ParseItemPropertiesBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseItemPropertiesBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected iprp box at offset {offset}, got box type {actual}"
            ),
            ParseItemPropertiesBoxError::ChildBox(err) => write!(f, "{err}"),
            ParseItemPropertiesBoxError::PropertyContainer(err) => write!(f, "{err}"),
            ParseItemPropertiesBoxError::PropertyAssociation(err) => write!(f, "{err}"),
        }
    }
}

impl Error for ParseItemPropertiesBoxError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseItemPropertiesBoxError::ChildBox(err) => Some(err),
            ParseItemPropertiesBoxError::PropertyContainer(err) => Some(err),
            ParseItemPropertiesBoxError::PropertyAssociation(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseBoxError> for ParseItemPropertiesBoxError {
    fn from(value: ParseBoxError) -> Self {
        Self::ChildBox(value)
    }
}

impl From<ParseItemPropertyContainerBoxError> for ParseItemPropertiesBoxError {
    fn from(value: ParseItemPropertyContainerBoxError) -> Self {
        Self::PropertyContainer(value)
    }
}

impl From<ParseItemPropertyAssociationBoxError> for ParseItemPropertiesBoxError {
    fn from(value: ParseItemPropertyAssociationBoxError) -> Self {
        Self::PropertyAssociation(value)
    }
}

/// Errors returned when resolving the primary item from a parsed `meta` graph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvePrimaryItemGraphError {
    ChildBox(ParseBoxError),
    MissingRequiredBox {
        offset: u64,
        box_type: FourCc,
    },
    PrimaryItem(ParsePrimaryItemBoxError),
    ItemLocation(ParseItemLocationBoxError),
    ItemInfo(ParseItemInfoBoxError),
    ItemProperties(ParseItemPropertiesBoxError),
    ItemReference(ParseItemReferenceBoxError),
    PrimaryItemMissingFromItemInfo {
        offset: u64,
        item_id: u32,
    },
    PrimaryItemMissingFromLocations {
        offset: u64,
        item_id: u32,
    },
    PropertyIndexOutOfRange {
        offset: u64,
        item_id: u32,
        property_index: u16,
        available: usize,
    },
}

impl Display for ResolvePrimaryItemGraphError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolvePrimaryItemGraphError::ChildBox(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::MissingRequiredBox { offset, box_type } => {
                write!(f, "required meta child box {box_type} is missing at offset {offset}")
            }
            ResolvePrimaryItemGraphError::PrimaryItem(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::ItemLocation(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::ItemInfo(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::ItemProperties(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::ItemReference(err) => write!(f, "{err}"),
            ResolvePrimaryItemGraphError::PrimaryItemMissingFromItemInfo { offset, item_id } => {
                write!(
                    f,
                    "primary item_ID {item_id} from pitm is missing in iinf at offset {offset}"
                )
            }
            ResolvePrimaryItemGraphError::PrimaryItemMissingFromLocations { offset, item_id } => {
                write!(
                    f,
                    "primary item_ID {item_id} from pitm is missing in iloc at offset {offset}"
                )
            }
            ResolvePrimaryItemGraphError::PropertyIndexOutOfRange {
                offset,
                item_id,
                property_index,
                available,
            } => write!(
                f,
                "ipma property index {property_index} for item_ID {item_id} exceeds available ipco properties ({available}) at offset {offset}"
            ),
        }
    }
}

impl Error for ResolvePrimaryItemGraphError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ResolvePrimaryItemGraphError::ChildBox(err) => Some(err),
            ResolvePrimaryItemGraphError::PrimaryItem(err) => Some(err),
            ResolvePrimaryItemGraphError::ItemLocation(err) => Some(err),
            ResolvePrimaryItemGraphError::ItemInfo(err) => Some(err),
            ResolvePrimaryItemGraphError::ItemProperties(err) => Some(err),
            ResolvePrimaryItemGraphError::ItemReference(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParseBoxError) -> Self {
        Self::ChildBox(value)
    }
}

impl From<ParsePrimaryItemBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParsePrimaryItemBoxError) -> Self {
        Self::PrimaryItem(value)
    }
}

impl From<ParseItemLocationBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParseItemLocationBoxError) -> Self {
        Self::ItemLocation(value)
    }
}

impl From<ParseItemInfoBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParseItemInfoBoxError) -> Self {
        Self::ItemInfo(value)
    }
}

impl From<ParseItemPropertiesBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParseItemPropertiesBoxError) -> Self {
        Self::ItemProperties(value)
    }
}

impl From<ParseItemReferenceBoxError> for ResolvePrimaryItemGraphError {
    fn from(value: ParseItemReferenceBoxError) -> Self {
        Self::ItemReference(value)
    }
}

/// Errors returned when parsing an `av1C` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseAv1CodecConfigurationBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        available: usize,
        required: usize,
    },
    InvalidMarkerBit {
        offset: u64,
        value: u8,
    },
}

impl Display for ParseAv1CodecConfigurationBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseAv1CodecConfigurationBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected av1C box at offset {offset}, got box type {actual}"
            ),
            ParseAv1CodecConfigurationBoxError::PayloadTooSmall {
                offset,
                available,
                required,
            } => write!(
                f,
                "av1C payload too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseAv1CodecConfigurationBoxError::InvalidMarkerBit { offset, value } => write!(
                f,
                "av1C marker bit not set at offset {offset} (first byte: 0x{value:02x})"
            ),
        }
    }
}

impl Error for ParseAv1CodecConfigurationBoxError {}

/// Errors returned when parsing an `hvcC` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseHevcDecoderConfigurationBoxError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
    UnsupportedConfigurationVersion {
        offset: u64,
        version: u8,
    },
}

impl Display for ParseHevcDecoderConfigurationBoxError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseHevcDecoderConfigurationBoxError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected hvcC box at offset {offset}, got box type {actual}"
            ),
            ParseHevcDecoderConfigurationBoxError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "hvcC payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseHevcDecoderConfigurationBoxError::UnsupportedConfigurationVersion {
                offset,
                version,
            } => write!(
                f,
                "hvcC box at offset {offset} has unsupported configurationVersion {version}"
            ),
        }
    }
}

impl Error for ParseHevcDecoderConfigurationBoxError {}

/// Errors returned when parsing an `ispe` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseImageSpatialExtentsPropertyError {
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
        available: usize,
        required: usize,
    },
}

impl Display for ParseImageSpatialExtentsPropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseImageSpatialExtentsPropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected ispe box at offset {offset}, got box type {actual}"
            ),
            ParseImageSpatialExtentsPropertyError::FullBox(err) => write!(f, "{err}"),
            ParseImageSpatialExtentsPropertyError::UnsupportedVersion { offset, version } => write!(
                f,
                "ispe box at offset {offset} has unsupported full box version {version}"
            ),
            ParseImageSpatialExtentsPropertyError::PayloadTooSmall {
                offset,
                available,
                required,
            } => write!(
                f,
                "ispe payload too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParseImageSpatialExtentsPropertyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParseImageSpatialExtentsPropertyError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParseImageSpatialExtentsPropertyError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing a `pixi` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsePixelInformationPropertyError {
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

impl Display for ParsePixelInformationPropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsePixelInformationPropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected pixi box at offset {offset}, got box type {actual}"
            ),
            ParsePixelInformationPropertyError::FullBox(err) => write!(f, "{err}"),
            ParsePixelInformationPropertyError::UnsupportedVersion { offset, version } => write!(
                f,
                "pixi box at offset {offset} has unsupported full box version {version}"
            ),
            ParsePixelInformationPropertyError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "pixi payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParsePixelInformationPropertyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParsePixelInformationPropertyError::FullBox(err) => Some(err),
            _ => None,
        }
    }
}

impl From<ParseFullBoxError> for ParsePixelInformationPropertyError {
    fn from(value: ParseFullBoxError) -> Self {
        Self::FullBox(value)
    }
}

/// Errors returned when parsing a `colr` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseColorInformationPropertyError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        context: &'static str,
        available: usize,
        required: usize,
    },
    UnknownColourType {
        offset: u64,
        colour_type: FourCc,
    },
}

impl Display for ParseColorInformationPropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseColorInformationPropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected colr box at offset {offset}, got box type {actual}"
            ),
            ParseColorInformationPropertyError::PayloadTooSmall {
                offset,
                context,
                available,
                required,
            } => write!(
                f,
                "colr payload too small for {context} at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseColorInformationPropertyError::UnknownColourType {
                offset,
                colour_type,
            } => write!(
                f,
                "colr box at offset {offset} has unsupported colour type {colour_type}"
            ),
        }
    }
}

impl Error for ParseColorInformationPropertyError {}

/// Errors returned when parsing an `irot` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseImageRotationPropertyError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        available: usize,
        required: usize,
    },
}

impl Display for ParseImageRotationPropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseImageRotationPropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected irot box at offset {offset}, got box type {actual}"
            ),
            ParseImageRotationPropertyError::PayloadTooSmall {
                offset,
                available,
                required,
            } => write!(
                f,
                "irot payload too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParseImageRotationPropertyError {}

/// Errors returned when parsing an `imir` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseImageMirrorPropertyError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        available: usize,
        required: usize,
    },
}

impl Display for ParseImageMirrorPropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseImageMirrorPropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected imir box at offset {offset}, got box type {actual}"
            ),
            ParseImageMirrorPropertyError::PayloadTooSmall {
                offset,
                available,
                required,
            } => write!(
                f,
                "imir payload too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
        }
    }
}

impl Error for ParseImageMirrorPropertyError {}

/// Errors returned when parsing a `clap` payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParseImageCleanAperturePropertyError {
    UnexpectedBoxType {
        offset: u64,
        actual: FourCc,
    },
    PayloadTooSmall {
        offset: u64,
        available: usize,
        required: usize,
    },
    ZeroDenominator {
        offset: u64,
        field: &'static str,
    },
}

impl Display for ParseImageCleanAperturePropertyError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseImageCleanAperturePropertyError::UnexpectedBoxType { offset, actual } => write!(
                f,
                "expected clap box at offset {offset}, got box type {actual}"
            ),
            ParseImageCleanAperturePropertyError::PayloadTooSmall {
                offset,
                available,
                required,
            } => write!(
                f,
                "clap payload too small at offset {offset} (available: {available} bytes, required: {required})"
            ),
            ParseImageCleanAperturePropertyError::ZeroDenominator { offset, field } => write!(
                f,
                "clap field {field} has zero denominator at offset {offset}"
            ),
        }
    }
}

impl Error for ParseImageCleanAperturePropertyError {}

/// Errors returned when parsing primary-item transform properties.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsePrimaryItemTransformPropertiesError {
    TopLevelBoxes(ParseBoxError),
    MissingMetaBox,
    Meta(ParseMetaBoxError),
    ResolvePrimaryItem(ResolvePrimaryItemGraphError),
    ImageCleanAperture(ParseImageCleanAperturePropertyError),
    ImageRotation(ParseImageRotationPropertyError),
    ImageMirror(ParseImageMirrorPropertyError),
}

impl Display for ParsePrimaryItemTransformPropertiesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsePrimaryItemTransformPropertiesError::TopLevelBoxes(err) => write!(f, "{err}"),
            ParsePrimaryItemTransformPropertiesError::MissingMetaBox => {
                write!(f, "required top-level meta box is missing")
            }
            ParsePrimaryItemTransformPropertiesError::Meta(err) => write!(f, "{err}"),
            ParsePrimaryItemTransformPropertiesError::ResolvePrimaryItem(err) => write!(f, "{err}"),
            ParsePrimaryItemTransformPropertiesError::ImageCleanAperture(err) => write!(f, "{err}"),
            ParsePrimaryItemTransformPropertiesError::ImageRotation(err) => write!(f, "{err}"),
            ParsePrimaryItemTransformPropertiesError::ImageMirror(err) => write!(f, "{err}"),
        }
    }
}

impl Error for ParsePrimaryItemTransformPropertiesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParsePrimaryItemTransformPropertiesError::TopLevelBoxes(err) => Some(err),
            ParsePrimaryItemTransformPropertiesError::Meta(err) => Some(err),
            ParsePrimaryItemTransformPropertiesError::ResolvePrimaryItem(err) => Some(err),
            ParsePrimaryItemTransformPropertiesError::ImageCleanAperture(err) => Some(err),
            ParsePrimaryItemTransformPropertiesError::ImageRotation(err) => Some(err),
            ParsePrimaryItemTransformPropertiesError::ImageMirror(err) => Some(err),
            _ => None,
        }
    }
}

/// Errors returned when parsing/validating primary AVIF properties.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsePrimaryAvifPropertiesError {
    TopLevelBoxes(ParseBoxError),
    MissingMetaBox,
    Meta(ParseMetaBoxError),
    ResolvePrimaryItem(ResolvePrimaryItemGraphError),
    MissingPrimaryItemType {
        item_id: u32,
    },
    UnexpectedPrimaryItemType {
        item_id: u32,
        actual: FourCc,
    },
    MissingRequiredProperty {
        item_id: u32,
        property_type: FourCc,
    },
    DuplicateProperty {
        item_id: u32,
        property_type: FourCc,
    },
    Av1Codec(ParseAv1CodecConfigurationBoxError),
    ImageSpatialExtents(ParseImageSpatialExtentsPropertyError),
    PixelInformation(ParsePixelInformationPropertyError),
    ColorInformation(ParseColorInformationPropertyError),
    InvalidImageExtent {
        item_id: u32,
        width: u32,
        height: u32,
    },
    InvalidPixiChannelCount {
        item_id: u32,
        channel_count: usize,
    },
    InvalidPixiBitsPerChannel {
        item_id: u32,
        channel_index: usize,
    },
}

impl Display for ParsePrimaryAvifPropertiesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsePrimaryAvifPropertiesError::TopLevelBoxes(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::MissingMetaBox => {
                write!(f, "required top-level meta box is missing")
            }
            ParsePrimaryAvifPropertiesError::Meta(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::ResolvePrimaryItem(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::MissingPrimaryItemType { item_id } => write!(
                f,
                "primary item_ID {item_id} is missing an infe item_type, expected av01"
            ),
            ParsePrimaryAvifPropertiesError::UnexpectedPrimaryItemType { item_id, actual } => {
                write!(
                    f,
                    "primary item_ID {item_id} has infe item_type {actual}, expected av01"
                )
            }
            ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
                item_id,
                property_type,
            } => write!(
                f,
                "primary item_ID {item_id} is missing required AVIF property {property_type}"
            ),
            ParsePrimaryAvifPropertiesError::DuplicateProperty {
                item_id,
                property_type,
            } => write!(
                f,
                "primary item_ID {item_id} has multiple {property_type} properties"
            ),
            ParsePrimaryAvifPropertiesError::Av1Codec(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::ImageSpatialExtents(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::PixelInformation(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::ColorInformation(err) => write!(f, "{err}"),
            ParsePrimaryAvifPropertiesError::InvalidImageExtent {
                item_id,
                width,
                height,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid ispe dimensions ({width}x{height})"
            ),
            ParsePrimaryAvifPropertiesError::InvalidPixiChannelCount {
                item_id,
                channel_count,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid pixi channel count {channel_count}"
            ),
            ParsePrimaryAvifPropertiesError::InvalidPixiBitsPerChannel {
                item_id,
                channel_index,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid pixi bits_per_channel for channel index {channel_index}"
            ),
        }
    }
}

impl Error for ParsePrimaryAvifPropertiesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParsePrimaryAvifPropertiesError::TopLevelBoxes(err) => Some(err),
            ParsePrimaryAvifPropertiesError::Meta(err) => Some(err),
            ParsePrimaryAvifPropertiesError::ResolvePrimaryItem(err) => Some(err),
            ParsePrimaryAvifPropertiesError::Av1Codec(err) => Some(err),
            ParsePrimaryAvifPropertiesError::ImageSpatialExtents(err) => Some(err),
            ParsePrimaryAvifPropertiesError::PixelInformation(err) => Some(err),
            ParsePrimaryAvifPropertiesError::ColorInformation(err) => Some(err),
            _ => None,
        }
    }
}

/// Errors returned when parsing/validating primary HEIC properties.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParsePrimaryHeicPropertiesError {
    TopLevelBoxes(ParseBoxError),
    MissingMetaBox,
    Meta(ParseMetaBoxError),
    ResolvePrimaryItem(ResolvePrimaryItemGraphError),
    MissingPrimaryItemType {
        item_id: u32,
    },
    UnexpectedPrimaryItemType {
        item_id: u32,
        actual: FourCc,
    },
    MissingRequiredProperty {
        item_id: u32,
        property_type: FourCc,
    },
    DuplicateProperty {
        item_id: u32,
        property_type: FourCc,
    },
    HevcCodec(ParseHevcDecoderConfigurationBoxError),
    ImageSpatialExtents(ParseImageSpatialExtentsPropertyError),
    PixelInformation(ParsePixelInformationPropertyError),
    ColorInformation(ParseColorInformationPropertyError),
    InvalidImageExtent {
        item_id: u32,
        width: u32,
        height: u32,
    },
    InvalidPixiChannelCount {
        item_id: u32,
        channel_count: usize,
    },
    InvalidPixiBitsPerChannel {
        item_id: u32,
        channel_index: usize,
    },
}

impl Display for ParsePrimaryHeicPropertiesError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParsePrimaryHeicPropertiesError::TopLevelBoxes(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::MissingMetaBox => {
                write!(f, "required top-level meta box is missing")
            }
            ParsePrimaryHeicPropertiesError::Meta(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::ResolvePrimaryItem(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::MissingPrimaryItemType { item_id } => write!(
                f,
                "primary item_ID {item_id} is missing an infe item_type, expected hvc1 or hev1"
            ),
            ParsePrimaryHeicPropertiesError::UnexpectedPrimaryItemType { item_id, actual } => {
                write!(
                    f,
                    "primary item_ID {item_id} has infe item_type {actual}, expected hvc1 or hev1"
                )
            }
            ParsePrimaryHeicPropertiesError::MissingRequiredProperty {
                item_id,
                property_type,
            } => write!(
                f,
                "primary item_ID {item_id} is missing required HEIC property {property_type}"
            ),
            ParsePrimaryHeicPropertiesError::DuplicateProperty {
                item_id,
                property_type,
            } => write!(
                f,
                "primary item_ID {item_id} has multiple {property_type} properties"
            ),
            ParsePrimaryHeicPropertiesError::HevcCodec(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::ImageSpatialExtents(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::PixelInformation(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::ColorInformation(err) => write!(f, "{err}"),
            ParsePrimaryHeicPropertiesError::InvalidImageExtent {
                item_id,
                width,
                height,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid ispe dimensions ({width}x{height})"
            ),
            ParsePrimaryHeicPropertiesError::InvalidPixiChannelCount {
                item_id,
                channel_count,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid pixi channel count {channel_count}"
            ),
            ParsePrimaryHeicPropertiesError::InvalidPixiBitsPerChannel {
                item_id,
                channel_index,
            } => write!(
                f,
                "primary item_ID {item_id} has invalid pixi bits_per_channel for channel index {channel_index}"
            ),
        }
    }
}

impl Error for ParsePrimaryHeicPropertiesError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ParsePrimaryHeicPropertiesError::TopLevelBoxes(err) => Some(err),
            ParsePrimaryHeicPropertiesError::Meta(err) => Some(err),
            ParsePrimaryHeicPropertiesError::ResolvePrimaryItem(err) => Some(err),
            ParsePrimaryHeicPropertiesError::HevcCodec(err) => Some(err),
            ParsePrimaryHeicPropertiesError::ImageSpatialExtents(err) => Some(err),
            ParsePrimaryHeicPropertiesError::PixelInformation(err) => Some(err),
            ParsePrimaryHeicPropertiesError::ColorInformation(err) => Some(err),
            _ => None,
        }
    }
}

/// Errors returned when extracting the primary AVIF payload from BMFF boxes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtractAvifItemDataError {
    TopLevelBoxes(ParseBoxError),
    MissingMetaBox,
    Meta(ParseMetaBoxError),
    ResolvePrimaryItem(ResolvePrimaryItemGraphError),
    MissingPrimaryItemType {
        item_id: u32,
    },
    UnexpectedPrimaryItemType {
        item_id: u32,
        actual: FourCc,
    },
    UnsupportedDataReferenceIndex {
        item_id: u32,
        data_reference_index: u16,
    },
    UnsupportedConstructionMethod {
        item_id: u32,
        construction_method: u8,
    },
    MissingIdatBox {
        item_id: u32,
    },
    MetaChildBoxes(ParseBoxError),
    ExtentOffsetOverflow {
        item_id: u32,
        base_offset: u64,
        extent_offset: u64,
        extent_length: u64,
    },
    ExtentOutOfBounds {
        item_id: u32,
        construction_method: u8,
        start: u64,
        length: u64,
        available: u64,
    },
    PayloadTooLarge {
        item_id: u32,
        length: u64,
    },
}

impl Display for ExtractAvifItemDataError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractAvifItemDataError::TopLevelBoxes(err) => write!(f, "{err}"),
            ExtractAvifItemDataError::MissingMetaBox => {
                write!(f, "required top-level meta box is missing")
            }
            ExtractAvifItemDataError::Meta(err) => write!(f, "{err}"),
            ExtractAvifItemDataError::ResolvePrimaryItem(err) => write!(f, "{err}"),
            ExtractAvifItemDataError::MissingPrimaryItemType { item_id } => write!(
                f,
                "primary item_ID {item_id} is missing an infe item_type, expected av01"
            ),
            ExtractAvifItemDataError::UnexpectedPrimaryItemType { item_id, actual } => write!(
                f,
                "primary item_ID {item_id} has infe item_type {actual}, expected av01"
            ),
            ExtractAvifItemDataError::UnsupportedDataReferenceIndex {
                item_id,
                data_reference_index,
            } => write!(
                f,
                "primary item_ID {item_id} uses unsupported iloc data_reference_index {data_reference_index}"
            ),
            ExtractAvifItemDataError::UnsupportedConstructionMethod {
                item_id,
                construction_method,
            } => write!(
                f,
                "primary item_ID {item_id} uses unsupported iloc construction_method {construction_method}"
            ),
            ExtractAvifItemDataError::MissingIdatBox { item_id } => write!(
                f,
                "primary item_ID {item_id} references idat-backed iloc extents but no idat box exists in meta"
            ),
            ExtractAvifItemDataError::MetaChildBoxes(err) => write!(f, "{err}"),
            ExtractAvifItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset,
                extent_offset,
                extent_length,
            } => write!(
                f,
                "primary item_ID {item_id} iloc extent offset overflow (base_offset={base_offset}, extent_offset={extent_offset}, extent_length={extent_length})"
            ),
            ExtractAvifItemDataError::ExtentOutOfBounds {
                item_id,
                construction_method,
                start,
                length,
                available,
            } => write!(
                f,
                "primary item_ID {item_id} iloc extent (construction_method={construction_method}, start={start}, length={length}) exceeds available bytes {available}"
            ),
            ExtractAvifItemDataError::PayloadTooLarge { item_id, length } => write!(
                f,
                "primary item_ID {item_id} payload length {length} cannot be represented on this platform"
            ),
        }
    }
}

impl Error for ExtractAvifItemDataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExtractAvifItemDataError::TopLevelBoxes(err) => Some(err),
            ExtractAvifItemDataError::Meta(err) => Some(err),
            ExtractAvifItemDataError::ResolvePrimaryItem(err) => Some(err),
            ExtractAvifItemDataError::MetaChildBoxes(err) => Some(err),
            _ => None,
        }
    }
}

/// Errors returned when extracting the primary HEIC payload from BMFF boxes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExtractHeicItemDataError {
    TopLevelBoxes(ParseBoxError),
    MissingMetaBox,
    Meta(ParseMetaBoxError),
    ResolvePrimaryItem(ResolvePrimaryItemGraphError),
    MissingPrimaryItemType {
        item_id: u32,
    },
    UnexpectedPrimaryItemType {
        item_id: u32,
        actual: FourCc,
    },
    GridDescriptorTooSmall {
        item_id: u32,
        available: usize,
        required: usize,
    },
    UnsupportedGridDescriptorVersion {
        item_id: u32,
        version: u8,
    },
    MissingGridTileReferences {
        item_id: u32,
    },
    GridTileCountMismatch {
        item_id: u32,
        rows: u16,
        columns: u16,
        expected: usize,
        actual: usize,
    },
    MissingGridTileItemInfo {
        item_id: u32,
        tile_item_id: u32,
    },
    MissingGridTileItemType {
        item_id: u32,
        tile_item_id: u32,
    },
    UnexpectedGridTileItemType {
        item_id: u32,
        tile_item_id: u32,
        actual: FourCc,
    },
    MissingGridTileLocation {
        item_id: u32,
        tile_item_id: u32,
    },
    GridTilePropertyIndexOutOfRange {
        item_id: u32,
        tile_item_id: u32,
        property_index: u16,
        available: usize,
    },
    MissingGridTileCodecConfiguration {
        item_id: u32,
        tile_item_id: u32,
    },
    DuplicateGridTileCodecConfiguration {
        item_id: u32,
        tile_item_id: u32,
    },
    GridTileCodecConfiguration {
        item_id: u32,
        tile_item_id: u32,
        source: ParseHevcDecoderConfigurationBoxError,
    },
    UnsupportedDataReferenceIndex {
        item_id: u32,
        data_reference_index: u16,
    },
    UnsupportedConstructionMethod {
        item_id: u32,
        construction_method: u8,
    },
    MissingIdatBox {
        item_id: u32,
    },
    MetaChildBoxes(ParseBoxError),
    ExtentOffsetOverflow {
        item_id: u32,
        base_offset: u64,
        extent_offset: u64,
        extent_length: u64,
    },
    ExtentOutOfBounds {
        item_id: u32,
        construction_method: u8,
        start: u64,
        length: u64,
        available: u64,
    },
    PayloadTooLarge {
        item_id: u32,
        length: u64,
    },
}

impl Display for ExtractHeicItemDataError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtractHeicItemDataError::TopLevelBoxes(err) => write!(f, "{err}"),
            ExtractHeicItemDataError::MissingMetaBox => {
                write!(f, "required top-level meta box is missing")
            }
            ExtractHeicItemDataError::Meta(err) => write!(f, "{err}"),
            ExtractHeicItemDataError::ResolvePrimaryItem(err) => write!(f, "{err}"),
            ExtractHeicItemDataError::MissingPrimaryItemType { item_id } => write!(
                f,
                "primary item_ID {item_id} is missing an infe item_type, expected hvc1 or hev1"
            ),
            ExtractHeicItemDataError::UnexpectedPrimaryItemType { item_id, actual } => write!(
                f,
                "primary item_ID {item_id} has infe item_type {actual}, expected hvc1 or hev1"
            ),
            ExtractHeicItemDataError::GridDescriptorTooSmall {
                item_id,
                available,
                required,
            } => write!(
                f,
                "primary grid item_ID {item_id} descriptor is truncated (available: {available} bytes, required: {required})"
            ),
            ExtractHeicItemDataError::UnsupportedGridDescriptorVersion { item_id, version } => {
                write!(
                    f,
                    "primary grid item_ID {item_id} has unsupported descriptor version {version}"
                )
            }
            ExtractHeicItemDataError::MissingGridTileReferences { item_id } => write!(
                f,
                "primary grid item_ID {item_id} has no dimg tile references"
            ),
            ExtractHeicItemDataError::GridTileCountMismatch {
                item_id,
                rows,
                columns,
                expected,
                actual,
            } => write!(
                f,
                "primary grid item_ID {item_id} expects {rows}x{columns}={expected} dimg tile references, found {actual}"
            ),
            ExtractHeicItemDataError::MissingGridTileItemInfo {
                item_id,
                tile_item_id,
            } => write!(
                f,
                "primary grid item_ID {item_id} references tile item_ID {tile_item_id} that is missing from iinf"
            ),
            ExtractHeicItemDataError::MissingGridTileItemType {
                item_id,
                tile_item_id,
            } => write!(
                f,
                "primary grid item_ID {item_id} references tile item_ID {tile_item_id} without an infe item_type"
            ),
            ExtractHeicItemDataError::UnexpectedGridTileItemType {
                item_id,
                tile_item_id,
                actual,
            } => write!(
                f,
                "primary grid item_ID {item_id} references tile item_ID {tile_item_id} with infe item_type {actual}, expected hvc1 or hev1"
            ),
            ExtractHeicItemDataError::MissingGridTileLocation {
                item_id,
                tile_item_id,
            } => write!(
                f,
                "primary grid item_ID {item_id} references tile item_ID {tile_item_id} that is missing from iloc"
            ),
            ExtractHeicItemDataError::GridTilePropertyIndexOutOfRange {
                item_id,
                tile_item_id,
                property_index,
                available,
            } => write!(
                f,
                "primary grid item_ID {item_id} tile item_ID {tile_item_id} references property index {property_index}, but only {available} properties are available"
            ),
            ExtractHeicItemDataError::MissingGridTileCodecConfiguration {
                item_id,
                tile_item_id,
            } => write!(
                f,
                "primary grid item_ID {item_id} tile item_ID {tile_item_id} is missing required hvcC property"
            ),
            ExtractHeicItemDataError::DuplicateGridTileCodecConfiguration {
                item_id,
                tile_item_id,
            } => write!(
                f,
                "primary grid item_ID {item_id} tile item_ID {tile_item_id} has multiple hvcC properties"
            ),
            ExtractHeicItemDataError::GridTileCodecConfiguration {
                item_id,
                tile_item_id,
                source,
            } => write!(
                f,
                "primary grid item_ID {item_id} tile item_ID {tile_item_id} has invalid hvcC property: {source}"
            ),
            ExtractHeicItemDataError::UnsupportedDataReferenceIndex {
                item_id,
                data_reference_index,
            } => write!(
                f,
                "primary item_ID {item_id} uses unsupported iloc data_reference_index {data_reference_index}"
            ),
            ExtractHeicItemDataError::UnsupportedConstructionMethod {
                item_id,
                construction_method,
            } => write!(
                f,
                "primary item_ID {item_id} uses unsupported iloc construction_method {construction_method}"
            ),
            ExtractHeicItemDataError::MissingIdatBox { item_id } => write!(
                f,
                "primary item_ID {item_id} references idat-backed iloc extents but no idat box exists in meta"
            ),
            ExtractHeicItemDataError::MetaChildBoxes(err) => write!(f, "{err}"),
            ExtractHeicItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset,
                extent_offset,
                extent_length,
            } => write!(
                f,
                "primary item_ID {item_id} iloc extent offset overflow (base_offset={base_offset}, extent_offset={extent_offset}, extent_length={extent_length})"
            ),
            ExtractHeicItemDataError::ExtentOutOfBounds {
                item_id,
                construction_method,
                start,
                length,
                available,
            } => write!(
                f,
                "primary item_ID {item_id} iloc extent (construction_method={construction_method}, start={start}, length={length}) exceeds available bytes {available}"
            ),
            ExtractHeicItemDataError::PayloadTooLarge { item_id, length } => write!(
                f,
                "primary item_ID {item_id} payload length {length} cannot be represented on this platform"
            ),
        }
    }
}

impl Error for ExtractHeicItemDataError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExtractHeicItemDataError::TopLevelBoxes(err) => Some(err),
            ExtractHeicItemDataError::Meta(err) => Some(err),
            ExtractHeicItemDataError::ResolvePrimaryItem(err) => Some(err),
            ExtractHeicItemDataError::MetaChildBoxes(err) => Some(err),
            ExtractHeicItemDataError::GridTileCodecConfiguration { source, .. } => Some(source),
            _ => None,
        }
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

/// Parse primary-item transforms (`clap`/`irot`/`imir`) in property order.
pub fn parse_primary_item_transform_properties(
    input: &[u8],
) -> Result<PrimaryItemTransformProperties, ParsePrimaryItemTransformPropertiesError> {
    // Provenance: mirrors `clap`/`irot`/`imir` parse semantics from
    // libheif/libheif/box.cc:{Box_clap::parse,Box_irot::parse,Box_imir::parse}
    // and preserves associated-property order used by libheif transform application in
    // libheif/libheif/image-items/image_item.cc:ImageItem::decode_compressed_image.
    let top_level =
        parse_boxes(input).map_err(ParsePrimaryItemTransformPropertiesError::TopLevelBoxes)?;
    let meta_box = find_first_child_box(&top_level, META_BOX_TYPE)
        .ok_or(ParsePrimaryItemTransformPropertiesError::MissingMetaBox)?;
    let meta = meta_box
        .parse_meta()
        .map_err(ParsePrimaryItemTransformPropertiesError::Meta)?;
    let resolved = meta
        .resolve_primary_item()
        .map_err(ParsePrimaryItemTransformPropertiesError::ResolvePrimaryItem)?;

    let mut transforms = Vec::new();
    for property in &resolved.primary_item.properties {
        let property_type = property.property.header.box_type;
        if property_type.as_bytes() == CLAP_BOX_TYPE {
            transforms.push(PrimaryItemTransformProperty::CleanAperture(
                property
                    .property
                    .parse_clap()
                    .map_err(ParsePrimaryItemTransformPropertiesError::ImageCleanAperture)?,
            ));
        } else if property_type.as_bytes() == IROT_BOX_TYPE {
            transforms.push(PrimaryItemTransformProperty::Rotation(
                property
                    .property
                    .parse_irot()
                    .map_err(ParsePrimaryItemTransformPropertiesError::ImageRotation)?,
            ));
        } else if property_type.as_bytes() == IMIR_BOX_TYPE {
            transforms.push(PrimaryItemTransformProperty::Mirror(
                property
                    .property
                    .parse_imir()
                    .map_err(ParsePrimaryItemTransformPropertiesError::ImageMirror)?,
            ));
        }
    }

    Ok(PrimaryItemTransformProperties {
        item_id: resolved.primary_item.item_id,
        transforms,
    })
}

/// Parse and validate primary AVIF properties needed before decode.
pub fn parse_primary_avif_item_properties(
    input: &[u8],
) -> Result<AvifPrimaryItemProperties, ParsePrimaryAvifPropertiesError> {
    // Provenance: mirrors av1C/ispe/pixi/colr parse semantics and AVIF decoder
    // preconditions from libheif/libheif/codecs/avif_boxes.cc:Box_av1C::parse,
    // libheif/libheif/box.cc:Box_ispe::parse, libheif/libheif/box.cc:Box_pixi::parse,
    // libheif/libheif/nclx.cc:Box_colr::parse, and
    // libheif/libheif/image-items/avif.cc:ImageItem_AVIF::initialize_decoder.
    let top_level = parse_boxes(input).map_err(ParsePrimaryAvifPropertiesError::TopLevelBoxes)?;
    let meta_box = find_first_child_box(&top_level, META_BOX_TYPE)
        .ok_or(ParsePrimaryAvifPropertiesError::MissingMetaBox)?;
    let meta = meta_box
        .parse_meta()
        .map_err(ParsePrimaryAvifPropertiesError::Meta)?;
    let resolved = meta
        .resolve_primary_item()
        .map_err(ParsePrimaryAvifPropertiesError::ResolvePrimaryItem)?;

    let item_id = resolved.primary_item.item_id;
    let item_type = resolved
        .primary_item
        .item_info
        .item_type
        .ok_or(ParsePrimaryAvifPropertiesError::MissingPrimaryItemType { item_id })?;
    if item_type.as_bytes() != AV01_ITEM_TYPE {
        return Err(ParsePrimaryAvifPropertiesError::UnexpectedPrimaryItemType {
            item_id,
            actual: item_type,
        });
    }

    let mut av1c = None;
    let mut ispe = None;
    let mut pixi = None;
    let mut colr = PrimaryItemColorProperties::default();

    for property in &resolved.primary_item.properties {
        let property_type = property.property.header.box_type;
        if property_type.as_bytes() == AV1C_BOX_TYPE {
            if av1c.is_some() {
                return Err(ParsePrimaryAvifPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            av1c = Some(
                property
                    .property
                    .parse_av1c()
                    .map_err(ParsePrimaryAvifPropertiesError::Av1Codec)?,
            );
        } else if property_type.as_bytes() == ISPE_BOX_TYPE {
            if ispe.is_some() {
                return Err(ParsePrimaryAvifPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            ispe = Some(
                property
                    .property
                    .parse_ispe()
                    .map_err(ParsePrimaryAvifPropertiesError::ImageSpatialExtents)?,
            );
        } else if property_type.as_bytes() == PIXI_BOX_TYPE {
            if pixi.is_some() {
                return Err(ParsePrimaryAvifPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            pixi = Some(
                property
                    .property
                    .parse_pixi()
                    .map_err(ParsePrimaryAvifPropertiesError::PixelInformation)?,
            );
        } else if property_type.as_bytes() == COLR_BOX_TYPE {
            let parsed_colr = property
                .property
                .parse_colr()
                .map_err(ParsePrimaryAvifPropertiesError::ColorInformation)?;
            match parsed_colr.information {
                ColorInformation::Nclx(profile) => {
                    colr.nclx = Some(profile);
                }
                ColorInformation::Icc(profile) => {
                    colr.icc = Some(profile);
                }
            }
        }
    }

    let av1c = av1c.ok_or(ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
        item_id,
        property_type: FourCc::new(AV1C_BOX_TYPE),
    })?;
    let ispe = ispe.ok_or(ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
        item_id,
        property_type: FourCc::new(ISPE_BOX_TYPE),
    })?;
    let pixi = pixi.ok_or(ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
        item_id,
        property_type: FourCc::new(PIXI_BOX_TYPE),
    })?;

    if ispe.width == 0 || ispe.height == 0 {
        return Err(ParsePrimaryAvifPropertiesError::InvalidImageExtent {
            item_id,
            width: ispe.width,
            height: ispe.height,
        });
    }
    if pixi.bits_per_channel.is_empty() {
        return Err(ParsePrimaryAvifPropertiesError::InvalidPixiChannelCount {
            item_id,
            channel_count: 0,
        });
    }
    if let Some((channel_index, _)) = pixi
        .bits_per_channel
        .iter()
        .enumerate()
        .find(|(_, bits)| **bits == 0)
    {
        return Err(ParsePrimaryAvifPropertiesError::InvalidPixiBitsPerChannel {
            item_id,
            channel_index,
        });
    }

    Ok(AvifPrimaryItemProperties {
        item_id,
        av1c,
        ispe,
        pixi,
        colr,
    })
}

/// Parse and validate primary HEIC properties needed before decode.
pub fn parse_primary_heic_item_properties(
    input: &[u8],
) -> Result<HeicPrimaryItemProperties, ParsePrimaryHeicPropertiesError> {
    let preflight = parse_primary_heic_item_preflight_properties(input)?;
    let pixi = preflight
        .pixi
        .ok_or(ParsePrimaryHeicPropertiesError::MissingRequiredProperty {
            item_id: preflight.item_id,
            property_type: FourCc::new(PIXI_BOX_TYPE),
        })?;

    Ok(HeicPrimaryItemProperties {
        item_id: preflight.item_id,
        hvcc: preflight.hvcc,
        ispe: preflight.ispe,
        pixi,
        colr: preflight.colr,
    })
}

/// Parse and validate primary HEIC properties used by decode preflight.
pub fn parse_primary_heic_item_preflight_properties(
    input: &[u8],
) -> Result<HeicPrimaryItemPreflightProperties, ParsePrimaryHeicPropertiesError> {
    // Provenance: mirrors libheif HEIC property preconditions and hvcC/ispe/pixi/colr
    // parse semantics from libheif/libheif/image-items/hevc.cc:ImageItem_HEVC::initialize_decoder,
    // libheif/libheif/context.cc (hvcC presence checks), libheif/libheif/codecs/hevc_boxes.cc:HEVCDecoderConfigurationRecord::parse,
    // libheif/libheif/box.cc:Box_ispe::parse, libheif/libheif/box.cc:Box_pixi::parse,
    // and libheif/libheif/nclx.cc:Box_colr::parse.
    // HEIC preflight for decoder input accepts missing pixi when hvcC/ispe are
    // valid, matching libheif's hvcC-only decode bootstrap path.
    let top_level = parse_boxes(input).map_err(ParsePrimaryHeicPropertiesError::TopLevelBoxes)?;
    let meta_box = find_first_child_box(&top_level, META_BOX_TYPE)
        .ok_or(ParsePrimaryHeicPropertiesError::MissingMetaBox)?;
    let meta = meta_box
        .parse_meta()
        .map_err(ParsePrimaryHeicPropertiesError::Meta)?;
    let resolved = meta
        .resolve_primary_item()
        .map_err(ParsePrimaryHeicPropertiesError::ResolvePrimaryItem)?;

    let item_id = resolved.primary_item.item_id;
    let item_type = resolved
        .primary_item
        .item_info
        .item_type
        .ok_or(ParsePrimaryHeicPropertiesError::MissingPrimaryItemType { item_id })?;
    if item_type.as_bytes() != HVC1_ITEM_TYPE && item_type.as_bytes() != HEV1_ITEM_TYPE {
        return Err(ParsePrimaryHeicPropertiesError::UnexpectedPrimaryItemType {
            item_id,
            actual: item_type,
        });
    }

    let mut hvcc = None;
    let mut ispe = None;
    let mut pixi = None;
    let mut colr = PrimaryItemColorProperties::default();

    for property in &resolved.primary_item.properties {
        let property_type = property.property.header.box_type;
        if property_type.as_bytes() == HVCC_BOX_TYPE {
            if hvcc.is_some() {
                return Err(ParsePrimaryHeicPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            hvcc = Some(
                property
                    .property
                    .parse_hvcc()
                    .map_err(ParsePrimaryHeicPropertiesError::HevcCodec)?,
            );
        } else if property_type.as_bytes() == ISPE_BOX_TYPE {
            if ispe.is_some() {
                return Err(ParsePrimaryHeicPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            ispe = Some(
                property
                    .property
                    .parse_ispe()
                    .map_err(ParsePrimaryHeicPropertiesError::ImageSpatialExtents)?,
            );
        } else if property_type.as_bytes() == PIXI_BOX_TYPE {
            if pixi.is_some() {
                return Err(ParsePrimaryHeicPropertiesError::DuplicateProperty {
                    item_id,
                    property_type,
                });
            }
            pixi = Some(
                property
                    .property
                    .parse_pixi()
                    .map_err(ParsePrimaryHeicPropertiesError::PixelInformation)?,
            );
        } else if property_type.as_bytes() == COLR_BOX_TYPE {
            let parsed_colr = property
                .property
                .parse_colr()
                .map_err(ParsePrimaryHeicPropertiesError::ColorInformation)?;
            match parsed_colr.information {
                ColorInformation::Nclx(profile) => {
                    colr.nclx = Some(profile);
                }
                ColorInformation::Icc(profile) => {
                    colr.icc = Some(profile);
                }
            }
        }
    }

    let hvcc = hvcc.ok_or(ParsePrimaryHeicPropertiesError::MissingRequiredProperty {
        item_id,
        property_type: FourCc::new(HVCC_BOX_TYPE),
    })?;
    let ispe = ispe.ok_or(ParsePrimaryHeicPropertiesError::MissingRequiredProperty {
        item_id,
        property_type: FourCc::new(ISPE_BOX_TYPE),
    })?;

    if ispe.width == 0 || ispe.height == 0 {
        return Err(ParsePrimaryHeicPropertiesError::InvalidImageExtent {
            item_id,
            width: ispe.width,
            height: ispe.height,
        });
    }
    if let Some(pixi) = &pixi {
        if pixi.bits_per_channel.is_empty() {
            return Err(ParsePrimaryHeicPropertiesError::InvalidPixiChannelCount {
                item_id,
                channel_count: 0,
            });
        }
        if let Some((channel_index, _)) = pixi
            .bits_per_channel
            .iter()
            .enumerate()
            .find(|(_, bits)| **bits == 0)
        {
            return Err(ParsePrimaryHeicPropertiesError::InvalidPixiBitsPerChannel {
                item_id,
                channel_index,
            });
        }
    }

    Ok(HeicPrimaryItemPreflightProperties {
        item_id,
        hvcc,
        ispe,
        pixi,
        colr,
    })
}

/// Extract the primary AVIF (`av01`) coded payload from `iloc` extents.
pub fn extract_primary_avif_item_data(
    input: &[u8],
) -> Result<AvifPrimaryItemData, ExtractAvifItemDataError> {
    // Provenance: mirrors the primary-item selection and extent read flow from
    // libheif/libheif/image-items/avif.cc:ImageItem_AVIF::set_decoder_input_data
    // and libheif/libheif/box.cc:Box_iloc::read_data.
    let top_level = parse_boxes(input).map_err(ExtractAvifItemDataError::TopLevelBoxes)?;
    let meta_box = find_first_child_box(&top_level, META_BOX_TYPE)
        .ok_or(ExtractAvifItemDataError::MissingMetaBox)?;
    let meta = meta_box
        .parse_meta()
        .map_err(ExtractAvifItemDataError::Meta)?;
    let resolved = meta
        .resolve_primary_item()
        .map_err(ExtractAvifItemDataError::ResolvePrimaryItem)?;

    let item_id = resolved.primary_item.item_id;
    let item_type = resolved
        .primary_item
        .item_info
        .item_type
        .ok_or(ExtractAvifItemDataError::MissingPrimaryItemType { item_id })?;
    if item_type.as_bytes() != AV01_ITEM_TYPE {
        return Err(ExtractAvifItemDataError::UnexpectedPrimaryItemType {
            item_id,
            actual: item_type,
        });
    }

    let location = &resolved.primary_item.location;
    if location.data_reference_index != 0 {
        return Err(ExtractAvifItemDataError::UnsupportedDataReferenceIndex {
            item_id,
            data_reference_index: location.data_reference_index,
        });
    }

    let total_length = location
        .extents
        .iter()
        .try_fold(0_u64, |acc, extent| acc.checked_add(extent.length))
        .ok_or(ExtractAvifItemDataError::PayloadTooLarge {
            item_id,
            length: u64::MAX,
        })?;
    let payload_capacity =
        usize::try_from(total_length).map_err(|_| ExtractAvifItemDataError::PayloadTooLarge {
            item_id,
            length: total_length,
        })?;
    let mut payload = Vec::with_capacity(payload_capacity);

    match location.construction_method {
        0 => append_iloc_extents_to_payload(input, location, item_id, 0, &mut payload)?,
        1 => {
            let children = meta
                .parse_children()
                .map_err(ExtractAvifItemDataError::MetaChildBoxes)?;
            let idat_box = find_first_child_box(&children, IDAT_BOX_TYPE)
                .ok_or(ExtractAvifItemDataError::MissingIdatBox { item_id })?;
            append_iloc_extents_to_payload(idat_box.payload, location, item_id, 1, &mut payload)?;
        }
        construction_method => {
            return Err(ExtractAvifItemDataError::UnsupportedConstructionMethod {
                item_id,
                construction_method,
            });
        }
    }

    Ok(AvifPrimaryItemData {
        item_id,
        construction_method: location.construction_method,
        payload,
    })
}

/// Extract the primary HEIC (`hvc1`/`hev1`) coded payload from `iloc` extents.
pub fn extract_primary_heic_item_data(
    input: &[u8],
) -> Result<HeicPrimaryItemData, ExtractHeicItemDataError> {
    // Provenance: mirrors the HEIC primary-item selection and extent read flow
    // from libheif/libheif/image-items/hevc.cc:ImageItem_HEVC::set_decoder_input_data,
    // libheif/libheif/box.cc:Box_iloc::read_data, and
    // libheif/libheif/box.cc:Box_idat::read_data.
    let (meta, resolved) = resolve_primary_heic_item_graph(input)?;

    let item_id = resolved.primary_item.item_id;
    let item_type = resolved
        .primary_item
        .item_info
        .item_type
        .ok_or(ExtractHeicItemDataError::MissingPrimaryItemType { item_id })?;
    if item_type.as_bytes() != HVC1_ITEM_TYPE && item_type.as_bytes() != HEV1_ITEM_TYPE {
        return Err(ExtractHeicItemDataError::UnexpectedPrimaryItemType {
            item_id,
            actual: item_type,
        });
    }

    let (construction_method, payload) = extract_heic_item_payload_from_location(
        input,
        &meta,
        &resolved.primary_item.location,
        item_id,
    )?;

    Ok(HeicPrimaryItemData {
        item_id,
        construction_method,
        payload,
    })
}

/// Extract primary HEIC item data, including `grid` item descriptor + tiles.
pub fn extract_primary_heic_item_data_with_grid(
    input: &[u8],
) -> Result<HeicPrimaryItemDataWithGrid, ExtractHeicItemDataError> {
    // Provenance: mirrors libheif `grid` descriptor parsing and tile-reference
    // resolution in libheif/libheif/image-items/grid.cc:{ImageGrid::parse,ImageItem_Grid::read_grid_spec},
    // with coded tile payload extraction reusing the same iloc/idat data flow as
    // libheif/libheif/box.cc:{Box_iloc::read_data,Box_idat::read_data}.
    let (meta, resolved) = resolve_primary_heic_item_graph(input)?;

    let item_id = resolved.primary_item.item_id;
    let item_type = resolved
        .primary_item
        .item_info
        .item_type
        .ok_or(ExtractHeicItemDataError::MissingPrimaryItemType { item_id })?;

    if item_type.as_bytes() == HVC1_ITEM_TYPE || item_type.as_bytes() == HEV1_ITEM_TYPE {
        let (construction_method, payload) = extract_heic_item_payload_from_location(
            input,
            &meta,
            &resolved.primary_item.location,
            item_id,
        )?;
        return Ok(HeicPrimaryItemDataWithGrid::Coded(HeicPrimaryItemData {
            item_id,
            construction_method,
            payload,
        }));
    }

    if item_type.as_bytes() != GRID_ITEM_TYPE {
        return Err(ExtractHeicItemDataError::UnexpectedPrimaryItemType {
            item_id,
            actual: item_type,
        });
    }

    let (construction_method, grid_payload) = extract_heic_item_payload_from_location(
        input,
        &meta,
        &resolved.primary_item.location,
        item_id,
    )?;
    let descriptor = parse_heic_grid_descriptor(item_id, &grid_payload)?;

    let tile_item_ids: Vec<u32> = resolved
        .primary_item
        .references
        .iter()
        .filter(|reference| reference.reference_type.as_bytes() == DIMG_REFERENCE_TYPE)
        .flat_map(|reference| reference.to_item_ids.iter().copied())
        .collect();
    if tile_item_ids.is_empty() {
        return Err(ExtractHeicItemDataError::MissingGridTileReferences { item_id });
    }

    let expected_tiles_u64 = u64::from(descriptor.rows) * u64::from(descriptor.columns);
    let expected_tiles = usize::try_from(expected_tiles_u64).map_err(|_| {
        ExtractHeicItemDataError::PayloadTooLarge {
            item_id,
            length: expected_tiles_u64,
        }
    })?;
    if tile_item_ids.len() != expected_tiles {
        return Err(ExtractHeicItemDataError::GridTileCountMismatch {
            item_id,
            rows: descriptor.rows,
            columns: descriptor.columns,
            expected: expected_tiles,
            actual: tile_item_ids.len(),
        });
    }

    let mut flattened_properties = Vec::new();
    for property_container in &resolved.iprp.property_containers {
        flattened_properties.extend(property_container.properties.iter().cloned());
    }

    let mut tiles = Vec::with_capacity(tile_item_ids.len());
    for &tile_item_id in &tile_item_ids {
        let tile_item_info = resolved
            .iinf
            .entries
            .iter()
            .find(|entry| entry.item_id == tile_item_id)
            .ok_or(ExtractHeicItemDataError::MissingGridTileItemInfo {
                item_id,
                tile_item_id,
            })?;
        let tile_item_type =
            tile_item_info
                .item_type
                .ok_or(ExtractHeicItemDataError::MissingGridTileItemType {
                    item_id,
                    tile_item_id,
                })?;
        if tile_item_type.as_bytes() != HVC1_ITEM_TYPE
            && tile_item_type.as_bytes() != HEV1_ITEM_TYPE
        {
            return Err(ExtractHeicItemDataError::UnexpectedGridTileItemType {
                item_id,
                tile_item_id,
                actual: tile_item_type,
            });
        }

        let tile_hvcc = resolve_grid_tile_hvcc(
            item_id,
            tile_item_id,
            &resolved.iprp.associations,
            &flattened_properties,
        )?;

        let tile_location = resolved
            .iloc
            .items
            .iter()
            .find(|item| item.item_id == tile_item_id)
            .ok_or(ExtractHeicItemDataError::MissingGridTileLocation {
                item_id,
                tile_item_id,
            })?;
        let (tile_construction_method, payload) =
            extract_heic_item_payload_from_location(input, &meta, tile_location, tile_item_id)?;
        tiles.push(HeicGridTileItemData {
            item_id: tile_item_id,
            construction_method: tile_construction_method,
            hvcc: tile_hvcc,
            payload,
        });
    }

    Ok(HeicPrimaryItemDataWithGrid::Grid(HeicGridPrimaryItemData {
        item_id,
        construction_method,
        descriptor,
        tile_item_ids,
        tiles,
    }))
}

fn resolve_grid_tile_hvcc(
    item_id: u32,
    tile_item_id: u32,
    associations: &[ItemPropertyAssociationBox],
    flattened_properties: &[ParsedBox<'_>],
) -> Result<HevcDecoderConfigurationBox, ExtractHeicItemDataError> {
    let mut hvcc = None;

    for association_box in associations {
        for entry in &association_box.entries {
            if entry.item_id != tile_item_id {
                continue;
            }

            for association in &entry.associations {
                if association.property_index == 0 {
                    continue;
                }

                let property_index = usize::from(association.property_index - 1);
                if property_index >= flattened_properties.len() {
                    return Err(ExtractHeicItemDataError::GridTilePropertyIndexOutOfRange {
                        item_id,
                        tile_item_id,
                        property_index: association.property_index,
                        available: flattened_properties.len(),
                    });
                }
                let property = &flattened_properties[property_index];
                if property.header.box_type.as_bytes() != HVCC_BOX_TYPE {
                    continue;
                }

                if hvcc.is_some() {
                    return Err(
                        ExtractHeicItemDataError::DuplicateGridTileCodecConfiguration {
                            item_id,
                            tile_item_id,
                        },
                    );
                }

                hvcc = Some(property.parse_hvcc().map_err(|source| {
                    ExtractHeicItemDataError::GridTileCodecConfiguration {
                        item_id,
                        tile_item_id,
                        source,
                    }
                })?);
            }
        }
    }

    hvcc.ok_or(
        ExtractHeicItemDataError::MissingGridTileCodecConfiguration {
            item_id,
            tile_item_id,
        },
    )
}

fn resolve_primary_heic_item_graph<'a>(
    input: &'a [u8],
) -> Result<(MetaBox<'a>, ResolvedPrimaryItemGraph<'a>), ExtractHeicItemDataError> {
    let top_level = parse_boxes(input).map_err(ExtractHeicItemDataError::TopLevelBoxes)?;
    let meta_box = find_first_child_box(&top_level, META_BOX_TYPE)
        .ok_or(ExtractHeicItemDataError::MissingMetaBox)?;
    let meta = meta_box
        .parse_meta()
        .map_err(ExtractHeicItemDataError::Meta)?;
    let resolved = meta
        .resolve_primary_item()
        .map_err(ExtractHeicItemDataError::ResolvePrimaryItem)?;
    Ok((meta, resolved))
}

fn extract_heic_item_payload_from_location(
    input: &[u8],
    meta: &MetaBox<'_>,
    location: &ItemLocationItem,
    item_id: u32,
) -> Result<(u8, Vec<u8>), ExtractHeicItemDataError> {
    if location.data_reference_index != 0 {
        return Err(ExtractHeicItemDataError::UnsupportedDataReferenceIndex {
            item_id,
            data_reference_index: location.data_reference_index,
        });
    }

    let total_length = location
        .extents
        .iter()
        .try_fold(0_u64, |acc, extent| acc.checked_add(extent.length))
        .ok_or(ExtractHeicItemDataError::PayloadTooLarge {
            item_id,
            length: u64::MAX,
        })?;
    let payload_capacity =
        usize::try_from(total_length).map_err(|_| ExtractHeicItemDataError::PayloadTooLarge {
            item_id,
            length: total_length,
        })?;
    let mut payload = Vec::with_capacity(payload_capacity);

    match location.construction_method {
        0 => append_iloc_extents_to_payload_for_heic(input, location, item_id, 0, &mut payload)?,
        1 => {
            let children = meta
                .parse_children()
                .map_err(ExtractHeicItemDataError::MetaChildBoxes)?;
            let idat_box = find_first_child_box(&children, IDAT_BOX_TYPE)
                .ok_or(ExtractHeicItemDataError::MissingIdatBox { item_id })?;
            append_iloc_extents_to_payload_for_heic(
                idat_box.payload,
                location,
                item_id,
                1,
                &mut payload,
            )?;
        }
        construction_method => {
            return Err(ExtractHeicItemDataError::UnsupportedConstructionMethod {
                item_id,
                construction_method,
            });
        }
    }

    Ok((location.construction_method, payload))
}

fn parse_heic_grid_descriptor(
    item_id: u32,
    payload: &[u8],
) -> Result<HeicGridDescriptor, ExtractHeicItemDataError> {
    const GRID_MINIMUM_SIZE: usize = 8;
    if payload.len() < GRID_MINIMUM_SIZE {
        return Err(ExtractHeicItemDataError::GridDescriptorTooSmall {
            item_id,
            available: payload.len(),
            required: GRID_MINIMUM_SIZE,
        });
    }

    let version = payload[0];
    if version != 0 {
        return Err(ExtractHeicItemDataError::UnsupportedGridDescriptorVersion {
            item_id,
            version,
        });
    }

    let flags = payload[1];
    let field_uses_32_bits = (flags & 0x01) != 0;
    let rows = u16::from(payload[2]) + 1;
    let columns = u16::from(payload[3]) + 1;
    let (required, output_width, output_height) = if field_uses_32_bits {
        let required = 12;
        if payload.len() < required {
            return Err(ExtractHeicItemDataError::GridDescriptorTooSmall {
                item_id,
                available: payload.len(),
                required,
            });
        }

        (
            required,
            read_u32_be(&payload[4..8]),
            read_u32_be(&payload[8..12]),
        )
    } else {
        (
            GRID_MINIMUM_SIZE,
            u32::from(read_u16_be(&payload[4..6])),
            u32::from(read_u16_be(&payload[6..8])),
        )
    };

    if payload.len() < required {
        return Err(ExtractHeicItemDataError::GridDescriptorTooSmall {
            item_id,
            available: payload.len(),
            required,
        });
    }

    Ok(HeicGridDescriptor {
        version,
        rows,
        columns,
        output_width,
        output_height,
    })
}

fn append_iloc_extents_to_payload(
    source: &[u8],
    location: &ItemLocationItem,
    item_id: u32,
    construction_method: u8,
    output: &mut Vec<u8>,
) -> Result<(), ExtractAvifItemDataError> {
    let available = source.len() as u64;

    for extent in &location.extents {
        let start = location.base_offset.checked_add(extent.offset).ok_or(
            ExtractAvifItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset: location.base_offset,
                extent_offset: extent.offset,
                extent_length: extent.length,
            },
        )?;
        let end = start.checked_add(extent.length).ok_or(
            ExtractAvifItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset: location.base_offset,
                extent_offset: extent.offset,
                extent_length: extent.length,
            },
        )?;

        if end > available {
            return Err(ExtractAvifItemDataError::ExtentOutOfBounds {
                item_id,
                construction_method,
                start,
                length: extent.length,
                available,
            });
        }

        let start =
            usize::try_from(start).map_err(|_| ExtractAvifItemDataError::PayloadTooLarge {
                item_id,
                length: end,
            })?;
        let end = usize::try_from(end).map_err(|_| ExtractAvifItemDataError::PayloadTooLarge {
            item_id,
            length: end,
        })?;
        output.extend_from_slice(&source[start..end]);
    }

    Ok(())
}

fn append_iloc_extents_to_payload_for_heic(
    source: &[u8],
    location: &ItemLocationItem,
    item_id: u32,
    construction_method: u8,
    output: &mut Vec<u8>,
) -> Result<(), ExtractHeicItemDataError> {
    let available = source.len() as u64;

    for extent in &location.extents {
        let start = location.base_offset.checked_add(extent.offset).ok_or(
            ExtractHeicItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset: location.base_offset,
                extent_offset: extent.offset,
                extent_length: extent.length,
            },
        )?;
        let end = start.checked_add(extent.length).ok_or(
            ExtractHeicItemDataError::ExtentOffsetOverflow {
                item_id,
                base_offset: location.base_offset,
                extent_offset: extent.offset,
                extent_length: extent.length,
            },
        )?;

        if end > available {
            return Err(ExtractHeicItemDataError::ExtentOutOfBounds {
                item_id,
                construction_method,
                start,
                length: extent.length,
                available,
            });
        }

        let start =
            usize::try_from(start).map_err(|_| ExtractHeicItemDataError::PayloadTooLarge {
                item_id,
                length: end,
            })?;
        let end = usize::try_from(end).map_err(|_| ExtractHeicItemDataError::PayloadTooLarge {
            item_id,
            length: end,
        })?;
        output.extend_from_slice(&source[start..end]);
    }

    Ok(())
}

fn find_first_child_box<'a, 'b>(
    children: &'b [ParsedBox<'a>],
    box_type: [u8; 4],
) -> Option<&'b ParsedBox<'a>> {
    children
        .iter()
        .find(|child| child.header.box_type.as_bytes() == box_type)
}

fn parse_av1c_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<Av1CodecConfigurationBox, ParseAv1CodecConfigurationBoxError> {
    if payload.len() < 4 {
        return Err(ParseAv1CodecConfigurationBoxError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
            required: 4,
        });
    }

    let byte0 = payload[0];
    if (byte0 & 0x80) == 0 {
        return Err(ParseAv1CodecConfigurationBoxError::InvalidMarkerBit {
            offset: payload_offset,
            value: byte0,
        });
    }

    let byte1 = payload[1];
    let byte2 = payload[2];
    let byte3 = payload[3];
    let initial_presentation_delay_present = (byte3 & 0x10) != 0;

    Ok(Av1CodecConfigurationBox {
        marker: true,
        version: byte0 & 0x7F,
        seq_profile: (byte1 >> 5) & 0x07,
        seq_level_idx_0: byte1 & 0x1F,
        seq_tier_0: (byte2 & 0x80) != 0,
        high_bitdepth: (byte2 & 0x40) != 0,
        twelve_bit: (byte2 & 0x20) != 0,
        monochrome: (byte2 & 0x10) != 0,
        chroma_subsampling_x: (byte2 & 0x08) != 0,
        chroma_subsampling_y: (byte2 & 0x04) != 0,
        chroma_sample_position: byte2 & 0x03,
        initial_presentation_delay_present,
        initial_presentation_delay_minus_one: initial_presentation_delay_present
            .then_some(byte3 & 0x0F),
        config_obus: payload[4..].to_vec(),
    })
}

fn parse_hvcc_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<HevcDecoderConfigurationBox, ParseHevcDecoderConfigurationBoxError> {
    // Provenance: mirrors HEVCDecoderConfigurationRecord::parse from
    // libheif/libheif/codecs/hevc_boxes.cc (field extraction order, masking,
    // nal-array iteration, and tolerance for trailing bytes).
    let mut cursor = 0_usize;
    let configuration_version =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "configurationVersion")?;
    if configuration_version != 1 {
        return Err(
            ParseHevcDecoderConfigurationBoxError::UnsupportedConfigurationVersion {
                offset: payload_offset,
                version: configuration_version,
            },
        );
    }

    let profile_byte =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "general_profile")?;
    let general_profile_space = (profile_byte >> 6) & 0x03;
    let general_tier_flag = (profile_byte & 0x20) != 0;
    let general_profile_idc = profile_byte & 0x1F;
    let general_profile_compatibility_flags = read_u32_cursor_hvcc(
        payload,
        &mut cursor,
        payload_offset,
        "general_profile_compatibility_flags",
    )?;
    let constraint_bytes = take_cursor_bytes_hvcc(
        payload,
        &mut cursor,
        6,
        payload_offset,
        "general_constraint_indicator_flags",
    )?;
    let mut general_constraint_indicator_flags = [0_u8; 6];
    general_constraint_indicator_flags.copy_from_slice(constraint_bytes);

    let general_level_idc =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "general_level_idc")?;
    let min_spatial_segmentation_idc = read_u16_cursor_hvcc(
        payload,
        &mut cursor,
        payload_offset,
        "min_spatial_segmentation_idc",
    )? & 0x0FFF;
    let parallelism_type =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "parallelism_type")? & 0x03;
    let chroma_format =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "chroma_format")? & 0x03;
    let bit_depth_luma =
        (read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "bit_depth_luma")? & 0x07) + 8;
    let bit_depth_chroma =
        (read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "bit_depth_chroma")? & 0x07) + 8;
    let avg_frame_rate =
        read_u16_cursor_hvcc(payload, &mut cursor, payload_offset, "avg_frame_rate")?;
    let temporal_byte =
        read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "temporal_layer_flags")?;
    let constant_frame_rate = (temporal_byte >> 6) & 0x03;
    let num_temporal_layers = (temporal_byte >> 3) & 0x07;
    let temporal_id_nested = (temporal_byte & 0x04) != 0;
    let nal_length_size = (temporal_byte & 0x03) + 1;

    let nal_array_count = usize::from(read_u8_cursor_hvcc(
        payload,
        &mut cursor,
        payload_offset,
        "num_of_arrays",
    )?);
    let mut nal_arrays = Vec::with_capacity(nal_array_count);
    for _ in 0..nal_array_count {
        let array_header =
            read_u8_cursor_hvcc(payload, &mut cursor, payload_offset, "nal_array_header")?;
        let array_completeness = (array_header & 0x40) != 0;
        let nal_unit_type = array_header & 0x3F;
        let nal_unit_count = usize::from(read_u16_cursor_hvcc(
            payload,
            &mut cursor,
            payload_offset,
            "nal_unit_count",
        )?);
        let mut nal_units = Vec::with_capacity(nal_unit_count);
        for _ in 0..nal_unit_count {
            let nal_unit_length = usize::from(read_u16_cursor_hvcc(
                payload,
                &mut cursor,
                payload_offset,
                "nal_unit_length",
            )?);
            if nal_unit_length == 0 {
                continue;
            }
            let nal_unit = take_cursor_bytes_hvcc(
                payload,
                &mut cursor,
                nal_unit_length,
                payload_offset,
                "nal_unit_data",
            )?
            .to_vec();
            nal_units.push(nal_unit);
        }

        nal_arrays.push(HevcNalArray {
            array_completeness,
            nal_unit_type,
            nal_units,
        });
    }

    Ok(HevcDecoderConfigurationBox {
        configuration_version,
        general_profile_space,
        general_tier_flag,
        general_profile_idc,
        general_profile_compatibility_flags,
        general_constraint_indicator_flags,
        general_level_idc,
        min_spatial_segmentation_idc,
        parallelism_type,
        chroma_format,
        bit_depth_luma,
        bit_depth_chroma,
        avg_frame_rate,
        constant_frame_rate,
        num_temporal_layers,
        temporal_id_nested,
        nal_length_size,
        nal_arrays,
    })
}

fn parse_ispe_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ImageSpatialExtentsProperty, ParseImageSpatialExtentsPropertyError> {
    // Provenance: mirrors libheif/libheif/box.cc:Box_ispe::parse FullBox and
    // width/height extraction (version 0, then 32-bit width/height).
    let (full_box, ispe_payload, ispe_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version != 0 {
        return Err(ParseImageSpatialExtentsPropertyError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    let required = size_of::<u32>() * 2;
    if ispe_payload.len() < required {
        return Err(ParseImageSpatialExtentsPropertyError::PayloadTooSmall {
            offset: ispe_payload_offset,
            available: ispe_payload.len(),
            required,
        });
    }

    Ok(ImageSpatialExtentsProperty {
        full_box,
        width: read_u32_be(&ispe_payload[0..4]),
        height: read_u32_be(&ispe_payload[4..8]),
    })
}

fn parse_pixi_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<PixelInformationProperty, ParsePixelInformationPropertyError> {
    // Provenance: mirrors libheif/libheif/box.cc:Box_pixi::parse (FullBox
    // version 0 + 8-bit channel count + N bits_per_channel values).
    let (full_box, pixi_payload, pixi_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version != 0 {
        return Err(ParsePixelInformationPropertyError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    if pixi_payload.is_empty() {
        return Err(ParsePixelInformationPropertyError::PayloadTooSmall {
            offset: pixi_payload_offset,
            context: "num_channels",
            available: 0,
            required: 1,
        });
    }

    let num_channels = usize::from(pixi_payload[0]);
    let available_channels = pixi_payload.len().saturating_sub(1);
    if available_channels < num_channels {
        return Err(ParsePixelInformationPropertyError::PayloadTooSmall {
            offset: pixi_payload_offset + 1,
            context: "bits_per_channel",
            available: available_channels,
            required: num_channels,
        });
    }

    Ok(PixelInformationProperty {
        full_box,
        bits_per_channel: pixi_payload[1..(1 + num_channels)].to_vec(),
    })
}

fn parse_colr_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ColorInformationProperty, ParseColorInformationPropertyError> {
    // Provenance: mirrors libheif/libheif/nclx.cc:Box_colr::parse handling for
    // nclx/nclc and ICC (prof/rICC) colour profile payload variants.
    if payload.len() < BRAND_FIELD_SIZE {
        return Err(ParseColorInformationPropertyError::PayloadTooSmall {
            offset: payload_offset,
            context: "colour_type",
            available: payload.len(),
            required: BRAND_FIELD_SIZE,
        });
    }

    let colour_type = read_fourcc(&payload[0..BRAND_FIELD_SIZE]);
    let profile_payload = &payload[BRAND_FIELD_SIZE..];
    let profile_payload_offset = payload_offset + BRAND_FIELD_SIZE as u64;

    if colour_type.as_bytes() == NCLX_COLOR_TYPE {
        let required = size_of::<u16>() * 3 + size_of::<u8>();
        if profile_payload.len() < required {
            return Err(ParseColorInformationPropertyError::PayloadTooSmall {
                offset: profile_payload_offset,
                context: "nclx_profile",
                available: profile_payload.len(),
                required,
            });
        }

        return Ok(ColorInformationProperty {
            colour_type,
            information: ColorInformation::Nclx(NclxColorProfile {
                colour_primaries: read_u16_be(&profile_payload[0..2]),
                transfer_characteristics: read_u16_be(&profile_payload[2..4]),
                matrix_coefficients: read_u16_be(&profile_payload[4..6]),
                full_range_flag: (profile_payload[6] & 0x80) != 0,
            }),
        });
    }

    if colour_type.as_bytes() == NCLC_COLOR_TYPE {
        let required = size_of::<u16>() * 3;
        if profile_payload.len() < required {
            return Err(ParseColorInformationPropertyError::PayloadTooSmall {
                offset: profile_payload_offset,
                context: "nclc_profile",
                available: profile_payload.len(),
                required,
            });
        }

        let matrix_coefficients = read_u16_be(&profile_payload[4..6]);
        return Ok(ColorInformationProperty {
            colour_type,
            information: ColorInformation::Nclx(NclxColorProfile {
                colour_primaries: read_u16_be(&profile_payload[0..2]),
                transfer_characteristics: read_u16_be(&profile_payload[2..4]),
                matrix_coefficients,
                full_range_flag: matrix_coefficients == 0,
            }),
        });
    }

    if colour_type.as_bytes() == PROF_COLOR_TYPE || colour_type.as_bytes() == RICC_COLOR_TYPE {
        return Ok(ColorInformationProperty {
            colour_type,
            information: ColorInformation::Icc(IccColorProfile {
                profile_type: colour_type,
                profile: profile_payload.to_vec(),
            }),
        });
    }

    Err(ParseColorInformationPropertyError::UnknownColourType {
        offset: payload_offset,
        colour_type,
    })
}

fn parse_irot_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ImageRotationProperty, ParseImageRotationPropertyError> {
    // Provenance: mirrors libheif/libheif/box.cc:Box_irot::parse, where the
    // low two bits encode rotation units of 90 degrees.
    let required = size_of::<u8>();
    if payload.len() < required {
        return Err(ParseImageRotationPropertyError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
            required,
        });
    }

    let rotation_ccw_degrees = u16::from(payload[0] & 0x03) * 90;
    Ok(ImageRotationProperty {
        rotation_ccw_degrees,
    })
}

fn parse_imir_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ImageMirrorProperty, ParseImageMirrorPropertyError> {
    // Provenance: mirrors libheif/libheif/box.cc:Box_imir::parse, where bit 0
    // selects mirror direction (1=horizontal, 0=vertical).
    let required = size_of::<u8>();
    if payload.len() < required {
        return Err(ParseImageMirrorPropertyError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
            required,
        });
    }

    let direction = if (payload[0] & 0x01) != 0 {
        ImageMirrorDirection::Horizontal
    } else {
        ImageMirrorDirection::Vertical
    };
    Ok(ImageMirrorProperty { direction })
}

fn parse_clap_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ImageCleanApertureProperty, ParseImageCleanAperturePropertyError> {
    // Provenance: mirrors libheif/libheif/box.cc:Box_clap::parse field order
    // and denominator validity checks.
    const CLAP_PAYLOAD_SIZE: usize = 32;
    let required = CLAP_PAYLOAD_SIZE;
    if payload.len() < required {
        return Err(ParseImageCleanAperturePropertyError::PayloadTooSmall {
            offset: payload_offset,
            available: payload.len(),
            required,
        });
    }

    let clean_aperture_width_num = read_u32_be(&payload[0..4]);
    let clean_aperture_width_den = read_u32_be(&payload[4..8]);
    let clean_aperture_height_num = read_u32_be(&payload[8..12]);
    let clean_aperture_height_den = read_u32_be(&payload[12..16]);
    let horizontal_offset_num = read_i32_be(&payload[16..20]);
    let horizontal_offset_den = read_u32_be(&payload[20..24]);
    let vertical_offset_num = read_i32_be(&payload[24..28]);
    let vertical_offset_den = read_u32_be(&payload[28..32]);

    for (field, denominator, byte_offset) in [
        (
            "clean_aperture_width_den",
            clean_aperture_width_den,
            payload_offset + 4,
        ),
        (
            "clean_aperture_height_den",
            clean_aperture_height_den,
            payload_offset + 12,
        ),
        (
            "horizontal_offset_den",
            horizontal_offset_den,
            payload_offset + 20,
        ),
        (
            "vertical_offset_den",
            vertical_offset_den,
            payload_offset + 28,
        ),
    ] {
        if denominator == 0 {
            return Err(ParseImageCleanAperturePropertyError::ZeroDenominator {
                offset: byte_offset,
                field,
            });
        }
    }

    Ok(ImageCleanApertureProperty {
        clean_aperture_width_num,
        clean_aperture_width_den,
        clean_aperture_height_num,
        clean_aperture_height_den,
        horizontal_offset_num,
        horizontal_offset_den,
        vertical_offset_num,
        vertical_offset_den,
    })
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

fn parse_iref_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ItemReferenceBox, ParseItemReferenceBoxError> {
    // Provenance: mirrors iref parsing in libheif/libheif/box.cc:Box_iref::parse,
    // where versions 0/1 are supported, each typed child box carries one
    // from_item_ID, a 16-bit reference count, and that many to_item_ID values
    // using ID width selected by the iref version.
    let (full_box, iref_payload, iref_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version > 1 {
        return Err(ParseItemReferenceBoxError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    let children =
        BoxIter::with_offset(iref_payload, iref_payload_offset).collect::<Result<Vec<_>, _>>()?;

    let mut references = Vec::with_capacity(children.len());
    for child in children {
        let mut cursor = 0_usize;
        let from_item_id = read_item_id_cursor_iref(
            child.payload,
            &mut cursor,
            child.payload_offset(),
            full_box.version,
            "from_item_ID",
        )?;
        let reference_count = read_u16_cursor_iref(
            child.payload,
            &mut cursor,
            child.payload_offset(),
            "reference_count",
        )?;
        if reference_count == 0 {
            return Err(ParseItemReferenceBoxError::EmptyReferenceList {
                offset: child.payload_offset() + cursor as u64 - size_of::<u16>() as u64,
                reference_type: child.header.box_type,
            });
        }

        let mut to_item_ids = Vec::with_capacity(usize::from(reference_count));
        for _ in 0..reference_count {
            to_item_ids.push(read_item_id_cursor_iref(
                child.payload,
                &mut cursor,
                child.payload_offset(),
                full_box.version,
                "to_item_ID",
            )?);
        }

        references.push(ItemReferenceEntry {
            reference_type: child.header.box_type,
            from_item_id,
            to_item_ids,
        });
    }

    Ok(ItemReferenceBox {
        full_box,
        references,
    })
}

fn parse_ipco_payload<'a>(
    payload: &'a [u8],
    payload_offset: u64,
) -> Result<ItemPropertyContainerBox<'a>, ParseItemPropertyContainerBoxError> {
    // Provenance: mirrors libheif `ipco` handling in
    // libheif/libheif/box.cc:Box_ipco::parse, which parses all child property
    // boxes under ipco.
    let properties =
        BoxIter::with_offset(payload, payload_offset).collect::<Result<Vec<_>, ParseBoxError>>()?;

    Ok(ItemPropertyContainerBox { properties })
}

fn parse_ipma_payload(
    payload: &[u8],
    payload_offset: u64,
) -> Result<ItemPropertyAssociationBox, ParseItemPropertyAssociationBoxError> {
    // Provenance: mirrors libheif `ipma` entry parsing in
    // libheif/libheif/box.cc:Box_ipma::parse, including version checks (0/1),
    // item_ID width selection (v0=16-bit, v1=32-bit), and association entry
    // decoding controlled by flags bit 0 (8-bit or 16-bit indices).
    let (full_box, ipma_payload, ipma_payload_offset) =
        parse_full_box_payload(payload, payload_offset)?;
    if full_box.version > 1 {
        return Err(ParseItemPropertyAssociationBoxError::UnsupportedVersion {
            offset: payload_offset,
            version: full_box.version,
        });
    }

    let mut cursor = 0_usize;
    let entry_count = read_u32_cursor_ipma(
        ipma_payload,
        &mut cursor,
        ipma_payload_offset,
        "entry_count",
    )?;
    let entry_capacity = usize::try_from(entry_count).map_err(|_| {
        ParseItemPropertyAssociationBoxError::EntryCountTooLarge {
            offset: ipma_payload_offset,
            entry_count,
        }
    })?;

    let mut entries = Vec::with_capacity(entry_capacity);
    for _ in 0..entry_count {
        let item_id = if full_box.version == 0 {
            u32::from(read_u16_cursor_ipma(
                ipma_payload,
                &mut cursor,
                ipma_payload_offset,
                "item_ID",
            )?)
        } else {
            read_u32_cursor_ipma(ipma_payload, &mut cursor, ipma_payload_offset, "item_ID")?
        };
        let association_count = read_u8_cursor_ipma(
            ipma_payload,
            &mut cursor,
            ipma_payload_offset,
            "association_count",
        )?;
        let mut associations = Vec::with_capacity(usize::from(association_count));
        for _ in 0..association_count {
            let association = if (full_box.flags & 0x1) != 0 {
                let value = read_u16_cursor_ipma(
                    ipma_payload,
                    &mut cursor,
                    ipma_payload_offset,
                    "association",
                )?;
                ItemPropertyAssociation {
                    essential: (value & 0x8000) != 0,
                    property_index: value & 0x7FFF,
                }
            } else {
                let value = read_u8_cursor_ipma(
                    ipma_payload,
                    &mut cursor,
                    ipma_payload_offset,
                    "association",
                )?;
                ItemPropertyAssociation {
                    essential: (value & 0x80) != 0,
                    property_index: u16::from(value & 0x7F),
                }
            };
            associations.push(association);
        }

        entries.push(ItemPropertyAssociationEntry {
            item_id,
            associations,
        });
    }

    Ok(ItemPropertyAssociationBox {
        full_box,
        entry_count,
        entries,
    })
}

fn parse_iprp_payload<'a>(
    payload: &'a [u8],
    payload_offset: u64,
) -> Result<ItemPropertiesBox<'a>, ParseItemPropertiesBoxError> {
    // Provenance: mirrors libheif `iprp` handling in
    // libheif/libheif/box.cc:Box_iprp::parse, where all child boxes are read
    // and `ipco`/`ipma` children are interpreted by downstream consumers.
    let children =
        BoxIter::with_offset(payload, payload_offset).collect::<Result<Vec<_>, ParseBoxError>>()?;

    let mut property_containers = Vec::new();
    let mut associations = Vec::new();
    for child in children {
        let child_type = child.header.box_type.as_bytes();
        if child_type == IPCO_BOX_TYPE {
            property_containers.push(parse_ipco_payload(child.payload, child.payload_offset())?);
        } else if child_type == IPMA_BOX_TYPE {
            associations.push(parse_ipma_payload(child.payload, child.payload_offset())?);
        }
    }

    Ok(ItemPropertiesBox {
        property_containers,
        associations,
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

fn read_item_id_cursor_iref(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    version: u8,
    context: &'static str,
) -> Result<u32, ParseItemReferenceBoxError> {
    if version == 0 {
        Ok(u32::from(read_u16_cursor_iref(
            payload,
            cursor,
            payload_offset,
            context,
        )?))
    } else {
        read_u32_cursor_iref(payload, cursor, payload_offset, context)
    }
}

fn read_u16_cursor_iref(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseItemReferenceBoxError> {
    let bytes = take_cursor_bytes_iref(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor_iref(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseItemReferenceBoxError> {
    let bytes = take_cursor_bytes_iref(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn take_cursor_bytes_iref<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseItemReferenceBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseItemReferenceBoxError::PayloadTooSmall {
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

fn read_u8_cursor_hvcc(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u8, ParseHevcDecoderConfigurationBoxError> {
    let bytes = take_cursor_bytes_hvcc(payload, cursor, size_of::<u8>(), payload_offset, context)?;
    Ok(bytes[0])
}

fn read_u16_cursor_hvcc(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseHevcDecoderConfigurationBoxError> {
    let bytes = take_cursor_bytes_hvcc(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor_hvcc(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseHevcDecoderConfigurationBoxError> {
    let bytes = take_cursor_bytes_hvcc(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn take_cursor_bytes_hvcc<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseHevcDecoderConfigurationBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseHevcDecoderConfigurationBoxError::PayloadTooSmall {
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

fn read_u8_cursor_ipma(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u8, ParseItemPropertyAssociationBoxError> {
    let bytes = take_cursor_bytes_ipma(payload, cursor, size_of::<u8>(), payload_offset, context)?;
    Ok(bytes[0])
}

fn read_u16_cursor_ipma(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u16, ParseItemPropertyAssociationBoxError> {
    let bytes = take_cursor_bytes_ipma(payload, cursor, size_of::<u16>(), payload_offset, context)?;
    Ok(read_u16_be(bytes))
}

fn read_u32_cursor_ipma(
    payload: &[u8],
    cursor: &mut usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<u32, ParseItemPropertyAssociationBoxError> {
    let bytes = take_cursor_bytes_ipma(payload, cursor, size_of::<u32>(), payload_offset, context)?;
    Ok(read_u32_be(bytes))
}

fn take_cursor_bytes_ipma<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    size: usize,
    payload_offset: u64,
    context: &'static str,
) -> Result<&'a [u8], ParseItemPropertyAssociationBoxError> {
    let start = *cursor;
    let available = payload.len().saturating_sub(start);
    if available < size {
        return Err(ParseItemPropertyAssociationBoxError::PayloadTooSmall {
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

fn read_i32_be(input: &[u8]) -> i32 {
    i32::from_be_bytes([input[0], input[1], input[2], input[3]])
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
        extract_primary_avif_item_data, extract_primary_heic_item_data,
        extract_primary_heic_item_data_with_grid, parse_boxes, parse_primary_avif_item_properties,
        parse_primary_heic_item_preflight_properties, parse_primary_heic_item_properties,
        parse_primary_item_transform_properties, BoxIter, ColorInformation,
        ExtractAvifItemDataError, ExtractHeicItemDataError, FourCc, HeicPrimaryItemDataWithGrid,
        IccColorProfile, ImageCleanApertureProperty, ImageMirrorDirection, ImageMirrorProperty,
        ImageRotationProperty, ItemLocationField, NclxColorProfile,
        ParseAv1CodecConfigurationBoxError, ParseBoxError, ParseColorInformationPropertyError,
        ParseFileTypeBoxError, ParseFullBoxError, ParseHevcDecoderConfigurationBoxError,
        ParseImageCleanAperturePropertyError, ParseImageMirrorPropertyError,
        ParseImageRotationPropertyError, ParseImageSpatialExtentsPropertyError,
        ParseItemInfoBoxError, ParseItemInfoEntryBoxError, ParseItemLocationBoxError,
        ParseItemPropertiesBoxError, ParseItemPropertyAssociationBoxError,
        ParseItemPropertyContainerBoxError, ParseItemReferenceBoxError, ParseMetaBoxError,
        ParsePixelInformationPropertyError, ParsePrimaryAvifPropertiesError,
        ParsePrimaryHeicPropertiesError, ParsePrimaryItemBoxError, PrimaryItemColorProperties,
        PrimaryItemTransformProperty, ResolvePrimaryItemGraphError, BASIC_HEADER_SIZE,
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

    #[test]
    fn parses_iref_version_zero_typed_references() {
        let mut dimg_payload = Vec::new();
        dimg_payload.extend_from_slice(&0x0002_u16.to_be_bytes()); // from_item_ID
        dimg_payload.extend_from_slice(&2_u16.to_be_bytes()); // reference_count
        dimg_payload.extend_from_slice(&0x0010_u16.to_be_bytes()); // to_item_ID[0]
        dimg_payload.extend_from_slice(&0x0011_u16.to_be_bytes()); // to_item_ID[1]
        let dimg = make_basic_box(*b"dimg", &dimg_payload);

        let mut thmb_payload = Vec::new();
        thmb_payload.extend_from_slice(&0x0002_u16.to_be_bytes()); // from_item_ID
        thmb_payload.extend_from_slice(&1_u16.to_be_bytes()); // reference_count
        thmb_payload.extend_from_slice(&0x0005_u16.to_be_bytes()); // to_item_ID[0]
        let thmb = make_basic_box(*b"thmb", &thmb_payload);

        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iref_payload.extend_from_slice(&dimg);
        iref_payload.extend_from_slice(&thmb);
        let bytes = make_basic_box(*b"iref", &iref_payload);
        let top_level = parse_boxes(&bytes).expect("iref box should parse");

        let iref = top_level[0]
            .parse_iref()
            .expect("iref v0 payload should parse");
        assert_eq!(iref.full_box.version, 0);
        assert_eq!(iref.references.len(), 2);
        assert_eq!(iref.references[0].reference_type, FourCc::new(*b"dimg"));
        assert_eq!(iref.references[0].from_item_id, 2);
        assert_eq!(iref.references[0].to_item_ids, vec![16, 17]);
        assert_eq!(iref.references[1].reference_type, FourCc::new(*b"thmb"));
        assert_eq!(iref.references[1].from_item_id, 2);
        assert_eq!(iref.references[1].to_item_ids, vec![5]);
    }

    #[test]
    fn parses_iref_version_one_with_u32_item_ids() {
        let mut auxl_payload = Vec::new();
        auxl_payload.extend_from_slice(&0x1122_3344_u32.to_be_bytes()); // from_item_ID
        auxl_payload.extend_from_slice(&1_u16.to_be_bytes()); // reference_count
        auxl_payload.extend_from_slice(&0x5566_7788_u32.to_be_bytes()); // to_item_ID[0]
        let auxl = make_basic_box(*b"auxl", &auxl_payload);

        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        iref_payload.extend_from_slice(&auxl);
        let bytes = make_basic_box(*b"iref", &iref_payload);
        let top_level = parse_boxes(&bytes).expect("iref box should parse");

        let iref = top_level[0]
            .parse_iref()
            .expect("iref v1 payload should parse");
        assert_eq!(iref.full_box.version, 1);
        assert_eq!(iref.references.len(), 1);
        assert_eq!(iref.references[0].reference_type, FourCc::new(*b"auxl"));
        assert_eq!(iref.references[0].from_item_id, 0x1122_3344);
        assert_eq!(iref.references[0].to_item_ids, vec![0x5566_7788]);
    }

    #[test]
    fn rejects_iref_parse_for_non_iref_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_iref()
            .expect_err("parsing non-iref as iref must fail");
        assert_eq!(
            err,
            ParseItemReferenceBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_iref_parse_for_unsupported_version() {
        let bytes = make_basic_box(*b"iref", &[0x02, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("iref box should parse");

        let err = top_level[0]
            .parse_iref()
            .expect_err("unsupported iref version must fail");
        assert_eq!(
            err,
            ParseItemReferenceBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 2,
            }
        );
    }

    #[test]
    fn rejects_iref_parse_when_reference_target_id_is_truncated() {
        let mut dimg_payload = Vec::new();
        dimg_payload.extend_from_slice(&0x0002_u16.to_be_bytes()); // from_item_ID
        dimg_payload.extend_from_slice(&1_u16.to_be_bytes()); // reference_count
        let dimg = make_basic_box(*b"dimg", &dimg_payload);

        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iref_payload.extend_from_slice(&dimg);
        let bytes = make_basic_box(*b"iref", &iref_payload);
        let top_level = parse_boxes(&bytes).expect("iref box should parse");

        let err = top_level[0]
            .parse_iref()
            .expect_err("truncated to_item_ID must fail");
        assert_eq!(
            err,
            ParseItemReferenceBoxError::PayloadTooSmall {
                offset: 24,
                context: "to_item_ID",
                available: 0,
                required: 2,
            }
        );
    }

    #[test]
    fn rejects_iref_parse_when_reference_count_is_zero() {
        let mut dimg_payload = Vec::new();
        dimg_payload.extend_from_slice(&0x0002_u16.to_be_bytes()); // from_item_ID
        dimg_payload.extend_from_slice(&0_u16.to_be_bytes()); // reference_count (invalid)
        let dimg = make_basic_box(*b"dimg", &dimg_payload);

        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iref_payload.extend_from_slice(&dimg);
        let bytes = make_basic_box(*b"iref", &iref_payload);
        let top_level = parse_boxes(&bytes).expect("iref box should parse");

        let err = top_level[0]
            .parse_iref()
            .expect_err("zero reference_count must fail");
        assert_eq!(
            err,
            ParseItemReferenceBoxError::EmptyReferenceList {
                offset: 22,
                reference_type: FourCc::new(*b"dimg"),
            }
        );
    }

    #[test]
    fn parses_ipco_child_property_boxes() {
        let prop_a = make_basic_box(*b"ispe", &[0x01, 0x02, 0x03, 0x04]);
        let prop_b = make_basic_box(*b"pixi", &[0x08, 0x08, 0x08]);
        let mut payload = Vec::new();
        payload.extend_from_slice(&prop_a);
        payload.extend_from_slice(&prop_b);
        let bytes = make_basic_box(*b"ipco", &payload);
        let top_level = parse_boxes(&bytes).expect("ipco box should parse");

        let ipco = top_level[0]
            .parse_ipco()
            .expect("ipco payload should parse");
        assert_eq!(ipco.properties.len(), 2);
        assert_eq!(ipco.properties[0].header.box_type.as_bytes(), *b"ispe");
        assert_eq!(ipco.properties[1].header.box_type.as_bytes(), *b"pixi");
        assert_eq!(ipco.properties[0].offset, BASIC_HEADER_SIZE as u64);
        assert_eq!(
            ipco.properties[1].offset,
            BASIC_HEADER_SIZE as u64 + prop_a.len() as u64
        );
    }

    #[test]
    fn rejects_ipco_parse_for_non_ipco_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_ipco()
            .expect_err("parsing non-ipco as ipco must fail");
        assert_eq!(
            err,
            ParseItemPropertyContainerBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn parses_ipma_version_zero_with_compact_associations() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        payload.extend_from_slice(&0x1234_u16.to_be_bytes()); // item_ID
        payload.push(2); // association_count
        payload.push(0x81); // essential=true, property_index=1
        payload.push(0x05); // essential=false, property_index=5
        let bytes = make_basic_box(*b"ipma", &payload);
        let top_level = parse_boxes(&bytes).expect("ipma box should parse");

        let ipma = top_level[0]
            .parse_ipma()
            .expect("ipma v0 payload should parse");
        assert_eq!(ipma.full_box.version, 0);
        assert_eq!(ipma.full_box.flags, 0);
        assert_eq!(ipma.entry_count, 1);
        assert_eq!(ipma.entries.len(), 1);
        assert_eq!(ipma.entries[0].item_id, 0x1234);
        assert_eq!(ipma.entries[0].associations.len(), 2);
        assert_eq!(ipma.entries[0].associations[0].property_index, 1);
        assert!(ipma.entries[0].associations[0].essential);
        assert_eq!(ipma.entries[0].associations[1].property_index, 5);
        assert!(!ipma.entries[0].associations[1].essential);
    }

    #[test]
    fn parses_ipma_version_one_with_extended_associations() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x01]); // version=1, flags=1
        payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        payload.extend_from_slice(&0x1122_3344_u32.to_be_bytes()); // item_ID
        payload.push(2); // association_count
        payload.extend_from_slice(&0x8002_u16.to_be_bytes()); // essential=true, property_index=2
        payload.extend_from_slice(&0x00ff_u16.to_be_bytes()); // essential=false, property_index=255
        let bytes = make_basic_box(*b"ipma", &payload);
        let top_level = parse_boxes(&bytes).expect("ipma box should parse");

        let ipma = top_level[0]
            .parse_ipma()
            .expect("ipma v1 payload should parse");
        assert_eq!(ipma.full_box.version, 1);
        assert_eq!(ipma.full_box.flags, 1);
        assert_eq!(ipma.entry_count, 1);
        assert_eq!(ipma.entries.len(), 1);
        assert_eq!(ipma.entries[0].item_id, 0x1122_3344);
        assert_eq!(ipma.entries[0].associations.len(), 2);
        assert_eq!(ipma.entries[0].associations[0].property_index, 2);
        assert!(ipma.entries[0].associations[0].essential);
        assert_eq!(ipma.entries[0].associations[1].property_index, 255);
        assert!(!ipma.entries[0].associations[1].essential);
    }

    #[test]
    fn rejects_ipma_parse_for_non_ipma_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_ipma()
            .expect_err("parsing non-ipma as ipma must fail");
        assert_eq!(
            err,
            ParseItemPropertyAssociationBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_ipma_parse_for_unsupported_version() {
        let bytes = make_basic_box(*b"ipma", &[0x02, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("ipma box should parse");

        let err = top_level[0]
            .parse_ipma()
            .expect_err("unsupported ipma version must fail");
        assert_eq!(
            err,
            ParseItemPropertyAssociationBoxError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 2,
            }
        );
    }

    #[test]
    fn rejects_ipma_parse_when_entry_count_field_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        payload.extend_from_slice(&[0xaa, 0xbb]); // truncated entry_count
        let bytes = make_basic_box(*b"ipma", &payload);
        let top_level = parse_boxes(&bytes).expect("ipma box should parse");

        let err = top_level[0]
            .parse_ipma()
            .expect_err("truncated ipma entry_count must fail");
        assert_eq!(
            err,
            ParseItemPropertyAssociationBoxError::PayloadTooSmall {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                context: "entry_count",
                available: 2,
                required: 4,
            }
        );
    }

    #[test]
    fn rejects_ipma_parse_when_association_field_is_truncated() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x01]); // version=1, flags=1
        payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        payload.extend_from_slice(&0x1122_3344_u32.to_be_bytes()); // item_ID
        payload.push(1); // association_count
        payload.push(0x80); // truncated 16-bit association
        let bytes = make_basic_box(*b"ipma", &payload);
        let top_level = parse_boxes(&bytes).expect("ipma box should parse");

        let err = top_level[0]
            .parse_ipma()
            .expect_err("truncated ipma association must fail");
        assert_eq!(
            err,
            ParseItemPropertyAssociationBoxError::PayloadTooSmall {
                offset: 21,
                context: "association",
                available: 1,
                required: 2,
            }
        );
    }

    #[test]
    fn parses_iprp_with_ipco_and_ipma_children() {
        let property = make_basic_box(*b"ispe", &[0x00, 0x00, 0x00, 0x00]);
        let ipco = make_basic_box(*b"ipco", &property);

        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        ipma_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // item_ID
        ipma_payload.push(1); // association_count
        ipma_payload.push(0x01); // property_index=1
        let ipma = make_basic_box(*b"ipma", &ipma_payload);

        let unknown = make_basic_box(*b"free", &[0x01, 0x02, 0x03, 0x04]);

        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&unknown);
        iprp_payload.extend_from_slice(&ipma);
        let bytes = make_basic_box(*b"iprp", &iprp_payload);
        let top_level = parse_boxes(&bytes).expect("iprp box should parse");

        let iprp = top_level[0]
            .parse_iprp()
            .expect("iprp payload should parse");
        assert_eq!(iprp.property_containers.len(), 1);
        assert_eq!(iprp.associations.len(), 1);
        assert_eq!(iprp.property_containers[0].properties.len(), 1);
        assert_eq!(
            iprp.property_containers[0].properties[0]
                .header
                .box_type
                .as_bytes(),
            *b"ispe"
        );
        assert_eq!(iprp.associations[0].entries.len(), 1);
        assert_eq!(iprp.associations[0].entries[0].item_id, 1);
        assert_eq!(
            iprp.associations[0].entries[0].associations[0].property_index,
            1
        );
    }

    #[test]
    fn rejects_iprp_parse_for_non_iprp_box() {
        let bytes = make_basic_box(*b"free", &[0x00, 0x00, 0x00, 0x00]);
        let top_level = parse_boxes(&bytes).expect("free box should parse");

        let err = top_level[0]
            .parse_iprp()
            .expect_err("parsing non-iprp as iprp must fail");
        assert_eq!(
            err,
            ParseItemPropertiesBoxError::UnexpectedBoxType {
                offset: 0,
                actual: FourCc::new(*b"free"),
            }
        );
    }

    #[test]
    fn rejects_iprp_parse_when_child_box_is_out_of_bounds() {
        let mut invalid_child = Vec::new();
        invalid_child.extend_from_slice(&16_u32.to_be_bytes());
        invalid_child.extend_from_slice(b"ipco");
        invalid_child.extend_from_slice(&[0xaa, 0xbb, 0xcc, 0xdd]);
        let bytes = make_basic_box(*b"iprp", &invalid_child);
        let top_level = parse_boxes(&bytes).expect("iprp box should parse");

        let err = top_level[0]
            .parse_iprp()
            .expect_err("invalid iprp child bounds must fail");
        assert_eq!(
            err,
            ParseItemPropertiesBoxError::ChildBox(ParseBoxError::BoxOutOfBounds {
                offset: BASIC_HEADER_SIZE as u64,
                box_size: 16,
                available: 12,
            })
        );
    }

    #[test]
    fn resolves_meta_primary_item_graph() {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"av01"); // item_type
        infe_payload.extend_from_slice(b"primary\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let iinf = make_basic_box(*b"iinf", &iinf_payload);

        let ispe = make_basic_box(*b"ispe", &[0x00, 0x00, 0x00, 0x00]);
        let pixi = make_basic_box(*b"pixi", &[0x08, 0x08, 0x08]);
        let mut ipco_payload = Vec::new();
        ipco_payload.extend_from_slice(&ispe);
        ipco_payload.extend_from_slice(&pixi);
        let ipco = make_basic_box(*b"ipco", &ipco_payload);

        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        ipma_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        ipma_payload.push(2); // association_count
        ipma_payload.push(0x81); // essential=true, property_index=1
        ipma_payload.push(0x02); // essential=false, property_index=2
        let ipma = make_basic_box(*b"ipma", &ipma_payload);

        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        let iprp = make_basic_box(*b"iprp", &iprp_payload);

        let mut dimg_payload = Vec::new();
        dimg_payload.extend_from_slice(&1_u16.to_be_bytes()); // from_item_ID
        dimg_payload.extend_from_slice(&1_u16.to_be_bytes()); // reference_count
        dimg_payload.extend_from_slice(&2_u16.to_be_bytes()); // to_item_ID[0]
        let dimg = make_basic_box(*b"dimg", &dimg_payload);

        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iref_payload.extend_from_slice(&dimg);
        let iref = make_basic_box(*b"iref", &iref_payload);

        let meta = make_meta_box(&[pitm, iloc, iinf, iprp, iref]);
        let top_level = parse_boxes(&meta).expect("meta box should parse");

        let parsed_meta = top_level[0].parse_meta().expect("meta should parse");
        let resolved = parsed_meta
            .resolve_primary_item()
            .expect("meta primary resolver should succeed");
        assert_eq!(resolved.pitm.item_id, 1);
        assert_eq!(resolved.primary_item.item_id, 1);
        assert_eq!(
            resolved.primary_item.item_info.item_type,
            Some(FourCc::new(*b"av01"))
        );
        assert_eq!(resolved.primary_item.location.item_id, 1);
        assert_eq!(resolved.primary_item.properties.len(), 2);
        assert!(resolved.primary_item.properties[0].essential);
        assert_eq!(resolved.primary_item.properties[0].property_index, 1);
        assert_eq!(
            resolved.primary_item.properties[0]
                .property
                .header
                .box_type
                .as_bytes(),
            *b"ispe"
        );
        assert!(!resolved.primary_item.properties[1].essential);
        assert_eq!(resolved.primary_item.properties[1].property_index, 2);
        assert_eq!(
            resolved.primary_item.properties[1]
                .property
                .header
                .box_type
                .as_bytes(),
            *b"pixi"
        );
        assert_eq!(resolved.primary_item.references.len(), 1);
        assert_eq!(
            resolved.primary_item.references[0].reference_type,
            FourCc::new(*b"dimg")
        );
        assert_eq!(resolved.primary_item.references[0].to_item_ids, vec![2]);
    }

    #[test]
    fn rejects_meta_primary_resolution_when_required_box_is_missing() {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let meta = make_meta_box(&[pitm]);
        let top_level = parse_boxes(&meta).expect("meta box should parse");
        let parsed_meta = top_level[0].parse_meta().expect("meta should parse");

        let err = parsed_meta
            .resolve_primary_item()
            .expect_err("missing iloc must fail");
        assert_eq!(
            err,
            ResolvePrimaryItemGraphError::MissingRequiredBox {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE) as u64,
                box_type: FourCc::new(*b"iloc"),
            }
        );
    }

    #[test]
    fn rejects_meta_primary_resolution_when_pitm_item_is_missing_from_iinf() {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x02]);

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&2_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID (different from pitm)
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"av01"); // item_type
        infe_payload.extend_from_slice(b"other\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let iinf = make_basic_box(*b"iinf", &iinf_payload);

        let property = make_basic_box(*b"ispe", &[0x00, 0x00, 0x00, 0x00]);
        let ipco = make_basic_box(*b"ipco", &property);
        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        ipma_payload.extend_from_slice(&2_u16.to_be_bytes()); // item_ID
        ipma_payload.push(1); // association_count
        ipma_payload.push(0x01); // property_index=1
        let ipma = make_basic_box(*b"ipma", &ipma_payload);
        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        let iprp = make_basic_box(*b"iprp", &iprp_payload);

        let meta = make_meta_box(&[pitm, iloc, iinf, iprp]);
        let top_level = parse_boxes(&meta).expect("meta box should parse");
        let parsed_meta = top_level[0].parse_meta().expect("meta should parse");

        let err = parsed_meta
            .resolve_primary_item()
            .expect_err("pitm item_ID missing from iinf must fail");
        assert!(matches!(
            err,
            ResolvePrimaryItemGraphError::PrimaryItemMissingFromItemInfo { item_id: 2, .. }
        ));
    }

    #[test]
    fn rejects_meta_primary_resolution_when_property_index_is_out_of_range() {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(b"av01"); // item_type
        infe_payload.extend_from_slice(b"primary\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let iinf = make_basic_box(*b"iinf", &iinf_payload);

        let property = make_basic_box(*b"ispe", &[0x00, 0x00, 0x00, 0x00]);
        let ipco = make_basic_box(*b"ipco", &property);
        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        ipma_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        ipma_payload.push(1); // association_count
        ipma_payload.push(0x02); // property_index=2 (out of range)
        let ipma = make_basic_box(*b"ipma", &ipma_payload);
        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        let iprp = make_basic_box(*b"iprp", &iprp_payload);

        let meta = make_meta_box(&[pitm, iloc, iinf, iprp]);
        let top_level = parse_boxes(&meta).expect("meta box should parse");
        let parsed_meta = top_level[0].parse_meta().expect("meta should parse");

        let err = parsed_meta
            .resolve_primary_item()
            .expect_err("invalid property index must fail");
        assert!(matches!(
            err,
            ResolvePrimaryItemGraphError::PropertyIndexOutOfRange {
                item_id: 1,
                property_index: 2,
                available: 1,
                ..
            }
        ));
    }

    #[test]
    fn extracts_primary_avif_payload_from_mdat_backed_iloc_extents() {
        let mdat = make_basic_box(*b"mdat", &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&(BASIC_HEADER_SIZE as u32).to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&2_u16.to_be_bytes()); // extent_count
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // extent_offset[0]
        iloc_payload.extend_from_slice(&2_u32.to_be_bytes()); // extent_length[0]
        iloc_payload.extend_from_slice(&4_u32.to_be_bytes()); // extent_offset[1]
        iloc_payload.extend_from_slice(&2_u32.to_be_bytes()); // extent_length[1]
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_avif_meta(iloc, &[]);
        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let extracted =
            extract_primary_avif_item_data(&file).expect("AVIF mdat-backed payload must extract");
        assert_eq!(extracted.item_id, 1);
        assert_eq!(extracted.construction_method, 0);
        assert_eq!(extracted.payload, vec![0xaa, 0xbb, 0xee, 0xff]);
    }

    #[test]
    fn extracts_primary_avif_payload_from_idat_backed_iloc_extents() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4, index=0
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // construction_method=1
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
        iloc_payload.extend_from_slice(&1_u32.to_be_bytes()); // extent_offset
        iloc_payload.extend_from_slice(&3_u32.to_be_bytes()); // extent_length
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let idat = make_basic_box(*b"idat", &[0x10, 0x11, 0x12, 0x13, 0x14]);
        let meta = make_primary_avif_meta(iloc, &[idat]);

        let extracted =
            extract_primary_avif_item_data(&meta).expect("AVIF idat-backed payload must extract");
        assert_eq!(extracted.item_id, 1);
        assert_eq!(extracted.construction_method, 1);
        assert_eq!(extracted.payload, vec![0x11, 0x12, 0x13]);
    }

    #[test]
    fn rejects_primary_avif_extraction_when_idat_is_missing() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4, index=0
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // construction_method=1
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_avif_meta(iloc, &[]);
        let err = extract_primary_avif_item_data(&meta).expect_err("missing idat must fail");
        assert_eq!(err, ExtractAvifItemDataError::MissingIdatBox { item_id: 1 });
    }

    #[test]
    fn rejects_primary_avif_extraction_for_external_data_reference_index() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // size fields all zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_avif_meta(iloc, &[]);
        let err =
            extract_primary_avif_item_data(&meta).expect_err("external data reference must fail");
        assert_eq!(
            err,
            ExtractAvifItemDataError::UnsupportedDataReferenceIndex {
                item_id: 1,
                data_reference_index: 1,
            }
        );
    }

    #[test]
    fn extracts_primary_heic_payload_from_mdat_backed_iloc_extents() {
        let mdat = make_basic_box(*b"mdat", &[0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff]);

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&(BASIC_HEADER_SIZE as u32).to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&2_u16.to_be_bytes()); // extent_count
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // extent_offset[0]
        iloc_payload.extend_from_slice(&2_u32.to_be_bytes()); // extent_length[0]
        iloc_payload.extend_from_slice(&4_u32.to_be_bytes()); // extent_offset[1]
        iloc_payload.extend_from_slice(&2_u32.to_be_bytes()); // extent_length[1]
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_heic_meta(iloc, &[]);
        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let extracted =
            extract_primary_heic_item_data(&file).expect("HEIC mdat-backed payload must extract");
        assert_eq!(extracted.item_id, 1);
        assert_eq!(extracted.construction_method, 0);
        assert_eq!(extracted.payload, vec![0xaa, 0xbb, 0xee, 0xff]);
    }

    #[test]
    fn extracts_primary_heic_payload_from_idat_backed_iloc_extents() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x01, 0x00, 0x00, 0x00]); // version=1, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4, index=0
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0x0001_u16.to_be_bytes()); // construction_method=1
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
        iloc_payload.extend_from_slice(&1_u32.to_be_bytes()); // extent_offset
        iloc_payload.extend_from_slice(&3_u32.to_be_bytes()); // extent_length
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let idat = make_basic_box(*b"idat", &[0x10, 0x11, 0x12, 0x13, 0x14]);
        let meta = make_primary_heic_meta(iloc, &[idat]);

        let extracted =
            extract_primary_heic_item_data(&meta).expect("HEIC idat-backed payload must extract");
        assert_eq!(extracted.item_id, 1);
        assert_eq!(extracted.construction_method, 1);
        assert_eq!(extracted.payload, vec![0x11, 0x12, 0x13]);
    }

    #[test]
    fn accepts_primary_heic_extraction_for_hev1_item_type() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&(BASIC_HEADER_SIZE as u32).to_be_bytes()); // base_offset
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
        iloc_payload.extend_from_slice(&0_u32.to_be_bytes()); // extent_offset
        iloc_payload.extend_from_slice(&2_u32.to_be_bytes()); // extent_length
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let mdat = make_basic_box(*b"mdat", &[0xaa, 0xbb, 0xcc, 0xdd]);
        let meta = make_primary_heic_meta_with_item_type(*b"hev1", iloc, &[]);
        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let extracted =
            extract_primary_heic_item_data(&file).expect("HEIC hev1 payload should extract");
        assert_eq!(extracted.payload, vec![0xaa, 0xbb]);
    }

    #[test]
    fn rejects_primary_heic_extraction_for_unexpected_primary_item_type() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // size fields all zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_avif_meta(iloc, &[]);
        let err = extract_primary_heic_item_data(&meta)
            .expect_err("non-HEIC primary item type must fail");
        assert_eq!(
            err,
            ExtractHeicItemDataError::UnexpectedPrimaryItemType {
                item_id: 1,
                actual: FourCc::new(*b"av01"),
            }
        );
    }

    #[test]
    fn extracts_primary_heic_grid_descriptor_and_tile_payloads() {
        let grid_descriptor = vec![0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02];
        let tile_a = vec![0xaa, 0xbb];
        let tile_b = vec![0xcc, 0xdd, 0xee];
        let mut mdat_payload = Vec::new();
        mdat_payload.extend_from_slice(&grid_descriptor);
        mdat_payload.extend_from_slice(&tile_a);
        mdat_payload.extend_from_slice(&tile_b);
        let mdat = make_basic_box(*b"mdat", &mdat_payload);

        let base_offset = BASIC_HEADER_SIZE as u32;
        let iloc = make_iloc_v0_single_extent_items(&[
            (1_u16, base_offset, 0_u32, grid_descriptor.len() as u32),
            (
                2_u16,
                base_offset,
                grid_descriptor.len() as u32,
                tile_a.len() as u32,
            ),
            (
                3_u16,
                base_offset,
                (grid_descriptor.len() + tile_a.len()) as u32,
                tile_b.len() as u32,
            ),
        ]);
        let iinf = make_iinf_with_entries(&[
            (1_u16, *b"grid", b"primary-grid"),
            (2_u16, *b"hvc1", b"tile-0"),
            (3_u16, *b"hev1", b"tile-1"),
        ]);
        let iref = make_iref_dimg_v0(1_u16, &[2_u16, 3_u16]);

        let hvcc_property = make_hvcc_property();
        let expected_hvcc = parse_boxes(&hvcc_property).expect("hvcC property should parse")[0]
            .parse_hvcc()
            .expect("hvcC property payload should parse");
        let ipco = make_basic_box(*b"ipco", &hvcc_property);
        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&2_u32.to_be_bytes()); // entry_count
        for tile_item_id in [2_u16, 3_u16] {
            ipma_payload.extend_from_slice(&tile_item_id.to_be_bytes());
            ipma_payload.push(1); // association_count
            ipma_payload.push(0x01); // property_index=1
        }
        let ipma = make_basic_box(*b"ipma", &ipma_payload);
        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        let iprp = make_basic_box(*b"iprp", &iprp_payload);
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let meta = make_meta_box(&[pitm, iloc, iinf, iprp, iref]);

        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let extracted = extract_primary_heic_item_data_with_grid(&file)
            .expect("grid primary extraction should succeed");
        assert_eq!(
            extracted,
            HeicPrimaryItemDataWithGrid::Grid(super::HeicGridPrimaryItemData {
                item_id: 1,
                construction_method: 0,
                descriptor: super::HeicGridDescriptor {
                    version: 0,
                    rows: 1,
                    columns: 2,
                    output_width: 4,
                    output_height: 2,
                },
                tile_item_ids: vec![2, 3],
                tiles: vec![
                    super::HeicGridTileItemData {
                        item_id: 2,
                        construction_method: 0,
                        hvcc: expected_hvcc.clone(),
                        payload: tile_a,
                    },
                    super::HeicGridTileItemData {
                        item_id: 3,
                        construction_method: 0,
                        hvcc: expected_hvcc,
                        payload: tile_b,
                    },
                ],
            })
        );
    }

    #[test]
    fn rejects_primary_heic_grid_extraction_when_dimg_references_are_missing() {
        let grid_descriptor = vec![0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0x00, 0x02];
        let mdat = make_basic_box(*b"mdat", &grid_descriptor);
        let base_offset = BASIC_HEADER_SIZE as u32;
        let iloc = make_iloc_v0_single_extent_items(&[(
            1_u16,
            base_offset,
            0_u32,
            grid_descriptor.len() as u32,
        )]);
        let iinf = make_iinf_with_entries(&[(1_u16, *b"grid", b"primary-grid")]);
        let meta = make_primary_grid_heic_meta(iloc, iinf, None, &[]);

        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let err = extract_primary_heic_item_data_with_grid(&file)
            .expect_err("missing dimg references must fail");
        assert_eq!(
            err,
            ExtractHeicItemDataError::MissingGridTileReferences { item_id: 1 }
        );
    }

    #[test]
    fn rejects_primary_heic_grid_extraction_when_tile_count_mismatches_descriptor() {
        let grid_descriptor = vec![0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02];
        let tile_a = vec![0xaa, 0xbb];
        let mut mdat_payload = Vec::new();
        mdat_payload.extend_from_slice(&grid_descriptor);
        mdat_payload.extend_from_slice(&tile_a);
        let mdat = make_basic_box(*b"mdat", &mdat_payload);

        let base_offset = BASIC_HEADER_SIZE as u32;
        let iloc = make_iloc_v0_single_extent_items(&[
            (1_u16, base_offset, 0_u32, grid_descriptor.len() as u32),
            (
                2_u16,
                base_offset,
                grid_descriptor.len() as u32,
                tile_a.len() as u32,
            ),
        ]);
        let iinf = make_iinf_with_entries(&[
            (1_u16, *b"grid", b"primary-grid"),
            (2_u16, *b"hvc1", b"tile-0"),
        ]);
        let iref = make_iref_dimg_v0(1_u16, &[2_u16]);
        let meta = make_primary_grid_heic_meta(iloc, iinf, Some(iref), &[]);

        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let err = extract_primary_heic_item_data_with_grid(&file)
            .expect_err("grid tile count mismatch must fail");
        assert_eq!(
            err,
            ExtractHeicItemDataError::GridTileCountMismatch {
                item_id: 1,
                rows: 1,
                columns: 2,
                expected: 2,
                actual: 1,
            }
        );
    }

    #[test]
    fn rejects_primary_heic_grid_extraction_when_tile_hvcc_property_is_missing() {
        let grid_descriptor = vec![0x00, 0x00, 0x00, 0x01, 0x00, 0x04, 0x00, 0x02];
        let tile_a = vec![0xaa, 0xbb];
        let tile_b = vec![0xcc, 0xdd, 0xee];
        let mut mdat_payload = Vec::new();
        mdat_payload.extend_from_slice(&grid_descriptor);
        mdat_payload.extend_from_slice(&tile_a);
        mdat_payload.extend_from_slice(&tile_b);
        let mdat = make_basic_box(*b"mdat", &mdat_payload);

        let base_offset = BASIC_HEADER_SIZE as u32;
        let iloc = make_iloc_v0_single_extent_items(&[
            (1_u16, base_offset, 0_u32, grid_descriptor.len() as u32),
            (
                2_u16,
                base_offset,
                grid_descriptor.len() as u32,
                tile_a.len() as u32,
            ),
            (
                3_u16,
                base_offset,
                (grid_descriptor.len() + tile_a.len()) as u32,
                tile_b.len() as u32,
            ),
        ]);
        let iinf = make_iinf_with_entries(&[
            (1_u16, *b"grid", b"primary-grid"),
            (2_u16, *b"hvc1", b"tile-0"),
            (3_u16, *b"hev1", b"tile-1"),
        ]);
        let iref = make_iref_dimg_v0(1_u16, &[2_u16, 3_u16]);
        let meta = make_primary_grid_heic_meta(iloc, iinf, Some(iref), &[]);

        let mut file = Vec::new();
        file.extend_from_slice(&mdat);
        file.extend_from_slice(&meta);

        let err = extract_primary_heic_item_data_with_grid(&file)
            .expect_err("grid tile without hvcC must fail");
        assert_eq!(
            err,
            ExtractHeicItemDataError::MissingGridTileCodecConfiguration {
                item_id: 1,
                tile_item_id: 2,
            }
        );
    }

    #[test]
    fn parses_primary_avif_properties_from_item_graph() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_avif_meta(iloc, &[]);
        let properties = parse_primary_avif_item_properties(&meta)
            .expect("primary AVIF property parse should succeed");

        assert_eq!(properties.item_id, 1);
        assert!(properties.av1c.marker);
        assert_eq!(properties.av1c.version, 1);
        assert_eq!(properties.av1c.seq_profile, 2);
        assert_eq!(properties.av1c.seq_level_idx_0, 21);
        assert!(properties.av1c.seq_tier_0);
        assert!(properties.av1c.high_bitdepth);
        assert!(!properties.av1c.twelve_bit);
        assert!(properties.av1c.monochrome);
        assert!(properties.av1c.chroma_subsampling_x);
        assert!(!properties.av1c.chroma_subsampling_y);
        assert_eq!(properties.av1c.chroma_sample_position, 2);
        assert_eq!(
            properties.av1c.initial_presentation_delay_minus_one,
            Some(5)
        );
        assert_eq!(properties.av1c.config_obus, vec![0xaa, 0xbb]);

        assert_eq!(properties.ispe.width, 640);
        assert_eq!(properties.ispe.height, 480);
        assert_eq!(properties.pixi.bits_per_channel, vec![8, 8, 8]);
        assert_eq!(properties.colr, PrimaryItemColorProperties::default());
    }

    #[test]
    fn rejects_primary_avif_property_parse_when_required_property_is_missing() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![make_av1c_property(), make_ispe_property(640, 480)];
        let meta = make_primary_avif_meta_with_properties(iloc, &properties, &[]);
        let err =
            parse_primary_avif_item_properties(&meta).expect_err("missing pixi property must fail");
        assert_eq!(
            err,
            ParsePrimaryAvifPropertiesError::MissingRequiredProperty {
                item_id: 1,
                property_type: FourCc::new(*b"pixi"),
            }
        );
    }

    #[test]
    fn rejects_primary_avif_property_parse_when_property_is_duplicated() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![
            make_av1c_property(),
            make_av1c_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
        ];
        let meta = make_primary_avif_meta_with_properties(iloc, &properties, &[]);
        let err = parse_primary_avif_item_properties(&meta)
            .expect_err("duplicate av1C property must fail");
        assert_eq!(
            err,
            ParsePrimaryAvifPropertiesError::DuplicateProperty {
                item_id: 1,
                property_type: FourCc::new(*b"av1C"),
            }
        );
    }

    #[test]
    fn parses_primary_heic_properties_from_item_graph() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let meta = make_primary_heic_meta(iloc, &[]);
        let properties = parse_primary_heic_item_properties(&meta)
            .expect("primary HEIC property parse should succeed");

        assert_eq!(properties.item_id, 1);
        assert_eq!(properties.hvcc.configuration_version, 1);
        assert_eq!(properties.hvcc.general_profile_space, 0);
        assert!(!properties.hvcc.general_tier_flag);
        assert_eq!(properties.hvcc.general_profile_idc, 1);
        assert_eq!(properties.hvcc.bit_depth_luma, 8);
        assert_eq!(properties.hvcc.bit_depth_chroma, 8);
        assert_eq!(properties.hvcc.nal_length_size, 4);
        assert_eq!(properties.hvcc.nal_arrays.len(), 1);
        assert_eq!(properties.hvcc.nal_arrays[0].nal_unit_type, 33);
        assert_eq!(
            properties.hvcc.nal_arrays[0].nal_units,
            vec![vec![0x42, 0x01]]
        );
        assert_eq!(properties.ispe.width, 640);
        assert_eq!(properties.ispe.height, 480);
        assert_eq!(properties.pixi.bits_per_channel, vec![8, 8, 8]);
        assert_eq!(properties.colr, PrimaryItemColorProperties::default());
    }

    #[test]
    fn parses_primary_heic_preflight_properties_without_pixi() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![make_hvcc_property(), make_ispe_property(640, 480)];
        let meta = make_primary_item_meta_with_properties(*b"hvc1", iloc, &properties, &[]);
        let preflight = parse_primary_heic_item_preflight_properties(&meta)
            .expect("missing pixi should still parse for HEIC decoder preflight");

        assert_eq!(preflight.item_id, 1);
        assert_eq!(preflight.hvcc.nal_length_size, 4);
        assert_eq!(preflight.ispe.width, 640);
        assert_eq!(preflight.ispe.height, 480);
        assert!(preflight.pixi.is_none());
        assert_eq!(preflight.colr, PrimaryItemColorProperties::default());
    }

    #[test]
    fn parses_primary_item_colr_profiles_for_avif_and_heic() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let nclx = make_colr_nclx_property(1, 13, 6, false);
        let ricc = make_colr_icc_property(*b"rICC", &[0x10, 0x20, 0x30]);

        let avif_properties = vec![
            make_av1c_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
            nclx.clone(),
            ricc.clone(),
        ];
        let avif_meta = make_primary_avif_meta_with_properties(iloc.clone(), &avif_properties, &[]);
        let avif = parse_primary_avif_item_properties(&avif_meta)
            .expect("AVIF primary property parse should include colr profiles");
        assert_eq!(
            avif.colr.nclx,
            Some(NclxColorProfile {
                colour_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 6,
                full_range_flag: false,
            })
        );
        assert_eq!(
            avif.colr.icc,
            Some(IccColorProfile {
                profile_type: FourCc::new(*b"rICC"),
                profile: vec![0x10, 0x20, 0x30],
            })
        );

        let heic_properties = vec![
            make_hvcc_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
            nclx,
            ricc,
        ];
        let heic_meta =
            make_primary_item_meta_with_properties(*b"hvc1", iloc, &heic_properties, &[]);
        let heic = parse_primary_heic_item_preflight_properties(&heic_meta)
            .expect("HEIC primary preflight parse should include colr profiles");
        assert_eq!(
            heic.colr.nclx,
            Some(NclxColorProfile {
                colour_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 6,
                full_range_flag: false,
            })
        );
        assert_eq!(
            heic.colr.icc,
            Some(IccColorProfile {
                profile_type: FourCc::new(*b"rICC"),
                profile: vec![0x10, 0x20, 0x30],
            })
        );
    }

    #[test]
    fn rejects_primary_heic_property_parse_when_required_property_is_missing() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
        ];
        let meta = make_primary_item_meta_with_properties(*b"hvc1", iloc, &properties, &[]);
        let err =
            parse_primary_heic_item_properties(&meta).expect_err("missing hvcC property must fail");
        assert_eq!(
            err,
            ParsePrimaryHeicPropertiesError::MissingRequiredProperty {
                item_id: 1,
                property_type: FourCc::new(*b"hvcC"),
            }
        );
    }

    #[test]
    fn rejects_primary_heic_property_parse_when_property_is_duplicated() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![
            make_hvcc_property(),
            make_hvcc_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
        ];
        let meta = make_primary_item_meta_with_properties(*b"hvc1", iloc, &properties, &[]);
        let err = parse_primary_heic_item_properties(&meta)
            .expect_err("duplicate hvcC property must fail");
        assert_eq!(
            err,
            ParsePrimaryHeicPropertiesError::DuplicateProperty {
                item_id: 1,
                property_type: FourCc::new(*b"hvcC"),
            }
        );
    }

    #[test]
    fn rejects_av1c_parse_when_marker_bit_is_unset() {
        let av1c = make_basic_box(*b"av1C", &[0x01, 0x00, 0x00, 0x00]);
        let parsed = parse_boxes(&av1c).expect("av1C box should parse");

        let err = parsed[0]
            .parse_av1c()
            .expect_err("av1C marker-bit violation must fail");
        assert_eq!(
            err,
            ParseAv1CodecConfigurationBoxError::InvalidMarkerBit {
                offset: BASIC_HEADER_SIZE as u64,
                value: 0x01,
            }
        );
    }

    #[test]
    fn rejects_hvcc_parse_for_unsupported_configuration_version() {
        let hvcc = make_hvcc_property_with_configuration_version(2);
        let parsed = parse_boxes(&hvcc).expect("hvcC box should parse");

        let err = parsed[0]
            .parse_hvcc()
            .expect_err("unsupported hvcC configuration version must fail");
        assert_eq!(
            err,
            ParseHevcDecoderConfigurationBoxError::UnsupportedConfigurationVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 2,
            }
        );
    }

    #[test]
    fn rejects_ispe_parse_for_unsupported_version() {
        let ispe = make_ispe_property_with_version(1, 640, 480);
        let parsed = parse_boxes(&ispe).expect("ispe box should parse");

        let err = parsed[0]
            .parse_ispe()
            .expect_err("unsupported ispe version must fail");
        assert_eq!(
            err,
            ParseImageSpatialExtentsPropertyError::UnsupportedVersion {
                offset: BASIC_HEADER_SIZE as u64,
                version: 1,
            }
        );
    }

    #[test]
    fn rejects_pixi_parse_when_channel_data_is_truncated() {
        let pixi = make_basic_box(*b"pixi", &[0x00, 0x00, 0x00, 0x00, 0x03, 0x08, 0x08]);
        let parsed = parse_boxes(&pixi).expect("pixi box should parse");

        let err = parsed[0]
            .parse_pixi()
            .expect_err("truncated pixi channel list must fail");
        assert_eq!(
            err,
            ParsePixelInformationPropertyError::PayloadTooSmall {
                offset: (BASIC_HEADER_SIZE + FULL_BOX_HEADER_SIZE + 1) as u64,
                context: "bits_per_channel",
                available: 2,
                required: 3,
            }
        );
    }

    #[test]
    fn parses_colr_nclx_and_nclc_profiles() {
        let nclx = make_colr_nclx_property(1, 13, 6, true);
        let parsed = parse_boxes(&nclx).expect("colr box should parse");
        let nclx_property = parsed[0]
            .parse_colr()
            .expect("nclx colr property should parse");
        assert_eq!(nclx_property.colour_type, FourCc::new(*b"nclx"));
        assert_eq!(
            nclx_property.information,
            ColorInformation::Nclx(NclxColorProfile {
                colour_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 6,
                full_range_flag: true,
            })
        );

        let nclc = make_colr_nclc_property(1, 13, 0);
        let parsed = parse_boxes(&nclc).expect("colr box should parse");
        let nclc_property = parsed[0]
            .parse_colr()
            .expect("nclc colr property should parse");
        assert_eq!(nclc_property.colour_type, FourCc::new(*b"nclc"));
        assert_eq!(
            nclc_property.information,
            ColorInformation::Nclx(NclxColorProfile {
                colour_primaries: 1,
                transfer_characteristics: 13,
                matrix_coefficients: 0,
                full_range_flag: true,
            })
        );
    }

    #[test]
    fn parses_colr_icc_payload_types() {
        let prof = make_colr_icc_property(*b"prof", &[0xAA, 0xBB]);
        let parsed = parse_boxes(&prof).expect("colr box should parse");
        let property = parsed[0]
            .parse_colr()
            .expect("ICC colr property should parse");
        assert_eq!(property.colour_type, FourCc::new(*b"prof"));
        assert_eq!(
            property.information,
            ColorInformation::Icc(IccColorProfile {
                profile_type: FourCc::new(*b"prof"),
                profile: vec![0xAA, 0xBB],
            })
        );
    }

    #[test]
    fn rejects_colr_parse_for_unknown_colour_type() {
        let colr = make_basic_box(*b"colr", b"zzzz");
        let parsed = parse_boxes(&colr).expect("colr box should parse");
        let err = parsed[0]
            .parse_colr()
            .expect_err("unknown colour_type must fail");
        assert_eq!(
            err,
            ParseColorInformationPropertyError::UnknownColourType {
                offset: BASIC_HEADER_SIZE as u64,
                colour_type: FourCc::new(*b"zzzz"),
            }
        );
    }

    #[test]
    fn parses_irot_property_rotation_steps() {
        let irot = make_irot_property(3);
        let parsed = parse_boxes(&irot).expect("irot box should parse");
        let property = parsed[0]
            .parse_irot()
            .expect("irot property should parse rotation");
        assert_eq!(property.rotation_ccw_degrees, 270);
    }

    #[test]
    fn rejects_irot_parse_when_payload_is_truncated() {
        let irot = make_basic_box(*b"irot", &[]);
        let parsed = parse_boxes(&irot).expect("irot box should parse");
        let err = parsed[0]
            .parse_irot()
            .expect_err("truncated irot payload must fail");
        assert_eq!(
            err,
            ParseImageRotationPropertyError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 0,
                required: 1,
            }
        );
    }

    #[test]
    fn parses_imir_property_direction_bit() {
        let horizontal = make_imir_property(true);
        let parsed = parse_boxes(&horizontal).expect("imir box should parse");
        let property = parsed[0]
            .parse_imir()
            .expect("imir property should parse direction");
        assert_eq!(property.direction, ImageMirrorDirection::Horizontal);

        let vertical = make_imir_property(false);
        let parsed = parse_boxes(&vertical).expect("imir box should parse");
        let property = parsed[0]
            .parse_imir()
            .expect("imir property should parse direction");
        assert_eq!(property.direction, ImageMirrorDirection::Vertical);
    }

    #[test]
    fn rejects_imir_parse_when_payload_is_truncated() {
        let imir = make_basic_box(*b"imir", &[]);
        let parsed = parse_boxes(&imir).expect("imir box should parse");
        let err = parsed[0]
            .parse_imir()
            .expect_err("truncated imir payload must fail");
        assert_eq!(
            err,
            ParseImageMirrorPropertyError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 0,
                required: 1,
            }
        );
    }

    #[test]
    fn parses_clap_property_fields() {
        let clap = make_clap_property(ImageCleanApertureProperty {
            clean_aperture_width_num: 640,
            clean_aperture_width_den: 1,
            clean_aperture_height_num: 480,
            clean_aperture_height_den: 1,
            horizontal_offset_num: -10,
            horizontal_offset_den: 2,
            vertical_offset_num: 6,
            vertical_offset_den: 2,
        });
        let parsed = parse_boxes(&clap).expect("clap box should parse");
        let property = parsed[0]
            .parse_clap()
            .expect("clap property should parse fields");
        assert_eq!(
            property,
            ImageCleanApertureProperty {
                clean_aperture_width_num: 640,
                clean_aperture_width_den: 1,
                clean_aperture_height_num: 480,
                clean_aperture_height_den: 1,
                horizontal_offset_num: -10,
                horizontal_offset_den: 2,
                vertical_offset_num: 6,
                vertical_offset_den: 2,
            }
        );
    }

    #[test]
    fn rejects_clap_parse_when_payload_is_truncated() {
        let clap = make_basic_box(*b"clap", &[0; 12]);
        let parsed = parse_boxes(&clap).expect("clap box should parse");
        let err = parsed[0]
            .parse_clap()
            .expect_err("truncated clap payload must fail");
        assert_eq!(
            err,
            ParseImageCleanAperturePropertyError::PayloadTooSmall {
                offset: BASIC_HEADER_SIZE as u64,
                available: 12,
                required: 32,
            }
        );
    }

    #[test]
    fn rejects_clap_parse_when_denominator_is_zero() {
        let clap = make_clap_property(ImageCleanApertureProperty {
            clean_aperture_width_num: 640,
            clean_aperture_width_den: 0,
            clean_aperture_height_num: 480,
            clean_aperture_height_den: 1,
            horizontal_offset_num: 0,
            horizontal_offset_den: 1,
            vertical_offset_num: 0,
            vertical_offset_den: 1,
        });
        let parsed = parse_boxes(&clap).expect("clap box should parse");
        let err = parsed[0]
            .parse_clap()
            .expect_err("clap denominator zero must fail");
        assert_eq!(
            err,
            ParseImageCleanAperturePropertyError::ZeroDenominator {
                offset: BASIC_HEADER_SIZE as u64 + 4,
                field: "clean_aperture_width_den",
            }
        );
    }

    #[test]
    fn parses_primary_item_transform_properties_in_association_order() {
        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x0000_u16.to_be_bytes()); // all size fields are zero
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
        iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // extent_count
        let iloc = make_basic_box(*b"iloc", &iloc_payload);

        let properties = vec![
            make_av1c_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
            make_clap_property(ImageCleanApertureProperty {
                clean_aperture_width_num: 638,
                clean_aperture_width_den: 1,
                clean_aperture_height_num: 478,
                clean_aperture_height_den: 1,
                horizontal_offset_num: -1,
                horizontal_offset_den: 2,
                vertical_offset_num: -1,
                vertical_offset_den: 2,
            }),
            make_imir_property(true),
            make_irot_property(1),
        ];
        let meta = make_primary_avif_meta_with_properties(iloc, &properties, &[]);
        let transforms = parse_primary_item_transform_properties(&meta)
            .expect("primary transform parse should preserve association order");

        assert_eq!(transforms.item_id, 1);
        assert_eq!(
            transforms.transforms,
            vec![
                PrimaryItemTransformProperty::CleanAperture(ImageCleanApertureProperty {
                    clean_aperture_width_num: 638,
                    clean_aperture_width_den: 1,
                    clean_aperture_height_num: 478,
                    clean_aperture_height_den: 1,
                    horizontal_offset_num: -1,
                    horizontal_offset_den: 2,
                    vertical_offset_num: -1,
                    vertical_offset_den: 2,
                }),
                PrimaryItemTransformProperty::Mirror(ImageMirrorProperty {
                    direction: ImageMirrorDirection::Horizontal,
                }),
                PrimaryItemTransformProperty::Rotation(ImageRotationProperty {
                    rotation_ccw_degrees: 90,
                }),
            ]
        );
    }

    fn make_primary_avif_meta(iloc: Vec<u8>, additional_children: &[Vec<u8>]) -> Vec<u8> {
        let properties = vec![
            make_av1c_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
        ];
        make_primary_item_meta_with_properties(*b"av01", iloc, &properties, additional_children)
    }

    fn make_primary_avif_meta_with_properties(
        iloc: Vec<u8>,
        properties: &[Vec<u8>],
        additional_children: &[Vec<u8>],
    ) -> Vec<u8> {
        make_primary_item_meta_with_properties(*b"av01", iloc, properties, additional_children)
    }

    fn make_primary_heic_meta(iloc: Vec<u8>, additional_children: &[Vec<u8>]) -> Vec<u8> {
        make_primary_heic_meta_with_item_type(*b"hvc1", iloc, additional_children)
    }

    fn make_primary_heic_meta_with_item_type(
        item_type: [u8; 4],
        iloc: Vec<u8>,
        additional_children: &[Vec<u8>],
    ) -> Vec<u8> {
        let properties = vec![
            make_hvcc_property(),
            make_ispe_property(640, 480),
            make_pixi_property(0, &[8, 8, 8]),
        ];
        make_primary_item_meta_with_properties(item_type, iloc, &properties, additional_children)
    }

    fn make_primary_grid_heic_meta(
        iloc: Vec<u8>,
        iinf: Vec<u8>,
        iref: Option<Vec<u8>>,
        additional_children: &[Vec<u8>],
    ) -> Vec<u8> {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let iprp = make_iprp_without_associations();

        let mut children = vec![pitm, iloc, iinf, iprp];
        if let Some(iref) = iref {
            children.push(iref);
        }
        children.extend_from_slice(additional_children);
        make_meta_box(&children)
    }

    fn make_iinf_with_entries(entries: &[(u16, [u8; 4], &[u8])]) -> Vec<u8> {
        assert!(
            entries.len() <= u16::MAX as usize,
            "too many entries for test iinf"
        );

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&(entries.len() as u16).to_be_bytes()); // item_count
        for (item_id, item_type, item_name) in entries {
            let mut infe_payload = Vec::new();
            infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
            infe_payload.extend_from_slice(&item_id.to_be_bytes());
            infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
            infe_payload.extend_from_slice(item_type);
            infe_payload.extend_from_slice(item_name);
            infe_payload.push(0); // C-string terminator
            iinf_payload.extend_from_slice(&make_basic_box(*b"infe", &infe_payload));
        }

        make_basic_box(*b"iinf", &iinf_payload)
    }

    fn make_iloc_v0_single_extent_items(items: &[(u16, u32, u32, u32)]) -> Vec<u8> {
        assert!(
            items.len() <= u16::MAX as usize,
            "too many entries for test iloc"
        );

        let mut iloc_payload = Vec::new();
        iloc_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iloc_payload.extend_from_slice(&0x4440_u16.to_be_bytes()); // offset/length/base_offset=4
        iloc_payload.extend_from_slice(&(items.len() as u16).to_be_bytes()); // item_count
        for (item_id, base_offset, extent_offset, extent_length) in items {
            iloc_payload.extend_from_slice(&item_id.to_be_bytes());
            iloc_payload.extend_from_slice(&0_u16.to_be_bytes()); // data_reference_index
            iloc_payload.extend_from_slice(&base_offset.to_be_bytes());
            iloc_payload.extend_from_slice(&1_u16.to_be_bytes()); // extent_count
            iloc_payload.extend_from_slice(&extent_offset.to_be_bytes());
            iloc_payload.extend_from_slice(&extent_length.to_be_bytes());
        }

        make_basic_box(*b"iloc", &iloc_payload)
    }

    fn make_iref_dimg_v0(from_item_id: u16, to_item_ids: &[u16]) -> Vec<u8> {
        assert!(
            to_item_ids.len() <= u16::MAX as usize,
            "too many test dimg references"
        );

        let mut dimg_payload = Vec::new();
        dimg_payload.extend_from_slice(&from_item_id.to_be_bytes());
        dimg_payload.extend_from_slice(&(to_item_ids.len() as u16).to_be_bytes());
        for to_item_id in to_item_ids {
            dimg_payload.extend_from_slice(&to_item_id.to_be_bytes());
        }

        let dimg = make_basic_box(*b"dimg", &dimg_payload);
        let mut iref_payload = Vec::new();
        iref_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iref_payload.extend_from_slice(&dimg);
        make_basic_box(*b"iref", &iref_payload)
    }

    fn make_iprp_without_associations() -> Vec<u8> {
        let ipco = make_basic_box(*b"ipco", &[]);
        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&0_u32.to_be_bytes()); // entry_count
        let ipma = make_basic_box(*b"ipma", &ipma_payload);

        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        make_basic_box(*b"iprp", &iprp_payload)
    }

    fn make_primary_item_meta_with_properties(
        item_type: [u8; 4],
        iloc: Vec<u8>,
        properties: &[Vec<u8>],
        additional_children: &[Vec<u8>],
    ) -> Vec<u8> {
        let pitm = make_basic_box(*b"pitm", &[0x00, 0x00, 0x00, 0x00, 0x00, 0x01]);

        let mut infe_payload = Vec::new();
        infe_payload.extend_from_slice(&[0x02, 0x00, 0x00, 0x00]); // version=2, flags=0
        infe_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        infe_payload.extend_from_slice(&0_u16.to_be_bytes()); // item_protection_index
        infe_payload.extend_from_slice(&item_type); // item_type
        infe_payload.extend_from_slice(b"primary\0"); // item_name
        let infe = make_basic_box(*b"infe", &infe_payload);

        let mut iinf_payload = Vec::new();
        iinf_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        iinf_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_count
        iinf_payload.extend_from_slice(&infe);
        let iinf = make_basic_box(*b"iinf", &iinf_payload);

        assert!(
            properties.len() <= 0x7F,
            "too many properties for test ipma"
        );
        let mut ipco_payload = Vec::new();
        for property in properties {
            ipco_payload.extend_from_slice(property);
        }
        let ipco = make_basic_box(*b"ipco", &ipco_payload);

        let mut ipma_payload = Vec::new();
        ipma_payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        ipma_payload.extend_from_slice(&1_u32.to_be_bytes()); // entry_count
        ipma_payload.extend_from_slice(&1_u16.to_be_bytes()); // item_ID
        ipma_payload.push(properties.len() as u8); // association_count
        for property_index in 1..=properties.len() {
            ipma_payload.push(property_index as u8); // property_index
        }
        let ipma = make_basic_box(*b"ipma", &ipma_payload);

        let mut iprp_payload = Vec::new();
        iprp_payload.extend_from_slice(&ipco);
        iprp_payload.extend_from_slice(&ipma);
        let iprp = make_basic_box(*b"iprp", &iprp_payload);

        let mut children = vec![pitm, iloc, iinf, iprp];
        children.extend_from_slice(additional_children);
        make_meta_box(&children)
    }

    fn make_hvcc_property() -> Vec<u8> {
        make_hvcc_property_with_configuration_version(1)
    }

    fn make_hvcc_property_with_configuration_version(configuration_version: u8) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(configuration_version); // configurationVersion
        payload.push(0x01); // profile_space=0, tier=0, profile_idc=1
        payload.extend_from_slice(&0x6000_0000_u32.to_be_bytes()); // profile compatibility flags
        payload.extend_from_slice(&[0x00; 6]); // constraint indicator flags
        payload.push(120); // level_idc
        payload.extend_from_slice(&0xF000_u16.to_be_bytes()); // min_spatial_segmentation_idc
        payload.push(0xFC); // parallelism_type (masked to 0)
        payload.push(0xFC); // chroma_format (masked to 0)
        payload.push(0xF8); // bit_depth_luma_minus8 = 0
        payload.push(0xF8); // bit_depth_chroma_minus8 = 0
        payload.extend_from_slice(&0_u16.to_be_bytes()); // avg_frame_rate
        payload.push(0x03); // length_size_minus_one = 3 => 4-byte NAL lengths
        payload.push(1); // numOfArrays
        payload.push(33); // array_completeness=0, nal_unit_type=33 (SPS)
        payload.extend_from_slice(&1_u16.to_be_bytes()); // numNalus
        payload.extend_from_slice(&2_u16.to_be_bytes()); // nalUnitLength
        payload.extend_from_slice(&[0x42, 0x01]); // nalUnit bytes
        make_basic_box(*b"hvcC", &payload)
    }

    fn make_av1c_property() -> Vec<u8> {
        make_basic_box(*b"av1C", &[0x81, 0x55, 0xDA, 0x15, 0xAA, 0xBB])
    }

    fn make_ispe_property(width: u32, height: u32) -> Vec<u8> {
        make_ispe_property_with_version(0, width, height)
    }

    fn make_ispe_property_with_version(version: u8, width: u32, height: u32) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.push(version);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]);
        payload.extend_from_slice(&width.to_be_bytes());
        payload.extend_from_slice(&height.to_be_bytes());
        make_basic_box(*b"ispe", &payload)
    }

    fn make_pixi_property(version: u8, bits_per_channel: &[u8]) -> Vec<u8> {
        assert!(
            bits_per_channel.len() <= u8::MAX as usize,
            "too many channels for test pixi property"
        );

        let mut payload = Vec::new();
        payload.push(version);
        payload.extend_from_slice(&[0x00, 0x00, 0x00]);
        payload.push(bits_per_channel.len() as u8);
        payload.extend_from_slice(bits_per_channel);
        make_basic_box(*b"pixi", &payload)
    }

    fn make_colr_nclx_property(
        colour_primaries: u16,
        transfer_characteristics: u16,
        matrix_coefficients: u16,
        full_range_flag: bool,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"nclx");
        payload.extend_from_slice(&colour_primaries.to_be_bytes());
        payload.extend_from_slice(&transfer_characteristics.to_be_bytes());
        payload.extend_from_slice(&matrix_coefficients.to_be_bytes());
        payload.push(if full_range_flag { 0x80 } else { 0x00 });
        make_basic_box(*b"colr", &payload)
    }

    fn make_colr_nclc_property(
        colour_primaries: u16,
        transfer_characteristics: u16,
        matrix_coefficients: u16,
    ) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(b"nclc");
        payload.extend_from_slice(&colour_primaries.to_be_bytes());
        payload.extend_from_slice(&transfer_characteristics.to_be_bytes());
        payload.extend_from_slice(&matrix_coefficients.to_be_bytes());
        make_basic_box(*b"colr", &payload)
    }

    fn make_colr_icc_property(profile_type: [u8; 4], profile: &[u8]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&profile_type);
        payload.extend_from_slice(profile);
        make_basic_box(*b"colr", &payload)
    }

    fn make_irot_property(rotation_steps_ccw: u8) -> Vec<u8> {
        make_basic_box(*b"irot", &[rotation_steps_ccw & 0x03])
    }

    fn make_imir_property(horizontal: bool) -> Vec<u8> {
        let axis = if horizontal { 1 } else { 0 };
        make_basic_box(*b"imir", &[axis])
    }

    fn make_clap_property(property: ImageCleanApertureProperty) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&property.clean_aperture_width_num.to_be_bytes());
        payload.extend_from_slice(&property.clean_aperture_width_den.to_be_bytes());
        payload.extend_from_slice(&property.clean_aperture_height_num.to_be_bytes());
        payload.extend_from_slice(&property.clean_aperture_height_den.to_be_bytes());
        payload.extend_from_slice(&property.horizontal_offset_num.to_be_bytes());
        payload.extend_from_slice(&property.horizontal_offset_den.to_be_bytes());
        payload.extend_from_slice(&property.vertical_offset_num.to_be_bytes());
        payload.extend_from_slice(&property.vertical_offset_den.to_be_bytes());
        make_basic_box(*b"clap", &payload)
    }

    fn make_meta_box(children: &[Vec<u8>]) -> Vec<u8> {
        let mut payload = Vec::new();
        payload.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // version=0, flags=0
        for child in children {
            payload.extend_from_slice(child);
        }
        make_basic_box(*b"meta", &payload)
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
