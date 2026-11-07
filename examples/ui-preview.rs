//! Render the actual 96x16 OLED fonts and layout to a host BMP contact sheet.
//! Run from outside the embedded Cargo configuration; see tools/check.sh.
use embedded_graphics::{pixelcolor::BinaryColor, prelude::*};
use pinecil_rs::{
    app::AppState,
    motion::MotionState,
    power::PowerStatus,
    sensors::{Measurements, Reading},
    settings::RotationMode,
    ui::{Edit, MenuItem, Screen, Ui},
    view::{Frame, SIZE},
};
use std::{convert::Infallible, io::Write};

struct Panel([bool; 96 * 16]);
impl OriginDimensions for Panel {
    fn size(&self) -> Size {
        SIZE
    }
}
impl DrawTarget for Panel {
    type Color = BinaryColor;
    type Error = Infallible;
    fn draw_iter<I: IntoIterator<Item = Pixel<BinaryColor>>>(
        &mut self,
        pixels: I,
    ) -> Result<(), Infallible> {
        for Pixel(point, color) in pixels {
            if (0..96).contains(&point.x) && (0..16).contains(&point.y) {
                self.0[point.y as usize * 96 + point.x as usize] = color == BinaryColor::On;
            }
        }
        Ok(())
    }
}
fn main() -> std::io::Result<()> {
    let path = std::env::args().nth(1).expect("output BMP path");
    let mut screens = vec![Screen::Main];
    screens.extend(MenuItem::ROOT.into_iter().map(Screen::Menu));
    screens.extend(MenuItem::DISPLAY.into_iter().map(Screen::Menu));
    screens.extend([
        Screen::Edit(Edit::Temperature(175)),
        Screen::Edit(Edit::Brightness(3)),
        Screen::Edit(Edit::Rotation(RotationMode::Auto)),
        Screen::Edit(Edit::Rotation(RotationMode::Right)),
        Screen::ConfirmDefaults,
        Screen::Measurements { from_menu: true },
    ]);
    let mut pixels = vec![0u8; 3 * 312 * 110];
    for (n, screen) in screens.into_iter().enumerate() {
        let mut ui = Ui::default();
        ui.screen = screen;
        let frame = Frame::new(&AppState {
            ui,
            power: PowerStatus::Contract {
                mv: 28_000,
                ma: 3000,
            },
            motion: MotionState::WAITING,
            measurements: Measurements {
                input_mv: Reading::Value(27_980),
                tip_centi_c: Reading::Value(2500),
                handle_centi_c: Reading::Value(2600),
                ..Measurements::WAITING
            },
        });
        let mut panel = Panel([false; 96 * 16]);
        frame.draw_header(&mut panel).unwrap();
        frame.draw_footer(&mut panel, true).unwrap();
        for y in 0..16 {
            for x in 0..96 {
                let px = n % 3 * 104 + x + 4;
                let py = n / 3 * 22 + y + 3;
                let index = (py * 312 + px) * 3;
                let color = if panel.0[y * 96 + x] {
                    [255, 230, 190]
                } else {
                    [28, 20, 12]
                };
                pixels[index..index + 3].copy_from_slice(&color);
            }
        }
        println!(
            "{:2}: {:?}: {} / {}",
            n + 1,
            screen,
            frame.header,
            frame.footer
        );
    }
    // Bottom-up 24-bit BMP, integer 4x scaling preserves the exact glyph pixels.
    let (width, height) = (312u32 * 4, 110u32 * 4);
    let data_size = width * height * 3;
    let mut output = std::fs::File::create(path)?;
    output.write_all(b"BM")?;
    output.write_all(&(54 + data_size).to_le_bytes())?;
    output.write_all(&[0; 4])?;
    output.write_all(&54u32.to_le_bytes())?;
    output.write_all(&40u32.to_le_bytes())?;
    output.write_all(&width.to_le_bytes())?;
    output.write_all(&height.to_le_bytes())?;
    output.write_all(&1u16.to_le_bytes())?;
    output.write_all(&24u16.to_le_bytes())?;
    output.write_all(&[0; 24])?;
    for y in (0..height as usize).rev() {
        for x in 0..width as usize {
            let index = ((y / 4) * 312 + x / 4) * 3;
            output.write_all(&pixels[index..index + 3])?;
        }
    }
    Ok(())
}
