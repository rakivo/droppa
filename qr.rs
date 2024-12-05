extern crate raylib_light as raylib;

use raylib::*;
use qrcodegen::QrCode;

const SCALE: u32 = 8;
const QR_SIZE: u32 = 25;
const QUIET_ZONE: u32 = 1;
const QUIET_ZONE_SIZE: i32 = 10;
const IMAGE_SIZE: u32 = (QR_SIZE + 2 * QUIET_ZONE) * SCALE;

pub unsafe fn gen_qr_canvas(qr: &QrCode) -> Texture2D {
    let canvas = LoadRenderTexture(IMAGE_SIZE as i32, IMAGE_SIZE as i32);
    BeginTextureMode(canvas);
    ClearBackground(WHITE);

    for y in 0..QR_SIZE {
        for x in 0..QR_SIZE {
            let color = if qr.get_module(x as i32, y as i32) { BLACK } else { WHITE };
            let y = QR_SIZE - 1 - y;
            for dy in 0..SCALE {
                for dx in 0..SCALE {
                    let px = (x + QUIET_ZONE) * SCALE + dx;
                    let py = (y + QUIET_ZONE) * SCALE + dy;
                    DrawPixel(px as i32, py as i32, color);
                }
            }
        }
    }

    EndTextureMode();
    canvas.texture
}

pub unsafe fn init_raylib() {
    SetWindowState(ConfigFlags::WindowHidden as u32 | ConfigFlags::WindowUndecorated as u32);
    InitWindow(230 + QUIET_ZONE_SIZE, 230 + QUIET_ZONE_SIZE, cstr!("qr"));
    SetTargetFPS(30);
}

pub unsafe fn draw_qr_code(qr_texture: Texture2D) {
    ClearWindowState(ConfigFlags::WindowHidden as _);
    while !WindowShouldClose() {
        BeginDrawing();
        ClearBackground(WHITE);
        DrawTexture(qr_texture, QUIET_ZONE_SIZE + 2, QUIET_ZONE_SIZE + 1, WHITE);
        EndDrawing();
    }
}
