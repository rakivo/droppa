use qrcodegen::QrCode;

use crate::stb_image_write::*;

const SCALE: usize = 10;
const BORDER: usize = 2;

pub fn gen_qr_png_bytes(qr: &QrCode) -> Result::<Vec::<u8>, ()> {
    let size = qr.size() as usize;
    let img_size = (size + 2 * BORDER) * SCALE;

    let mut image = vec![0xFF; img_size * img_size];
    for y in 0..size {
        for x in 0..size {
            if !qr.get_module(x as _, y as _) { continue }
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    let px = (BORDER + x) * SCALE + dx;
                    let py = (BORDER + y) * SCALE + dy;
                    image[py * img_size + px] = 0;
                }
            }
        }
    }

    unsafe { write_png_to_memory(&image, img_size as _, img_size as _) }
}

pub unsafe fn write_png_to_memory(image: &[u8], width: i32, height: i32) -> Result::<Vec::<u8>, ()> {
    let mut out_len = 0;

    let ret = stbi_write_png_to_mem(
        image.as_ptr(),
        width,
        width,
        height,
        1,
        &mut out_len as *mut i32
    );

    if !ret.is_null() {
        Ok(std::slice::from_raw_parts_mut(ret, out_len as usize).to_vec())
    } else {
        Err(())
    }
}
