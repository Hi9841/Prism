//! The launcher open/close state machine (`src/lib/launcherMotion.ts`).

pub const LAUNCHER_MOTION_MS: u64 = 110;
pub const POPOVER_MOTION_MS: u64 = 110;

/// Must stay <= PALETTE_HIDE_DELAY in prism-core (125 ms).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherPhase {
    Hidden,
    Preparing,
    Visible,
    Closing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LauncherEvent {
    Open,
    NativeClose,
    WebClose,
    Presented,
    PresentFailed,
    ExitFinished,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideInvocation {
    None,
    Instant,
    Deferred,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HideOrigin {
    Native,
    Web,
}

impl LauncherPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hidden => "hidden",
            Self::Preparing => "preparing",
            Self::Visible => "visible",
            Self::Closing => "closing",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "hidden" => Self::Hidden,
            "preparing" => Self::Preparing,
            "visible" => Self::Visible,
            "closing" => Self::Closing,
            _ => return None,
        })
    }
}

impl LauncherEvent {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "native-close" => Self::NativeClose,
            "web-close" => Self::WebClose,
            "presented" => Self::Presented,
            "present-failed" => Self::PresentFailed,
            "exit-finished" => Self::ExitFinished,
            _ => return None,
        })
    }
}

impl HideInvocation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Instant => "instant",
            Self::Deferred => "deferred",
        }
    }
}

impl HideOrigin {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "native" => Self::Native,
            "web" => Self::Web,
            _ => return None,
        })
    }
}

pub fn reduce_launcher_phase(phase: LauncherPhase, event: LauncherEvent) -> LauncherPhase {
    use LauncherPhase::*;
    match event {
        LauncherEvent::Open => {
            if phase == Closing || phase == Visible {
                Visible
            } else {
                Preparing
            }
        }
        LauncherEvent::Presented => {
            if phase == Preparing {
                Visible
            } else {
                phase
            }
        }
        LauncherEvent::PresentFailed => {
            if phase == Preparing || phase == Visible {
                Hidden
            } else {
                phase
            }
        }
        LauncherEvent::NativeClose | LauncherEvent::WebClose => match phase {
            Hidden | Closing => phase,
            Preparing => Hidden,
            Visible => Closing,
        },
        LauncherEvent::ExitFinished => {
            if phase == Closing {
                Hidden
            } else {
                phase
            }
        }
    }
}

pub fn hide_palette_invocation(origin: HideOrigin, next_phase: LauncherPhase) -> HideInvocation {
    if origin == HideOrigin::Native {
        return HideInvocation::None;
    }
    match next_phase {
        LauncherPhase::Hidden => HideInvocation::Instant,
        LauncherPhase::Closing => HideInvocation::Deferred,
        _ => HideInvocation::None,
    }
}

// PORT STATUS: confidence=high todos=0
