//! Integration tests for `decode_bitmap`.
//!
//! Verifies Raw8888 + Raw565 succeed; QOI / PNG / RLE / WebP return
//! `Unsupported` (the v1 contract).

use weftos_leaf_renderer::{decode_bitmap, BitmapError};
use weftos_leaf_scene::{px, BitmapFormat, Rgba};

#[test]
fn raw8888_decodes_2x2() {
    // R, G,
    // B, W
    #[rustfmt::skip]
    let data: Vec<u8> = vec![
        0xFF, 0x00, 0x00, 0xFF,
        0x00, 0xFF, 0x00, 0xFF,
        0x00, 0x00, 0xFF, 0xFF,
        0xFF, 0xFF, 0xFF, 0xFF,
    ];
    let d = decode_bitmap(px(2), px(2), BitmapFormat::Raw8888, &data).unwrap();
    assert_eq!(d.w, 2);
    assert_eq!(d.h, 2);
    assert_eq!(d.pixel(0, 0), Rgba::RED);
    assert_eq!(d.pixel(1, 0), Rgba::GREEN);
    assert_eq!(d.pixel(0, 1), Rgba::BLUE);
    assert_eq!(d.pixel(1, 1), Rgba::WHITE);
}

#[test]
fn raw565_decodes_2x1() {
    // px0 = 0xF800 LE = [0x00, 0xF8] (red)
    // px1 = 0x07E0 LE = [0xE0, 0x07] (green)
    let data: Vec<u8> = vec![0x00, 0xF8, 0xE0, 0x07];
    let d = decode_bitmap(px(2), px(1), BitmapFormat::Raw565, &data).unwrap();
    assert_eq!(d.w, 2);
    assert_eq!(d.h, 1);
    let red = d.pixel(0, 0);
    let green = d.pixel(1, 0);
    assert_eq!(red.r, 0xFF);
    assert_eq!(red.g, 0);
    assert_eq!(red.b, 0);
    assert_eq!(green.r, 0);
    assert!(green.g >= 0xFC); // 6-bit -> 8-bit replication
    assert_eq!(green.b, 0);
}

#[test]
fn qoi_returns_unsupported() {
    let data: Vec<u8> = vec![0u8; 100];
    let r = decode_bitmap(px(10), px(10), BitmapFormat::Qoi, &data);
    matches!(r, Err(BitmapError::Unsupported(BitmapFormat::Qoi)));
}

#[test]
fn png_returns_unsupported() {
    let data: Vec<u8> = vec![0u8; 100];
    let r = decode_bitmap(px(10), px(10), BitmapFormat::Png, &data);
    matches!(r, Err(BitmapError::Unsupported(BitmapFormat::Png)));
}

#[test]
fn rle_returns_unsupported() {
    let data: Vec<u8> = vec![0u8; 100];
    let r = decode_bitmap(px(10), px(10), BitmapFormat::Rle, &data);
    matches!(r, Err(BitmapError::Unsupported(BitmapFormat::Rle)));
}

#[test]
fn webp_returns_unsupported() {
    let data: Vec<u8> = vec![0u8; 100];
    let r = decode_bitmap(px(10), px(10), BitmapFormat::WebP, &data);
    matches!(r, Err(BitmapError::Unsupported(BitmapFormat::WebP)));
}

#[test]
fn size_mismatch_returns_error() {
    let data: Vec<u8> = vec![0u8; 3];
    let r = decode_bitmap(px(2), px(2), BitmapFormat::Raw8888, &data);
    match r {
        Err(BitmapError::SizeMismatch { expected, got }) => {
            assert_eq!(expected, 16);
            assert_eq!(got, 3);
        }
        other => panic!("expected SizeMismatch, got {other:?}"),
    }
}
