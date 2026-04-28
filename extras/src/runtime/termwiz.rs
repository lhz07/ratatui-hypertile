use ratatui::style::{Color, Modifier, Style};
pub use termwiz;
use termwiz::cell::{Blink, CellAttributes, Intensity, Underline};
use termwiz::color::{AnsiColor, ColorAttribute, ColorSpec, LinearRgba, RgbColor, SrgbaTuple};

/// A trait for converting types from Termwiz to Ratatui.
///
/// This trait replaces the `From` trait for converting types from Termwiz to Ratatui. It is
/// necessary because the `From` trait is not implemented for types defined in external crates.
pub trait FromTermwiz<T> {
    /// Converts the given Termwiz type to the Ratatui type.
    fn from_termwiz(termwiz: &T) -> Self;
}

/// A replacement for the `Into` trait for converting types from Ratatui to Termwiz.
///
/// This trait is necessary because the `Into` trait is not implemented for types defined in
/// external crates.
///
/// A blanket implementation is provided for all types that implement `FromTermwiz`.
///
/// This trait is private to the module as it would otherwise conflict with the other backend
/// modules. It is mainly used to avoid rewriting all the `.into()` calls in this module.
pub trait IntoRatatui<R> {
    fn into_ratatui(&self) -> R;
}

impl<C, R: FromTermwiz<C>> IntoRatatui<R> for C {
    fn into_ratatui(&self) -> R {
        R::from_termwiz(self)
    }
}

impl FromTermwiz<CellAttributes> for Style {
    fn from_termwiz(value: &CellAttributes) -> Self {
        let mut style = Self::new()
            .add_modifier(value.intensity().into_ratatui())
            .add_modifier(value.underline().into_ratatui())
            .add_modifier(value.blink().into_ratatui());

        if value.italic() {
            style.add_modifier |= Modifier::ITALIC;
        }
        if value.reverse() {
            style.add_modifier |= Modifier::REVERSED;
        }
        if value.strikethrough() {
            style.add_modifier |= Modifier::CROSSED_OUT;
        }
        if value.invisible() {
            style.add_modifier |= Modifier::HIDDEN;
        }

        style.fg = Some(value.foreground().into_ratatui());
        style.bg = Some(value.background().into_ratatui());

        style.underline_color = Some(value.underline_color().into_ratatui());

        style
    }
}

impl FromTermwiz<Intensity> for Modifier {
    fn from_termwiz(value: &Intensity) -> Self {
        match value {
            Intensity::Normal => Self::empty(),
            Intensity::Bold => Self::BOLD,
            Intensity::Half => Self::DIM,
        }
    }
}

impl FromTermwiz<Underline> for Modifier {
    fn from_termwiz(value: &Underline) -> Self {
        match value {
            Underline::None => Self::empty(),
            _ => Self::UNDERLINED,
        }
    }
}

impl FromTermwiz<Blink> for Modifier {
    fn from_termwiz(value: &Blink) -> Self {
        match value {
            Blink::None => Self::empty(),
            Blink::Slow => Self::SLOW_BLINK,
            Blink::Rapid => Self::RAPID_BLINK,
        }
    }
}

impl FromTermwiz<AnsiColor> for Color {
    fn from_termwiz(value: &AnsiColor) -> Self {
        match value {
            AnsiColor::Black => Self::Black,
            AnsiColor::Grey => Self::DarkGray,
            AnsiColor::Silver => Self::Gray,
            AnsiColor::Maroon => Self::Red,
            AnsiColor::Red => Self::LightRed,
            AnsiColor::Green => Self::Green,
            AnsiColor::Lime => Self::LightGreen,
            AnsiColor::Olive => Self::Yellow,
            AnsiColor::Yellow => Self::LightYellow,
            AnsiColor::Purple => Self::Magenta,
            AnsiColor::Fuchsia => Self::LightMagenta,
            AnsiColor::Teal => Self::Cyan,
            AnsiColor::Aqua => Self::LightCyan,
            AnsiColor::White => Self::White,
            AnsiColor::Navy => Self::Blue,
            AnsiColor::Blue => Self::LightBlue,
        }
    }
}

