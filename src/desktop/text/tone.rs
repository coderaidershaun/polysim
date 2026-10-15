//! The colour roles the painters and the bridge agree on, as names. Each side decides what a role
//! looks like, so the words and the roles are stated once and the hues are not.

use crate::labelled_enum::labelled_enum;

labelled_enum! {
    /// A role a piece of text or a shape plays, not a hue.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub(crate) enum Tone {
        Text = "text",
        Text2 = "text2",
        Bid = "bid",
        Ask = "ask",
        SellFill = "sellFill",
        Positive = "positive",
        Negative = "negative",
        Warning = "warning",
        Invalid = "invalid",
        Stale = "stale",
        Selected = "selected",
    }
    /// The role's name on the wire.
    pub(crate) fn as_str;
}
