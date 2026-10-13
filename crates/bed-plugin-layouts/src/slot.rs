//! Compatibility names for applications using nine numbered layouts and a startup layout.
use serde_json::{Value, json};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum LayoutSlot {
    #[default]
    Layout1,
    Layout2,
    Layout3,
    Layout4,
    Layout5,
    Layout6,
    Layout7,
    Layout8,
    Layout9,
    Startup,
}

impl LayoutSlot {
    pub const ALL: [Self; 10] = [
        Self::Layout1,
        Self::Layout2,
        Self::Layout3,
        Self::Layout4,
        Self::Layout5,
        Self::Layout6,
        Self::Layout7,
        Self::Layout8,
        Self::Layout9,
        Self::Startup,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Layout1 => "1",
            Self::Layout2 => "2",
            Self::Layout3 => "3",
            Self::Layout4 => "4",
            Self::Layout5 => "5",
            Self::Layout6 => "6",
            Self::Layout7 => "7",
            Self::Layout8 => "8",
            Self::Layout9 => "9",
            Self::Startup => "startup",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Layout1 => "Layout 1",
            Self::Layout2 => "Layout 2",
            Self::Layout3 => "Layout 3",
            Self::Layout4 => "Layout 4",
            Self::Layout5 => "Layout 5",
            Self::Layout6 => "Layout 6",
            Self::Layout7 => "Layout 7",
            Self::Layout8 => "Layout 8",
            Self::Layout9 => "Layout 9",
            Self::Startup => "Startup",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|slot| slot.key() == key)
    }

    pub fn from_value(value: &Value) -> Option<Self> {
        if value.as_str() == Some("startup") {
            Some(Self::Startup)
        } else {
            value
                .as_u64()
                .filter(|number| (1..=9).contains(number))
                .map(|number| Self::ALL[number as usize - 1])
        }
    }

    pub fn to_value(self) -> Value {
        if self == Self::Startup {
            json!("startup")
        } else {
            json!(self.key().parse::<u8>().unwrap())
        }
    }
}

impl std::fmt::Display for LayoutSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.key())
    }
}
