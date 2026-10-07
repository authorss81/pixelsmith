use image::imageops::FilterType;
fn px(b: &image::ImageBuffer<image::Rgba<u8>, Vec<u8>>, x: u32, y: u32) -> u8 { b.get_pixel(x,y).0[0] }
fn simd(src: &image::RgbaImage, w: u32, h: u32) -> image::RgbaImage {
    let s = fast_image_resize::images::ImageRef::new(src.width(), src.height(), src.as_raw(), fast_image_resize::PixelType::U8x4).unwrap();
    let mut d = fast_image_resize::images::Image::new(w, h, fast_image_resize::PixelType::U8x4);
    fast_image_resize::Resizer::new().resize(&s, &mut d, &fast_image_resize::ResizeOptions::new().resize_alg(fast_image_resize::ResizeAlg::Nearest).use_alpha(false)).unwrap();
    image::RgbaImage::from_raw(w, h, d.into_vec()).unwrap()
}
fn main() {
    // 4x4 -> 6x6 upscale, red channel encodes y only (so rows visible, cols constant)
    let src = image::RgbaImage::from_fn(4,4,|_,y| image::Rgba([(y as u8)*60,0,0,255]));
    let r = image::imageops::resize(&src, 6, 6, FilterType::Nearest);
    let s = simd(&src, 6, 6);
    println!("4x4->6x6 rows image: {:?}", (0..6).map(|y| px(&r,0,y)).collect::<Vec<_>>());
    println!("4x4->6x6 rows simd:  {:?}", (0..6).map(|y| px(&s,0,y)).collect::<Vec<_>>());
    let src = image::RgbaImage::from_fn(4,4,|x,_| image::Rgba([(x as u8)*60,0,0,255]));
    let r = image::imageops::resize(&src, 6, 6, FilterType::Nearest);
    let s = simd(&src, 6, 6);
    println!("4x4->6x6 cols image: {:?}", (0..6).map(|x| px(&r,x,0)).collect::<Vec<_>>());
    println!("4x4->6x6 cols simd:  {:?}", (0..6).map(|x| px(&s,x,0)).collect::<Vec<_>>());
    // 4x4 -> 5x5 (1.25)
    let src = image::RgbaImage::from_fn(4,4,|_,y| image::Rgba([(y as u8)*60,0,0,255]));
    let r = image::imageops::resize(&src, 5, 5, FilterType::Nearest);
    let s = simd(&src, 5, 5);
    println!("4x4->5x5 rows image: {:?}", (0..5).map(|y| px(&r,0,y)).collect::<Vec<_>>());
    println!("4x4->5x5 rows simd:  {:?}", (0..5).map(|y| px(&s,0,y)).collect::<Vec<_>>());
}
