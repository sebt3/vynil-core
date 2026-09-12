//! Date/time helper for Rhai.
//!
//! Provides `DateTimeHandler` (`date_now` + `format`) via `chrono_rhai_register`.

use chrono::{DateTime, Local};
#[cfg(feature = "rhai")] use rhai::{Engine, ImmutableString};

/// Local date-time handle. Create with [`DateTimeHandler::now`] then [`DateTimeHandler::format`].
#[derive(Clone, Eq, PartialEq, Ord, PartialOrd, Hash, Debug)]
pub struct DateTimeHandler {
    /// The wrapped local date-time value.
    pub date: DateTime<Local>,
}
impl DateTimeHandler {
    /// Current local date-time.
    #[must_use]
    pub fn now() -> Self {
        Self { date: Local::now() }
    }

    /// Formats the date with a `chrono` format string.
    #[must_use]
    pub fn format(&self, fmt: &str) -> String {
        format!("{}", self.date.format(fmt))
    }

    /// Rhai binding of [`DateTimeHandler::format`].
    // signature imposée par l'API Rhai (vyvil-core.sdd)
    #[allow(clippy::needless_pass_by_value)]
    #[cfg(feature = "rhai")]
    pub fn rhai_format(&mut self, fmt: String) -> ImmutableString {
        self.format(&fmt).into()
    }
}

/// Registers the `DateTimeHandler` type and its date helpers on a Rhai `engine`.
#[cfg(feature = "rhai")]
pub fn chrono_rhai_register(engine: &mut Engine) {
    engine
        .register_type_with_name::<DateTimeHandler>("DateTimeHandler")
        .register_fn("date_now", DateTimeHandler::now)
        .register_fn("format", DateTimeHandler::rhai_format);
}
