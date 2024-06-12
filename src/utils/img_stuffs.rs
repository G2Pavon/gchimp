use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{BufWriter, Cursor, Write},
    path::{Path, PathBuf},
};

use eyre::eyre;
use image::{imageops, RgbImage, RgbaImage};
use quantette::{ColorSpace, ImagePipeline, QuantizeMethod};
use rayon::prelude::*;

use crate::utils::constants::MAX_GOLDSRC_TEXTURE_SIZE;

use super::constants::{PALETTE_PAD_COLOR, PALETTE_TRANSPARENT_COLOR};

type Palette = Vec<quantette::palette::rgb::Rgb<quantette::palette::encoding::Srgb, u8>>;

/// The pixels are quantized with following palette.
///
/// ## Must convert image to 8bpp with the palette.
fn quantize_image(img: RgbImage) -> eyre::Result<(RgbImage, Palette)> {
    let pipeline = ImagePipeline::try_from(&img)?
        .palette_size(255)
        .dither(true)
        .colorspace(ColorSpace::Oklab)
        .quantize_method(QuantizeMethod::kmeans());

    let img = pipeline.clone().quantized_rgbimage_par();
    let palette: Palette = pipeline.palette_par();

    Ok((img, palette))
}

fn maybe_resize_due_to_exceeding_max_goldsrc_texture_size(img: RgbaImage) -> RgbaImage {
    let (width, height) = img.dimensions();

    let bigger_side = if width >= height { width } else { height };
    let q = bigger_side as f32 / MAX_GOLDSRC_TEXTURE_SIZE as f32;

    if q <= 1. {
        img
    } else {
        let (width, height) = (width as f32 / q, height as f32 / q);
        let (width, height) = (width.round() as u32, height.round() as u32);
        imageops::resize(
            &img,
            width,
            height,
            // eh, meow?
            imageops::FilterType::Lanczos3,
        )
    }
}

fn rgba8_to_rgb8_blended(img: RgbaImage) -> eyre::Result<RgbImage> {
    let (width, height) = img.dimensions();
    let buf = img
        .par_chunks_exact(4)
        .flat_map(|p| {
            let opacity = p[3] as f32 / 255.;
            [
                (p[0] as f32 * opacity).round() as u8,
                (p[1] as f32 * opacity).round() as u8,
                (p[2] as f32 * opacity).round() as u8,
            ]
        })
        .collect::<Vec<u8>>();

    let res = match RgbImage::from_vec(width, height, buf) {
        Some(buf) => Ok(buf),
        None => Err(eyre!("Cannot convert Rgba to Rgb")),
    }?;

    Ok(res)
}

#[allow(dead_code)]
// Replace any transparent pixel with a color if alpha channel is below the threshold
fn rgba8_to_rgb8_replace(
    img: RgbaImage,
    replacement: &[u8],
    threshold: u8,
) -> eyre::Result<RgbImage> {
    let (width, height) = img.dimensions();
    let buf = img
        .par_chunks_exact(4)
        .flat_map(|p| {
            let should_replace = threshold >= p[3];

            if should_replace {
                [replacement[0], replacement[1], replacement[2]]
            } else {
                [p[0], p[1], p[2]]
            }
        })
        .collect::<Vec<u8>>();

    let res = match RgbImage::from_vec(width, height, buf) {
        Some(buf) => Ok(buf),
        None => Err(eyre!("Cannot convert Rgba to Rgb")),
    }?;

    Ok(res)
}

fn format_quantette_palette(palette: Palette) -> Vec<[u8; 3]> {
    palette
        .iter()
        .map(|p| [p.red, p.green, p.blue])
        .collect::<Vec<[u8; 3]>>()
}

fn rgb8_to_8bpp(img: RgbImage, palette: &[[u8; 3]]) -> Vec<u8> {
    img.chunks_exact(3)
        .map(|p| {
            // unwrap is guaranteed because img uses palette colors
            let index_for_color = palette
                .iter()
                .position(|pp| *pp == [p[0], p[1], p[2]])
                .unwrap();
            index_for_color as u8
        })
        .collect::<Vec<u8>>()
}

