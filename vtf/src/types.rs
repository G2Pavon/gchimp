use image::DynamicImage;
use nom::IResult as _IResult;

use crate::formats::VtfImage;

pub(crate) type IResult<'a, T> = _IResult<&'a [u8], T>;

pub type ImageData = Vec<u8>;

#[derive(Debug, Clone)]
pub struct Header {
    /// [u8; 4]
    pub signature: Vec<u8>,
    /// [u32; 2]
    pub version: Vec<u32>,
    pub header_size: u32,
    pub width: u16,
    pub height: u16,
    pub flags: u32,
    pub frames: u16,
    // has to be i16 so -1 is allowed
    pub first_frame: i16,
    // pub padding0: [u8; 4],
    /// [f32; 3]
    pub reflectivity: Vec<f32>,
    // pub padding1: [u8; 4],
    pub bump_map_scale: f32,
    pub high_res_image_format: i32,
    pub mipmap_count: u8,
    pub low_res_image_format: i32,
    pub low_res_image_width: u8,
    pub low_res_image_height: u8,
    pub header72: Option<Header72>,
    pub header73: Option<Header73>,
}

#[derive(Debug, Clone)]
pub struct Header72 {
    pub depth: u16,
}

#[derive(Debug, Clone)]
pub struct Header73 {
    // pub padding2: [u8; 3],
    pub num_resources: u32,
    // pub padding3: [u8; 8],
}

#[derive(Debug, Clone)]
pub struct ResourceEntry {
    // [u8; 3]
    pub tag: ResourceEntryTag,
    pub flags: u8,
    pub offset: u32,
}

#[derive(Debug, Clone)]
pub enum ResourceEntryTag {
    LowRes,
    HighRes,
    AnimatedParticleSheet,
    CRC,
    TextureLODControl,
    ExtendedVTF,
    KeyValues,
}

impl TryFrom<&[u8]> for ResourceEntryTag {
    type Error = &'static str;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        if value.len() != 3 {
            return Err(format!("invalid slice length {} instead of 3", value.len()).leak());
        }

        let conv = |i: [char; 3]| i.iter().map(|&e| e as u8).collect::<Vec<u8>>();

        if value == &[0x01, 0, 0] {
            Ok(Self::LowRes)
        } else if value == &[0x30, 0, 0] {
            Ok(Self::HighRes)
        } else if value == &[0x10, 0, 0] {
            Ok(Self::AnimatedParticleSheet)
        } else if value == conv(['C', 'R', 'C']).as_slice() {
            Ok(Self::CRC)
        } else if value == conv(['L', 'O', 'D']).as_slice() {
            Ok(Self::TextureLODControl)
        } else if value == conv(['T', 'S', 'O']).as_slice() {
            Ok(Self::ExtendedVTF)
        } else if value == conv(['K', 'V', 'D']).as_slice() {
            Ok(Self::KeyValues)
        } else {
            Err("invalid resource entry tag")
        }
    }
}

#[repr(u32)]
pub enum VtfFlag {
    // Flags from the *.txt config file
    TextureflagsPointsample = 0x00000001,
    TextureflagsTrilinear = 0x00000002,
    TextureflagsClamps = 0x00000004,
    TextureflagsClampt = 0x00000008,
    TextureflagsAnisotropic = 0x00000010,
    TextureflagsHintDxt5 = 0x00000020,
    TextureflagsPwlCorrected = 0x00000040,
    TextureflagsNormal = 0x00000080,
    TextureflagsNomip = 0x00000100,
    TextureflagsNolod = 0x00000200,
    TextureflagsAllMips = 0x00000400,
    TextureflagsProcedural = 0x00000800,

    // These are automatically generated by vtex from the texture data.
    TextureflagsOnebitalpha = 0x00001000,
    TextureflagsEightbitalpha = 0x00002000,

    // Newer flags from the *.txt config file
    TextureflagsEnvmap = 0x00004000,
    TextureflagsRendertarget = 0x00008000,
    TextureflagsDepthrendertarget = 0x00010000,
    TextureflagsNodebugoverride = 0x00020000,
    TextureflagsSinglecopy = 0x00040000,
    TextureflagsPreSrgb = 0x00080000,

    TextureflagsUnused00100000 = 0x00100000,
    TextureflagsUnused00200000 = 0x00200000,
    TextureflagsUnused00400000 = 0x00400000,

    TextureflagsNodepthbuffer = 0x00800000,

    TextureflagsUnused01000000 = 0x01000000,

    TextureflagsClampu = 0x02000000,
    TextureflagsVertextexture = 0x04000000,
    TextureflagsSsbump = 0x08000000,

    TextureflagsUnused10000000 = 0x10000000,

    TextureflagsBorder = 0x20000000,

    TextureflagsUnused40000000 = 0x40000000,
    TextureflagsUnused80000000 = 0x80000000,
}

#[derive(Debug, Clone)]
pub enum Resource {
    LowRes(VtfImage),
    HighRes(VtfHighResImage),
    AnimatedParticleSheet,
    CRC,
    TextureLODControl,
    ExtendedVTF,
    KeyValues,
}

#[derive(Debug, Clone)]
pub struct VtfHighResImage {
    pub mipmaps: Vec<MipMap>,
}

impl VtfHighResImage {
    pub fn get_high_res_image(&self) -> eyre::Result<DynamicImage> {
        let Some(mipmap) = self.mipmaps.last() else {
            return Err(eyre::eyre!("no mipmaps"));
        };

        let Some(frame) = mipmap.frames.first() else {
            return Err(eyre::eyre!("no frames"));
        };

        let Some(face) = frame.faces.first() else {
            return Err(eyre::eyre!("no faces"));
        };

        Ok(face.image.to_image())
    }
}

#[derive(Debug, Clone)]
pub struct MipMap {
    pub frames: Vec<Frame>,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub faces: Vec<Face>,
}

#[derive(Debug, Clone)]
pub struct Face {
    pub image: VtfImage,
}

pub type Vtf73Data = Vec<Resource>;

#[derive(Debug, Clone)]
pub struct Vtf72Data {
    pub low_res: VtfImage,
    pub high_res: VtfHighResImage,
}

#[derive(Debug, Clone)]
pub enum VtfData {
    Vtf72(Vtf72Data),
    Vtf73(Vtf73Data),
}

#[derive(Debug, Clone)]
pub struct Vtf {
    pub header: Header,
    pub data: VtfData,
}