impl FromTermwiz<ColorAttribute> for Color {
    fn from_termwiz(value: &ColorAttribute) -> Self {
        match value {
            ColorAttribute::TrueColorWithDefaultFallback(srgba)
            | ColorAttribute::TrueColorWithPaletteFallback(srgba, _) => srgba.into_ratatui(),
            ColorAttribute::PaletteIndex(i) => Self::Indexed(*i),
            ColorAttribute::Default => Self::Reset,
        }
    }
}

impl FromTermwiz<ColorSpec> for Color {
    fn from_termwiz(value: &ColorSpec) -> Self {
        match value {
            ColorSpec::Default => Self::Reset,
            ColorSpec::PaletteIndex(i) => Self::Indexed(*i),
            ColorSpec::TrueColor(srgba) => srgba.into_ratatui(),
        }
    }
}

impl FromTermwiz<SrgbaTuple> for Color {
    fn from_termwiz(value: &SrgbaTuple) -> Self {
        let (r, g, b, _) = value.to_srgb_u8();
        Self::Rgb(r, g, b)
    }
}

impl FromTermwiz<RgbColor> for Color {
    fn from_termwiz(value: &RgbColor) -> Self {
        let (r, g, b) = value.to_tuple_rgb8();
        Self::Rgb(r, g, b)
    }
}