pub fn any_format_to_bmp_write_to_file(
    img_path: impl AsRef<Path> + Into<PathBuf>,
) -> eyre::Result<()> {
    let img = image::open(img_path.as_ref())?.into_rgba8();
    let rgba8 = maybe_resize_due_to_exceeding_max_goldsrc_texture_size(img);
    let GoldSrcBmp {
        image: img,
        palette,
        dimensions: dimension,
    } = rgba8_to_8bpp(rgba8)?;

    let mut out_img = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(img_path.as_ref().with_extension("bmp"))?;

    let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut out_img);

    encoder.encode_with_palette(
        &img,
        dimension.0,
        dimension.1,
        image::ExtendedColorType::L8,
        Some(&palette),
    )?;

    out_img.flush()?;

    Ok(())
}

pub fn any_format_to_png(img_path: impl AsRef<Path> + Into<PathBuf>) -> eyre::Result<Vec<u8>> {
    let img = image::open(img_path.as_ref())?;

    let mut buf: Vec<u8> = vec![];

    img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)?;

    Ok(buf)
}

pub fn any_format_to_8bpp(img_path: impl AsRef<Path> + Into<PathBuf>) -> eyre::Result<GoldSrcBmp> {
    let img = image::open(img_path.as_ref())?.into_rgba8();
    let rgba8 = maybe_resize_due_to_exceeding_max_goldsrc_texture_size(img);
    let res = rgba8_to_8bpp(rgba8)?;

    Ok(res)
}

pub fn png_to_bmp_folder(paths: &[PathBuf]) -> eyre::Result<()> {
    let err: Vec<eyre::Error> = paths
        .par_iter()
        .filter_map(|path| any_format_to_bmp_write_to_file(path).err())
        .collect();

    if !err.is_empty() {
        let err_str = err
            .iter()
            .fold(String::new(), |acc, e| format!("{}\n{}", acc, e));

        return Err(eyre::eyre!(err_str));
    }

    Ok(())
}

pub fn rgba8_to_8bpp(rgb8a: RgbaImage) -> eyre::Result<GoldSrcBmp> {
    let rgb8 = rgba8_to_rgb8_blended(rgb8a)?;
    let (rgb8, palette_color) = quantize_image(rgb8)?;

    let dimension = rgb8.dimensions();

    let palette_color_arr = format_quantette_palette(palette_color);
    let img_bmp_8pp = rgb8_to_8bpp(rgb8, &palette_color_arr);

    Ok(GoldSrcBmp {
        image: img_bmp_8pp,
        palette: palette_color_arr,
        dimensions: dimension,
    })
}

/// `file_name` should have .bmp have extension
pub fn write_8bpp_to_file(
    img: &[u8],
    palette: &[[u8; 3]],
    dimension: (u32, u32),
    file_path: impl AsRef<Path>,
) -> eyre::Result<()> {
    assert!(file_path.as_ref().extension().is_some());
    assert!(file_path.as_ref().extension().unwrap() == "bmp");

    let mut out_img = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(file_path)?;

    let mut writer = BufWriter::new(&mut out_img);
    let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut writer);

    encoder.encode_with_palette(
        img,
        dimension.0,
        dimension.1,
        image::ExtendedColorType::L8,
        Some(palette),
    )?;

    writer.flush()?;

    Ok(())
}

// need to encode it into a format so it has all the info to easily display every where
pub fn encode_8bpp_to_bitmap(
    img: &[u8],
    palette: &[[u8; 3]],
    dimension: (u32, u32),
) -> eyre::Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut encoder = image::codecs::bmp::BmpEncoder::new(&mut buf);

    encoder.encode_with_palette(
        img,
        dimension.0,
        dimension.1,
        image::ExtendedColorType::L8,
        Some(palette),
    )?;

    Ok(buf)
}

