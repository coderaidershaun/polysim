//! Where the crosshair stands: under a mouse, a dragging finger, or pinned where a finger lifted
//! until a click or tap lands in the stack off both plots. It uses no egui type, so it reads alone.

use crate::workstation::chart::view::ChartDomain;

/// One pointer event, already resolved against the plots. `finger` is the browser's id for one
/// finger, which stays the same from touch down to lift. `OffPlot` is a mouse click as well as a
/// tap, in the stack but off both plots.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Touch {
    OnPlot { finger: u64, bucket: u64 },
    OffPlot,
    Moved { finger: u64, bucket: u64 },
    Lifted { finger: u64 },
    Cancelled { finger: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Scrub {
    finger: u64,
    bucket: u64,
}

/// A mouse never pins, so on a desktop with no touch screen nothing here is ever set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct CrosshairPin {
    scrub: Option<Scrub>,
    pinned: Option<u64>,
}

impl CrosshairPin {
    pub(super) fn on_touch(&mut self, touch: Touch) {
        match touch {
            // A lift that landed off the page never arrives, so a new touch takes over a stale
            // scrub instead of waiting for it.
            Touch::OnPlot { finger, bucket } => self.scrub = Some(Scrub { finger, bucket }),
            Touch::OffPlot => *self = Self::default(),
            Touch::Moved { finger, bucket } => {
                if let Some(scrub) = self.scrub.as_mut().filter(|scrub| scrub.finger == finger) {
                    scrub.bucket = bucket;
                }
            }
            Touch::Lifted { finger } => {
                if let Some(scrub) = self.scrub.filter(|scrub| scrub.finger == finger) {
                    self.pinned = Some(scrub.bucket);
                    self.scrub = None;
                }
            }
            Touch::Cancelled { finger } => {
                if self.scrub.is_some_and(|scrub| scrub.finger == finger) {
                    self.scrub = None;
                }
            }
        }
    }

    /// The bucket to mark: under a scrubbing finger, else under the mouse, else the pin.
    pub(super) fn bucket(&self, hovered: Option<u64>) -> Option<u64> {
        self.scrub
            .map(|scrub| scrub.bucket)
            .or(hovered)
            .or(self.pinned)
    }

    /// A pin whose bucket has left the window marks nothing, so it is dropped.
    pub(super) fn forget_outside(&mut self, domain: ChartDomain) {
        self.pinned = self.pinned.filter(|bucket| domain.contains(*bucket));
    }
}
