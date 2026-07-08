use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Healthy,
    Warn,
    Critical,
    Stale,
    Unavailable,
}

impl Status {
    pub fn badge(self) -> &'static str {
        match self {
            Self::Healthy => "OK",
            Self::Warn => "WARN",
            Self::Critical => "CRIT",
            Self::Stale => "STALE",
            Self::Unavailable => "UNAVAILABLE",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Healthy => Color::Green,
            Self::Warn => Color::Yellow,
            Self::Critical => Color::Red,
            Self::Stale => Color::LightYellow,
            Self::Unavailable => Color::DarkGray,
        }
    }
}

pub const BG: Color = Color::Rgb(8, 12, 18);
pub const PANEL: Color = Color::Rgb(20, 27, 37);
pub const BORDER: Color = Color::Rgb(56, 70, 89);
pub const TEXT: Color = Color::Rgb(219, 229, 243);
pub const MUTED: Color = Color::Rgb(128, 142, 163);
pub const ACCENT: Color = Color::Rgb(90, 200, 250);
pub const CPU: Color = Color::Rgb(123, 220, 181);
pub const RAM: Color = Color::Rgb(150, 180, 255);
pub const STORAGE: Color = Color::Rgb(255, 184, 108);
pub const NETWORK: Color = Color::Rgb(95, 214, 231);