// egui doesn't take bitmap, what in tarnation?
pub fn eight_bpp_bitmap_to_png_bytes(
    img: &[u8],
    palette: &[[u8; 3]],
    (width, height): (u32, u32),
) -> eyre::Result<Vec<u8>> {
    let img_buffer = img
        .iter()
        .flat_map(|palette_index| palette[*palette_index as usize])
        .collect::<Vec<u8>>();
    let img_buffer = RgbImage::from_vec(width, height, img_buffer).unwrap();

    let mut bytes: Vec<u8> = Vec::new();
    img_buffer.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;

    Ok(bytes)
}

// tile the image in a way that the resulting image has the same dimension as the original
pub fn tile_and_resize(img: &RgbaImage, scalar: u32) -> RgbaImage {
    let (width, height) = img.dimensions();
    let mut res = RgbaImage::new(width * scalar, height * scalar);

    imageops::tile(&mut res, img);

    // this doesn't modify the original image, LOL wtf
    imageops::resize(&res, width, height, imageops::FilterType::Lanczos3)
}

// threshold is between 0 and 1
// if the percentage of the most used color is over the threshold, mark the color transparent
pub fn eight_bpp_transparent_img(
    img: &[u8],
    palette: &[[u8; 3]],
    threshold: f32,
) -> (Vec<u8>, Vec<[u8; 3]>) {
    // find most used color and its count
    let (most_used_color, most_used_color_count) = img
        .iter()
        .fold(HashMap::<u8, usize>::new(), |mut acc, p| {
            if let Some(count) = acc.get_mut(p) {
                *count += 1;
            } else {
                acc.insert(*p, 1);
            }

            acc
        })
        .iter()
        .fold((0, 0), |(acc_pixel, acc_count), (pixel, count)| {
            if *count > acc_count {
                (*pixel, *count)
            } else {
                (acc_pixel, acc_count)
            }
        });

    let over_threshold = most_used_color_count as f32 / img.len() as f32 >= threshold;

    if !over_threshold {
        return (img.to_vec(), palette.to_vec());
    }

    let mut new_palette = palette.to_vec();
    let mut new_img = img.to_vec();
    let palette_count = new_palette.len();

    // pad palette
    for _ in 0..(256 - palette_count) {
        new_palette.push(PALETTE_PAD_COLOR);
    }

    // change the final color of the palette to a rare color
    new_palette[255] = PALETTE_TRANSPARENT_COLOR;

    // swap the most used color (index) with 255
    for pixel in new_img.iter_mut() {
        if *pixel == most_used_color {
            *pixel = 255;
        }
    }

    (new_img, new_palette)
}

// TODO: better mipmaps generation because this is very SHIT
#[allow(clippy::type_complexity)]
pub fn generate_mipmaps(
    img_path: impl AsRef<Path> + Into<PathBuf>,
) -> eyre::Result<([Vec<u8>; 4], Vec<[u8; 3]>, (u32, u32))> {
    let img = image::open(img_path.as_ref())?.into_rgba8();
    let mip0 = maybe_resize_due_to_exceeding_max_goldsrc_texture_size(img);

    let mip0 = rgba8_to_rgb8_blended(mip0)?;
    let (mip0, palette_color) = quantize_image(mip0)?;

    let (width, height) = mip0.dimensions();

    let palette = format_quantette_palette(palette_color);

    let mip1 = imageops::resize(&mip0, width / 2, height / 2, imageops::FilterType::Nearest);
    let mip2 = imageops::resize(
        &mip0,
        width / 2 / 2,
        height / 2 / 2,
        imageops::FilterType::Nearest,
    );
    let mip3 = imageops::resize(
        &mip0,
        width / 2 / 2 / 2,
        height / 2 / 2 / 2,
        imageops::FilterType::Nearest,
    );

    let mip0 = rgb8_to_8bpp(mip0, &palette);
    let mip1 = rgb8_to_8bpp(mip1, &palette);
    let mip2 = rgb8_to_8bpp(mip2, &palette);
    let mip3 = rgb8_to_8bpp(mip3, &palette);

    Ok(([mip0, mip1, mip2, mip3], palette, (width, height)))
}

#[derive(Debug)]
pub struct GoldSrcBmp {
    pub image: Vec<u8>,
    pub palette: Vec<[u8; 3]>,
    pub dimensions: (u32, u32),
}
