//! Stable ladder identity shared by strategy intent and the order table.
//!
//! A level is an identity, not a position in a sorted list. Price changes therefore never make one
//! live order masquerade as another level, and an absent level has exactly one meaning: withdraw
//! the order carrying that identity.

pub const MAX_QUOTE_LEVELS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct QuoteLevel(u8);

impl QuoteLevel {
    pub const ZERO: Self = Self(0);

    /// Every rung, in ladder order. A pass over the ladder walks this rather than round-tripping
    /// each index through the fallible constructor.
    pub const ALL: [Self; MAX_QUOTE_LEVELS] = {
        let mut levels = [Self(0); MAX_QUOTE_LEVELS];
        let mut index = 0;
        while index < MAX_QUOTE_LEVELS {
            levels[index] = Self(index as u8);
            index += 1;
        }
        levels
    };

    #[inline]
    pub const fn new(value: u8) -> Option<Self> {
        if (value as usize) < MAX_QUOTE_LEVELS { Some(Self(value)) } else { None }
    }

    #[inline]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    #[inline]
    pub const fn get(self) -> u8 {
        self.0
    }
}

impl TryFrom<u8> for QuoteLevel {
    type Error = InvalidQuoteLevel;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Self::new(value).ok_or(InvalidQuoteLevel(value))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvalidQuoteLevel(pub u8);

impl core::fmt::Display for InvalidQuoteLevel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            formatter,
            "quote level {} is outside 0..{}",
            self.0, MAX_QUOTE_LEVELS
        )
    }
}

impl std::error::Error for InvalidQuoteLevel {}
