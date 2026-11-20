use crate::ui::Palette;
use iced::{
    Element, Font, Length, Size, Theme,
    alignment::Horizontal,
    font::Weight,
    mouse,
    widget::{column, container, text},
};
use iced_runtime::user_interface::{Cache, UserInterface};
use iced_tiny_skia::{Renderer, core::renderer, graphics::Viewport, window::compositor};

#[derive(Default)]
pub(super) struct ViewState {
    clock: String,
    date: String,
    pub(super) password_length: usize,
    pub(super) show_error: bool,
    pub(super) checking: bool,
    pub(super) caps_lock: bool,
}

impl ViewState {
    pub(super) fn refresh_clock(&mut self) -> bool {
        let now = chrono::Local::now();
        let clock = now.format("%H:%M").to_string();
        if self.clock == clock {
            return false;
        }
        self.clock = clock;
        self.date = now.format("%A, %d %B").to_string();
        true
    }

    fn element(&self, palette: Palette) -> Element<'_, (), Theme, Renderer> {
        let colors = palette.theme().palette();
        let status = if self.checking {
            "Checking password…"
        } else if self.show_error {
            "Authentication failed. Try again."
        } else if self.caps_lock {
            "Caps Lock is on"
        } else {
            "Type your password and press Enter"
        };
        let indicator = if self.password_length == 0 {
            String::from("Password")
        } else {
            "•".repeat(self.password_length.min(24))
        };
        let input = container(text(indicator).size(16).align_x(Horizontal::Center))
            .center_x(Length::Fill)
            .padding([11, 12])
            .style(move |_| container::Style {
                background: Some(colors.background.into()),
                border: iced::Border {
                    color: colors.primary,
                    width: 1.0,
                    radius: 9.0.into(),
                },
                ..Default::default()
            });
        let content = column![
            text("Frost").size(15).color(colors.primary),
            text(&self.clock).size(58).font(Font {
                weight: Weight::Semibold,
                ..Font::DEFAULT
            }),
            text(&self.date).size(16),
            input,
            text(status).size(13).color(if self.show_error {
                colors.danger
            } else {
                colors.text
            }),
        ]
        .align_x(iced::Alignment::Center)
        .spacing(12)
        .width(300);
        container(content)
            .padding(16)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    }
}

pub(super) fn new_renderer() -> Renderer {
    Renderer::new(Font::DEFAULT, 16.0.into())
}

pub(super) fn draw(
    renderer: &mut Renderer,
    state: &ViewState,
    palette: Palette,
    width: u32,
    height: u32,
    scale: u32,
) -> Vec<u8> {
    let theme = palette.theme();
    let bounds = Size::new(width as f32, height as f32);
    // The renderer holds weak references to text paragraphs owned by the UI.
    let mut ui = UserInterface::build(state.element(palette), bounds, Cache::new(), renderer);
    ui.draw(
        renderer,
        &theme,
        &renderer::Style {
            text_color: theme.palette().text,
        },
        mouse::Cursor::Unavailable,
    );
    let viewport =
        Viewport::with_physical_size(Size::new(width * scale, height * scale), scale as f32);
    compositor::screenshot(renderer, &viewport, palette.background)
}

// SCTK pads SHM slots to 64 bytes. The compositor reads only the frame.
pub(super) fn copy_xrgb(rgba: &[u8], slot: &mut [u8], frame_bytes: usize) -> Result<(), String> {
    let destination = slot
        .get_mut(..frame_bytes)
        .ok_or("lock buffer is too small")?;
    if rgba.len() != frame_bytes || !frame_bytes.is_multiple_of(4) {
        return Err("renderer returned an unexpected lock buffer size".into());
    }
    for (pixel, dest) in rgba
        .as_chunks::<4>()
        .0
        .iter()
        .zip(destination.as_chunks_mut::<4>().0)
    {
        *dest = u32::from_be_bytes([0xff, pixel[0], pixel[1], pixel[2]]).to_ne_bytes();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_an_xrgb_frame_into_a_padded_wayland_slot() {
        let mut slot = [0_u8; 64];
        copy_xrgb(&[0x12, 0x34, 0x56, 0xff], &mut slot, 4).unwrap();
        assert_eq!(slot[..4], 0xff123456_u32.to_ne_bytes());
        assert_eq!(slot[4..], [0_u8; 60]);
        assert!(copy_xrgb(&[0; 4], &mut slot, 8).is_err());
    }

    #[test]
    fn draws_an_opaque_hidpi_lock_frame() {
        let mut state = ViewState {
            password_length: 4,
            ..ViewState::default()
        };
        state.refresh_clock();
        let pixels = draw(&mut new_renderer(), &state, Palette::load(), 480, 320, 2);
        assert_eq!(pixels.len(), 960 * 640 * 4);
        let pixels = pixels.as_chunks::<4>().0;
        assert!(pixels.iter().all(|pixel| pixel[3] == u8::MAX));
        let foreground = Palette::load().theme().palette().text.into_rgba8();
        assert!(
            pixels.iter().any(
                |pixel| (0..3).all(|channel| pixel[channel].abs_diff(foreground[channel]) < 10)
            )
        );
    }
}