impl FromTermwiz<LinearRgba> for Color {
    fn from_termwiz(value: &LinearRgba) -> Self {
        value.to_srgb().into_ratatui()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod into_color {
        use Color as C;

        use super::*;

        #[test]
        fn from_linear_rgba() {
            // full black + opaque
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 0., 0., 1.)),
                Color::Rgb(0, 0, 0)
            );
            // full black + transparent
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 0., 0., 0.)),
                Color::Rgb(0, 0, 0)
            );

            // full white + opaque
            assert_eq!(
                C::from_termwiz(&LinearRgba(1., 1., 1., 1.)),
                C::Rgb(254, 254, 254)
            );
            // full white + transparent
            assert_eq!(
                C::from_termwiz(&LinearRgba(1., 1., 1., 0.)),
                C::Rgb(254, 254, 254)
            );

            // full red
            assert_eq!(
                C::from_termwiz(&LinearRgba(1., 0., 0., 1.)),
                C::Rgb(254, 0, 0)
            );
            // full green
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 1., 0., 1.)),
                C::Rgb(0, 254, 0)
            );
            // full blue
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 0., 1., 1.)),
                C::Rgb(0, 0, 254)
            );

            // See https://stackoverflow.com/questions/12524623/what-are-the-practical-differences-when-working-with-colors-in-a-linear-vs-a-no
            // for an explanation

            // half red
            assert_eq!(
                C::from_termwiz(&LinearRgba(0.214, 0., 0., 1.)),
                C::Rgb(127, 0, 0)
            );
            // half green
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 0.214, 0., 1.)),
                C::Rgb(0, 127, 0)
            );
            // half blue
            assert_eq!(
                C::from_termwiz(&LinearRgba(0., 0., 0.214, 1.)),
                C::Rgb(0, 0, 127)
            );
        }

        #[test]
        fn from_srgba() {
            // full black + opaque
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 0., 0., 1.)),
                Color::Rgb(0, 0, 0)
            );
            // full black + transparent
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 0., 0., 0.)),
                Color::Rgb(0, 0, 0)
            );

            // full white + opaque
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(1., 1., 1., 1.)),
                C::Rgb(255, 255, 255)
            );
            // full white + transparent
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(1., 1., 1., 0.)),
                C::Rgb(255, 255, 255)
            );

            // full red
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(1., 0., 0., 1.)),
                C::Rgb(255, 0, 0)
            );
            // full green
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 1., 0., 1.)),
                C::Rgb(0, 255, 0)
            );
            // full blue
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 0., 1., 1.)),
                C::Rgb(0, 0, 255)
            );

            // half red
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0.5, 0., 0., 1.)),
                C::Rgb(127, 0, 0)
            );
            // half green
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 0.5, 0., 1.)),
                C::Rgb(0, 127, 0)
            );
            // half blue
            assert_eq!(
                C::from_termwiz(&SrgbaTuple(0., 0., 0.5, 1.)),
                C::Rgb(0, 0, 127)
            );
        }

        #[test]
        fn from_rgbcolor() {
            // full black
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(0, 0, 0)),
                Color::Rgb(0, 0, 0)
            );
            // full white
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(255, 255, 255)),
                C::Rgb(255, 255, 255)
            );

            // full red
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(255, 0, 0)),
                C::Rgb(255, 0, 0)
            );
            // full green
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(0, 255, 0)),
                C::Rgb(0, 255, 0)
            );
            // full blue
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(0, 0, 255)),
                C::Rgb(0, 0, 255)
            );

            // half red
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(127, 0, 0)),
                C::Rgb(127, 0, 0)
            );
            // half green
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(0, 127, 0)),
                C::Rgb(0, 127, 0)
            );
            // half blue
            assert_eq!(
                C::from_termwiz(&RgbColor::new_8bpc(0, 0, 127)),
                C::Rgb(0, 0, 127)
            );
        }

        #[test]
        fn from_colorspec() {
            assert_eq!(C::from_termwiz(&ColorSpec::Default), C::Reset);
            assert_eq!(
                C::from_termwiz(&ColorSpec::PaletteIndex(33)),
                C::Indexed(33)
            );
            assert_eq!(
                C::from_termwiz(&ColorSpec::TrueColor(SrgbaTuple(0., 0., 0., 1.))),
                C::Rgb(0, 0, 0)
            );
        }

        #[test]
        fn from_colorattribute() {
            assert_eq!(C::from_termwiz(&ColorAttribute::Default), C::Reset);
            assert_eq!(
                C::from_termwiz(&ColorAttribute::PaletteIndex(32)),
                C::Indexed(32)
            );
            assert_eq!(
                C::from_termwiz(&ColorAttribute::TrueColorWithDefaultFallback(SrgbaTuple(
                    0., 0., 0., 1.
                ))),
                C::Rgb(0, 0, 0)
            );
            assert_eq!(
                C::from_termwiz(&ColorAttribute::TrueColorWithPaletteFallback(
                    SrgbaTuple(0., 0., 0., 1.),
                    31
                )),
                C::Rgb(0, 0, 0)
            );
        }

        #[test]
        fn from_ansicolor() {
            assert_eq!(C::from_termwiz(&AnsiColor::Black), Color::Black);
            assert_eq!(C::from_termwiz(&AnsiColor::Grey), Color::DarkGray);
            assert_eq!(C::from_termwiz(&AnsiColor::Silver), Color::Gray);
            assert_eq!(C::from_termwiz(&AnsiColor::Maroon), Color::Red);
            assert_eq!(C::from_termwiz(&AnsiColor::Red), Color::LightRed);
            assert_eq!(C::from_termwiz(&AnsiColor::Green), Color::Green);
            assert_eq!(C::from_termwiz(&AnsiColor::Lime), Color::LightGreen);
            assert_eq!(C::from_termwiz(&AnsiColor::Olive), Color::Yellow);
            assert_eq!(C::from_termwiz(&AnsiColor::Yellow), Color::LightYellow);
            assert_eq!(C::from_termwiz(&AnsiColor::Purple), Color::Magenta);
            assert_eq!(C::from_termwiz(&AnsiColor::Fuchsia), Color::LightMagenta);
            assert_eq!(C::from_termwiz(&AnsiColor::Teal), Color::Cyan);
            assert_eq!(C::from_termwiz(&AnsiColor::Aqua), Color::LightCyan);
            assert_eq!(C::from_termwiz(&AnsiColor::White), Color::White);
            assert_eq!(C::from_termwiz(&AnsiColor::Navy), Color::Blue);
            assert_eq!(C::from_termwiz(&AnsiColor::Blue), Color::LightBlue);
        }
    }

    mod into_modifier {
        use super::*;

        #[test]
        fn from_intensity() {
            assert_eq!(
                Modifier::from_termwiz(&Intensity::Normal),
                Modifier::empty()
            );
            assert_eq!(Modifier::from_termwiz(&Intensity::Bold), Modifier::BOLD);
            assert_eq!(Modifier::from_termwiz(&Intensity::Half), Modifier::DIM);
        }

        #[test]
        fn from_underline() {
            assert_eq!(Modifier::from_termwiz(&Underline::None), Modifier::empty());
            assert_eq!(
                Modifier::from_termwiz(&Underline::Single),
                Modifier::UNDERLINED
            );
            assert_eq!(
                Modifier::from_termwiz(&Underline::Double),
                Modifier::UNDERLINED
            );
            assert_eq!(
                Modifier::from_termwiz(&Underline::Curly),
                Modifier::UNDERLINED
            );
            assert_eq!(
                Modifier::from_termwiz(&Underline::Dashed),
                Modifier::UNDERLINED
            );
            assert_eq!(
                Modifier::from_termwiz(&Underline::Dotted),
                Modifier::UNDERLINED
            );
        }

        #[test]
        fn from_blink() {
            assert_eq!(Modifier::from_termwiz(&Blink::None), Modifier::empty());
            assert_eq!(Modifier::from_termwiz(&Blink::Slow), Modifier::SLOW_BLINK);
            assert_eq!(Modifier::from_termwiz(&Blink::Rapid), Modifier::RAPID_BLINK);
        }
    }

    #[test]
    fn from_cell_attribute_for_style() {
        const STYLE: Style = Style::new()
            .underline_color(Color::Reset)
            .fg(Color::Reset)
            .bg(Color::Reset);

        // default
        assert_eq!(Style::from_termwiz(&CellAttributes::default()), STYLE);

        // foreground color
        assert_eq!(
            Style::from_termwiz(
                &CellAttributes::default()
                    .set_foreground(ColorAttribute::PaletteIndex(31))
                    .to_owned()
            ),
            STYLE.fg(Color::Indexed(31))
        );
        // background color
        assert_eq!(
            Style::from_termwiz(
                &CellAttributes::default()
                    .set_background(ColorAttribute::PaletteIndex(31))
                    .to_owned()
            ),
            STYLE.bg(Color::Indexed(31))
        );
        // underlined
        assert_eq!(
            Style::from_termwiz(
                &CellAttributes::default()
                    .set_underline(Underline::Single)
                    .to_owned()
            ),
            STYLE.underlined()
        );
        // blink
        assert_eq!(
            Style::from_termwiz(&CellAttributes::default().set_blink(Blink::Slow).to_owned()),
            STYLE.slow_blink()
        );
        // intensity
        assert_eq!(
            Style::from_termwiz(
                &CellAttributes::default()
                    .set_intensity(Intensity::Bold)
                    .to_owned()
            ),
            STYLE.bold()
        );
        // italic
        assert_eq!(
            Style::from_termwiz(&CellAttributes::default().set_italic(true).to_owned()),
            STYLE.italic()
        );
        // reversed
        assert_eq!(
            Style::from_termwiz(&CellAttributes::default().set_reverse(true).to_owned()),
            STYLE.reversed()
        );
        // strikethrough
        assert_eq!(
            Style::from_termwiz(&CellAttributes::default().set_strikethrough(true).to_owned()),
            STYLE.crossed_out()
        );
        // hidden
        assert_eq!(
            Style::from_termwiz(&CellAttributes::default().set_invisible(true).to_owned()),
            STYLE.hidden()
        );

        // underline color
        assert_eq!(
            Style::from_termwiz(
                &CellAttributes::default()
                    .set_underline_color(AnsiColor::Red)
                    .to_owned()
            ),
            STYLE.underline_color(Color::Indexed(9))
        );
    }
}
